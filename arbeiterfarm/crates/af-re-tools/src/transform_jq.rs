//! `transform.jq` — Apply jq expressions to JSON artifacts.
//!
//! Uses `jaq-interpret` + `jaq-parse` (pure-Rust jq implementation, stable 1.x API).
//! Registers native builtins (length, keys, type, select, map, etc.) and a jq
//! standard library prelude so that common jq operations work as expected.

use af_builtin_tools::envelope::{OopArtifact, OopResult, ProducedFile};
use jaq_interpret::{Args, Ctx, Error, FilterT, Native, ParseCtx, RcIter, Val};
use serde_json::{json, Map, Value};
use std::path::Path;
use std::rc::Rc;

/// Max output size: 64 MB.
const MAX_OUTPUT_SIZE: usize = 64 * 1024 * 1024;

/// jq standard library prelude — functions defined in pure jq on top of native builtins.
const JQ_PRELUDE: &str = r#"
def select(f): if f then . else empty end;
def map(f): [.[] | f];
def map_values(f): .[] |= f;
def with_entries(f): to_entries | map(f) | from_entries;
def recurse(f): def r: ., (f | r); r;
def recurse: recurse(.[]?);
def limit(n; f): foreach f as $x (n; . - 1; $x, if . <= 0 then error else empty end);
def first(f): limit(1; f);
def last(f): reduce f as $x (null; $x);
def isempty(f): first((f | false), true);
def any(f): reduce (.[] | f) as $x (false; . or $x);
def all(f): reduce (.[] | f) as $x (true; . and $x);
def any: any(. == true);
def all: all(. == true);
def in(xs): . as $x | xs | has($x);
def del(f): delpaths([path(f)]);
def from_entries: map({(.key // .name // "key"): .value}) | add // {};
def to_entries: [keys[] as $k | {key: $k, value: .[$k]}];
def ascii: explode | .[0];
def leaf_paths: paths | select(getpath(.) | type != "array" and type != "object");
def index(s): indices(s) | .[0];
def rindex(s): indices(s) | .[-1];
def scan(re): [match(re; "g")] | .[].string;
def splits(re): split(re) | .[];
def sub(re; s): . as $in | [match(re)] | if length > 0 then .[0] | $in[:.offset] + s + $in[.offset+.length:] else $in end;
def gsub(re; s): split(re) | join(s);
def ascii_downcase: explode | map(if . >= 65 and . <= 90 then . + 32 else . end) | implode;
def ascii_upcase: explode | map(if . >= 97 and . <= 122 then . - 32 else . end) | implode;
def sort_by(f): group_by(f) | map(sort_by_key(f)) | add;
def unique_by(f): group_by(f) | map(.[0]);
def min_by(f): reduce .[] as $x (null; if . == null then $x elif ($x | f) < (. | f) then $x else . end);
def max_by(f): reduce .[] as $x (null; if . == null then $x elif ($x | f) > (. | f) then $x else . end);
def group_by(f): [.[] | [., (f)]] | sort_by_key(.[1]) | _group_by_sorted | map(map(.[0]));
"#;

type ValR2s<'a> = Box<dyn Iterator<Item = Result<Val, Error<Val>>> + 'a>;

fn once_ok(v: Val) -> ValR2s<'static> {
    Box::new(std::iter::once(Ok(v)))
}

fn once_err(e: Error<Val>) -> ValR2s<'static> {
    Box::new(std::iter::once(Err(e)))
}

/// Build a Val::Obj by going through serde_json::Value (avoids indexmap/ahash dependency).
fn make_obj(pairs: Vec<(&str, Val)>) -> Val {
    let mut m = Map::new();
    for (k, v) in pairs {
        let jv: Value = v.into();
        m.insert(k.to_string(), jv);
    }
    Val::from(Value::Object(m))
}

// ---------------------------------------------------------------------------
// 0-arity native builtins
// ---------------------------------------------------------------------------

fn native_length<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match &val {
        Val::Null => once_ok(Val::Int(0)),
        Val::Bool(_) => once_err(Error::Type(val, jaq_interpret::error::Type::Iter)),
        Val::Int(n) => once_ok(Val::Int((*n).unsigned_abs() as isize)),
        Val::Float(x) => once_ok(Val::Float(x.abs())),
        Val::Num(s) => match s.parse::<f64>() {
            Ok(x) => once_ok(Val::Float(x.abs())),
            Err(_) => once_err(Error::Type(val, jaq_interpret::error::Type::Num)),
        },
        Val::Str(s) => once_ok(Val::Int(s.chars().count() as isize)),
        Val::Arr(a) => once_ok(Val::Int(a.len() as isize)),
        Val::Obj(o) => once_ok(Val::Int(o.len() as isize)),
    }
}

fn native_keys<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Obj(ref o) => {
            let mut keys: Vec<Val> = o.keys().map(|k| Val::Str(Rc::clone(k))).collect();
            keys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            once_ok(Val::from(keys.into_iter().collect::<Val>()))
        }
        Val::Arr(ref a) => {
            let keys: Vec<Val> = (0..a.len()).map(|i| Val::Int(i as isize)).collect();
            once_ok(keys.into_iter().collect::<Val>())
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Iter)),
    }
}

fn native_keys_unsorted<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Obj(ref o) => {
            let keys: Vec<Val> = o.keys().map(|k| Val::Str(Rc::clone(k))).collect();
            once_ok(keys.into_iter().collect::<Val>())
        }
        Val::Arr(ref a) => {
            let keys: Vec<Val> = (0..a.len()).map(|i| Val::Int(i as isize)).collect();
            once_ok(keys.into_iter().collect::<Val>())
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Iter)),
    }
}

fn native_values<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Obj(ref o) => {
            let vals: Vec<Val> = o.values().cloned().collect();
            once_ok(vals.into_iter().collect::<Val>())
        }
        Val::Arr(_) => once_ok(val),
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Iter)),
    }
}

fn native_type<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let t = match val {
        Val::Null => "null",
        Val::Bool(_) => "boolean",
        Val::Int(_) | Val::Float(_) | Val::Num(_) => "number",
        Val::Str(_) => "string",
        Val::Arr(_) => "array",
        Val::Obj(_) => "object",
    };
    once_ok(Val::from(t.to_string()))
}

fn native_empty<'a>(_args: Args<'a>, _cv: (Ctx<'a>, Val)) -> ValR2s<'a> {
    Box::new(std::iter::empty())
}

fn native_error<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    once_err(Error::Val(val))
}

fn native_not<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let b = matches!(val, Val::Null | Val::Bool(false));
    once_ok(Val::Bool(b))
}

fn native_null<'a>(_args: Args<'a>, _cv: (Ctx<'a>, Val)) -> ValR2s<'a> {
    once_ok(Val::Null)
}

fn native_sort<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Arr(a) => {
            let mut v: Vec<Val> = (*a).clone();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            once_ok(v.into_iter().collect::<Val>())
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Arr)),
    }
}

fn native_reverse<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Arr(a) => {
            let mut v: Vec<Val> = (*a).clone();
            v.reverse();
            once_ok(v.into_iter().collect::<Val>())
        }
        Val::Str(s) => {
            let reversed: String = s.chars().rev().collect();
            once_ok(Val::from(reversed))
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Arr)),
    }
}

fn native_unique<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Arr(a) => {
            let mut v: Vec<Val> = (*a).clone();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            v.dedup();
            once_ok(v.into_iter().collect::<Val>())
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Arr)),
    }
}

fn native_flatten<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Arr(a) => {
            let mut out = Vec::new();
            for item in a.iter() {
                match item {
                    Val::Arr(inner) => out.extend(inner.iter().cloned()),
                    other => out.push(other.clone()),
                }
            }
            once_ok(out.into_iter().collect::<Val>())
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Arr)),
    }
}

fn native_add<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Arr(ref a) => {
            if a.is_empty() {
                return once_ok(Val::Null);
            }
            let mut acc = a[0].clone();
            for item in a.iter().skip(1) {
                acc = match (acc, item) {
                    (Val::Null, v) => v.clone(),
                    (Val::Int(a), Val::Int(b)) => Val::Int(a + b),
                    (Val::Float(a), Val::Float(b)) => Val::Float(a + b),
                    (Val::Int(a), Val::Float(b)) => Val::Float(a as f64 + b),
                    (Val::Float(a), Val::Int(b)) => Val::Float(a + *b as f64),
                    (Val::Str(a), Val::Str(b)) => {
                        Val::from(format!("{}{}", &*a, &*b))
                    }
                    (Val::Arr(a), Val::Arr(b)) => {
                        let mut v = (*a).clone();
                        v.extend(b.iter().cloned());
                        v.into_iter().collect::<Val>()
                    }
                    (Val::Obj(a), Val::Obj(b)) => {
                        let mut m = (*a).clone();
                        for (k, v) in b.iter() {
                            m.insert(Rc::clone(k), v.clone());
                        }
                        Val::Obj(Rc::new(m))
                    }
                    (a, b) => return once_err(Error::MathOp(a, jaq_syn::MathOp::Add, b.clone())),
                };
            }
            once_ok(acc)
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Arr)),
    }
}

fn native_min<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Arr(ref a) if a.is_empty() => once_ok(Val::Null),
        Val::Arr(ref a) => {
            let mut min = a[0].clone();
            for item in a.iter().skip(1) {
                if item.partial_cmp(&min).unwrap_or(std::cmp::Ordering::Equal)
                    == std::cmp::Ordering::Less
                {
                    min = item.clone();
                }
            }
            once_ok(min)
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Arr)),
    }
}

fn native_max<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Arr(ref a) if a.is_empty() => once_ok(Val::Null),
        Val::Arr(ref a) => {
            let mut max = a[0].clone();
            for item in a.iter().skip(1) {
                if item.partial_cmp(&max).unwrap_or(std::cmp::Ordering::Equal)
                    == std::cmp::Ordering::Greater
                {
                    max = item.clone();
                }
            }
            once_ok(max)
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Arr)),
    }
}

fn native_tonumber<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Int(_) | Val::Float(_) | Val::Num(_) => once_ok(val),
        Val::Str(ref s) => match s.parse::<isize>() {
            Ok(n) => once_ok(Val::Int(n)),
            Err(_) => match s.parse::<f64>() {
                Ok(f) => once_ok(Val::Float(f)),
                Err(_) => once_err(Error::Type(val, jaq_interpret::error::Type::Num)),
            },
        },
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Num)),
    }
}

fn native_tojson<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let json_val: serde_json::Value = val.into();
    let s = serde_json::to_string(&json_val).unwrap_or_default();
    once_ok(Val::from(s))
}

fn native_fromjson<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Str(ref s) => match serde_json::from_str::<serde_json::Value>(s) {
            Ok(v) => once_ok(Val::from(v)),
            Err(e) => once_err(Error::Val(Val::from(format!("fromjson: {e}")))),
        },
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Str)),
    }
}

fn native_to_entries<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Obj(ref o) => {
            let entries: Vec<Val> = o
                .iter()
                .map(|(k, v)| {
                    make_obj(vec![
                        ("key", Val::Str(Rc::clone(k))),
                        ("value", v.clone()),
                    ])
                })
                .collect();
            once_ok(entries.into_iter().collect::<Val>())
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Iter)),
    }
}

fn native_from_entries<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Arr(ref a) => {
            let mut m = serde_json::Map::new();
            for entry in a.iter() {
                if let Val::Obj(ref o) = entry {
                    let key = o
                        .get(&Rc::new("key".to_string()))
                        .or_else(|| o.get(&Rc::new("name".to_string())))
                        .cloned()
                        .unwrap_or(Val::Str(Rc::new("key".to_string())));
                    let key_str = match key {
                        Val::Str(s) => (*s).clone(),
                        other => {
                            let jv: Value = other.into();
                            serde_json::to_string(&jv).unwrap_or_default()
                        }
                    };
                    let value = o
                        .get(&Rc::new("value".to_string()))
                        .cloned()
                        .unwrap_or(Val::Null);
                    let jv: Value = value.into();
                    m.insert(key_str, jv);
                }
            }
            once_ok(Val::from(Value::Object(m)))
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Arr)),
    }
}

fn native_explode<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Str(ref s) => {
            let codepoints: Vec<Val> = s.chars().map(|c| Val::Int(c as isize)).collect();
            once_ok(codepoints.into_iter().collect::<Val>())
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Str)),
    }
}

fn native_implode<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Arr(ref a) => {
            let mut s = String::new();
            for item in a.iter() {
                match item {
                    Val::Int(n) => {
                        if let Some(c) = char::from_u32(*n as u32) {
                            s.push(c);
                        }
                    }
                    _ => return once_err(Error::Type(val, jaq_interpret::error::Type::Int)),
                }
            }
            once_ok(Val::from(s))
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Arr)),
    }
}

fn native_paths<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let mut paths = Vec::new();
    collect_paths(&val, &mut Vec::new(), &mut paths);
    Box::new(paths.into_iter().map(Ok))
}

fn collect_paths(val: &Val, current: &mut Vec<Val>, out: &mut Vec<Val>) {
    match val {
        Val::Arr(a) => {
            for (i, item) in a.iter().enumerate() {
                current.push(Val::Int(i as isize));
                out.push(current.iter().cloned().collect::<Val>());
                collect_paths(item, current, out);
                current.pop();
            }
        }
        Val::Obj(o) => {
            for (k, v) in o.iter() {
                current.push(Val::Str(Rc::clone(k)));
                out.push(current.iter().cloned().collect::<Val>());
                collect_paths(v, current, out);
                current.pop();
            }
        }
        _ => {}
    }
}

fn native_floor<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Int(_) => once_ok(val),
        Val::Float(x) => once_ok(Val::Float(x.floor())),
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Num)),
    }
}

fn native_ceil<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Int(_) => once_ok(val),
        Val::Float(x) => once_ok(Val::Float(x.ceil())),
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Num)),
    }
}

fn native_round<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Int(_) => once_ok(val),
        Val::Float(x) => once_ok(Val::Float(x.round())),
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Num)),
    }
}

fn native_sqrt<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Int(n) => once_ok(Val::Float((n as f64).sqrt())),
        Val::Float(x) => once_ok(Val::Float(x.sqrt())),
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Num)),
    }
}

fn native_fabs<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Int(n) => once_ok(Val::Int(n.unsigned_abs() as isize)),
        Val::Float(x) => once_ok(Val::Float(x.abs())),
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Num)),
    }
}

fn native_nan<'a>(_args: Args<'a>, _cv: (Ctx<'a>, Val)) -> ValR2s<'a> {
    once_ok(Val::Float(f64::NAN))
}

fn native_infinite<'a>(_args: Args<'a>, _cv: (Ctx<'a>, Val)) -> ValR2s<'a> {
    once_ok(Val::Float(f64::INFINITY))
}

fn native_isinfinite<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Float(x) => once_ok(Val::Bool(x.is_infinite())),
        Val::Int(_) => once_ok(Val::Bool(false)),
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Num)),
    }
}

fn native_isnan<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Float(x) => once_ok(Val::Bool(x.is_nan())),
        Val::Int(_) => once_ok(Val::Bool(false)),
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Num)),
    }
}

fn native_isnormal<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Float(x) => once_ok(Val::Bool(x.is_normal())),
        Val::Int(n) => once_ok(Val::Bool(n != 0)),
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Num)),
    }
}

fn native_debug<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let json_val: serde_json::Value = val.clone().into();
    eprintln!("[DEBUG] {}", serde_json::to_string(&json_val).unwrap_or_default());
    once_ok(val)
}

fn native_builtins<'a>(_args: Args<'a>, _cv: (Ctx<'a>, Val)) -> ValR2s<'a> {
    let names = vec![
        "length/0", "keys/0", "keys_unsorted/0", "values/0", "type/0",
        "empty/0", "error/0", "not/0", "null/0", "sort/0", "reverse/0",
        "unique/0", "flatten/0", "add/0", "min/0", "max/0", "tonumber/0",
        "tojson/0", "fromjson/0", "to_entries/0", "from_entries/0",
        "explode/0", "implode/0", "paths/0", "floor/0", "ceil/0",
        "round/0", "sqrt/0", "fabs/0", "nan/0", "infinite/0",
        "isinfinite/0", "isnan/0", "isnormal/0", "debug/0", "builtins/0",
        "has/1", "contains/1", "inside/1", "split/1", "join/1",
        "test/1", "match/1", "startswith/1", "endswith/1",
        "ltrimstr/1", "rtrimstr/1", "getpath/1", "indices/1",
        "range/1", "range/2",
        "select/1", "map/1", "map_values/1", "with_entries/1",
        "any/1", "all/1", "any/0", "all/0", "recurse/1", "recurse/0",
        "first/1", "last/1", "limit/2", "isempty/1", "del/1",
        "sort_by/1", "group_by/1", "unique_by/1", "min_by/1", "max_by/1",
        "ascii_downcase/0", "ascii_upcase/0",
    ];
    let arr: Vec<Val> = names.into_iter().map(|s| Val::from(s.to_string())).collect();
    once_ok(arr.into_iter().collect::<Val>())
}

// ---------------------------------------------------------------------------
// 1-arity native builtins (take one filter argument)
// ---------------------------------------------------------------------------

fn native_has<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |k_result| {
        let k = k_result?;
        match (&val, &k) {
            (Val::Obj(obj), Val::Str(s)) => Ok(Val::Bool(obj.contains_key(s))),
            (Val::Arr(arr), Val::Int(i)) => {
                let idx = if *i < 0 {
                    arr.len() as isize + *i
                } else {
                    *i
                } as usize;
                Ok(Val::Bool(idx < arr.len()))
            }
            _ => Err(Error::Index(val.clone(), k)),
        }
    }))
}

fn native_contains<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |b_result| {
        let b = b_result?;
        Ok(Val::Bool(val_contains(&val, &b)))
    }))
}

fn val_contains(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Str(a), Val::Str(b)) => a.contains(b.as_str()),
        (Val::Arr(a), Val::Arr(b)) => b.iter().all(|bi| a.iter().any(|ai| val_contains(ai, bi))),
        (Val::Obj(a), Val::Obj(b)) => b
            .iter()
            .all(|(k, bv)| a.get(k).map_or(false, |av| val_contains(av, bv))),
        (a, b) => a == b,
    }
}

fn native_inside<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |b_result| {
        let b = b_result?;
        Ok(Val::Bool(val_contains(&b, &val)))
    }))
}

fn native_split<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |sep_result| {
        let sep = sep_result?;
        match (&val, &sep) {
            (Val::Str(s), Val::Str(d)) => {
                let parts: Vec<Val> = s.split(d.as_str()).map(|p| Val::from(p.to_string())).collect();
                Ok(parts.into_iter().collect::<Val>())
            }
            _ => Err(Error::Type(val.clone(), jaq_interpret::error::Type::Str)),
        }
    }))
}

fn native_join<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |sep_result| {
        let sep = sep_result?;
        match (&val, &sep) {
            (Val::Arr(a), Val::Str(d)) => {
                let parts: Vec<String> = a
                    .iter()
                    .map(|v| match v {
                        Val::Str(s) => s.to_string(),
                        Val::Null => String::new(),
                        other => {
                            let jv: serde_json::Value = other.clone().into();
                            serde_json::to_string(&jv).unwrap_or_default()
                        }
                    })
                    .collect();
                Ok(Val::from(parts.join(d.as_str())))
            }
            _ => Err(Error::Type(val.clone(), jaq_interpret::error::Type::Arr)),
        }
    }))
}

fn native_test<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |re_result| {
        let re_val = re_result?;
        match (&val, &re_val) {
            (Val::Str(s), Val::Str(pattern)) => {
                match regex::Regex::new(pattern) {
                    Ok(re) => Ok(Val::Bool(re.is_match(s))),
                    Err(e) => Err(Error::Val(Val::from(format!("test: invalid regex: {e}")))),
                }
            }
            _ => Err(Error::Type(val.clone(), jaq_interpret::error::Type::Str)),
        }
    }))
}

fn native_match<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |re_result| {
        let re_val = re_result?;
        match (&val, &re_val) {
            (Val::Str(s), Val::Str(pattern)) => {
                match regex::Regex::new(pattern) {
                    Ok(re) => {
                        if let Some(m) = re.find(s) {
                            let captures: Vec<serde_json::Value> = re.captures(s).map_or_else(Vec::new, |caps| {
                                caps.iter().skip(1).map(|c| {
                                    match c {
                                        Some(m) => serde_json::json!({
                                            "offset": m.start(),
                                            "length": m.len(),
                                            "string": m.as_str()
                                        }),
                                        None => serde_json::Value::Null,
                                    }
                                }).collect()
                            });
                            let obj = serde_json::json!({
                                "offset": m.start(),
                                "length": m.len(),
                                "string": m.as_str(),
                                "captures": captures
                            });
                            Ok(Val::from(obj))
                        } else {
                            Err(Error::Val(Val::Null))
                        }
                    }
                    Err(e) => Err(Error::Val(Val::from(format!("match: invalid regex: {e}")))),
                }
            }
            _ => Err(Error::Type(val.clone(), jaq_interpret::error::Type::Str)),
        }
    }))
}

fn native_startswith<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |s_result| {
        let s = s_result?;
        match (&val, &s) {
            (Val::Str(a), Val::Str(b)) => Ok(Val::Bool(a.starts_with(b.as_str()))),
            _ => Err(Error::Type(val.clone(), jaq_interpret::error::Type::Str)),
        }
    }))
}

fn native_endswith<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |s_result| {
        let s = s_result?;
        match (&val, &s) {
            (Val::Str(a), Val::Str(b)) => Ok(Val::Bool(a.ends_with(b.as_str()))),
            _ => Err(Error::Type(val.clone(), jaq_interpret::error::Type::Str)),
        }
    }))
}

fn native_ltrimstr<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |s_result| {
        let s = s_result?;
        match (&val, &s) {
            (Val::Str(a), Val::Str(b)) => {
                if let Some(rest) = a.strip_prefix(b.as_str()) {
                    Ok(Val::from(rest.to_string()))
                } else {
                    Ok(val.clone())
                }
            }
            _ => Ok(val.clone()),
        }
    }))
}

fn native_rtrimstr<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |s_result| {
        let s = s_result?;
        match (&val, &s) {
            (Val::Str(a), Val::Str(b)) => {
                if let Some(rest) = a.strip_suffix(b.as_str()) {
                    Ok(Val::from(rest.to_string()))
                } else {
                    Ok(val.clone())
                }
            }
            _ => Ok(val.clone()),
        }
    }))
}

fn native_getpath<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |p_result| {
        let path = p_result?;
        match path {
            Val::Arr(ref keys) => {
                let mut current = val.clone();
                for key in keys.iter() {
                    current = match (&current, key) {
                        (Val::Obj(o), Val::Str(k)) => o.get(k).cloned().unwrap_or(Val::Null),
                        (Val::Arr(a), Val::Int(i)) => {
                            let idx = if *i < 0 {
                                a.len() as isize + *i
                            } else {
                                *i
                            } as usize;
                            a.get(idx).cloned().unwrap_or(Val::Null)
                        }
                        _ => Val::Null,
                    };
                }
                Ok(current)
            }
            _ => Err(Error::Type(path, jaq_interpret::error::Type::Arr)),
        }
    }))
}

fn native_indices<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    Box::new(arg_filter.run((ctx, val.clone())).map(move |s_result| {
        let needle = s_result?;
        match (&val, &needle) {
            (Val::Str(haystack), Val::Str(needle_str)) => {
                let mut indices = Vec::new();
                let mut start = 0;
                while let Some(pos) = haystack[start..].find(needle_str.as_str()) {
                    indices.push(Val::Int((start + pos) as isize));
                    start += pos + 1;
                }
                Ok(indices.into_iter().collect::<Val>())
            }
            (Val::Arr(haystack), _) => {
                let indices: Vec<Val> = haystack
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| *item == &needle)
                    .map(|(i, _)| Val::Int(i as isize))
                    .collect();
                Ok(indices.into_iter().collect::<Val>())
            }
            _ => Err(Error::Type(val.clone(), jaq_interpret::error::Type::Iter)),
        }
    }))
}

fn native_range1<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let arg_filter = args.get(0);
    let results: Vec<Result<Val, Error<Val>>> = arg_filter
        .run((ctx, val))
        .flat_map(|n_result| match n_result {
            Ok(Val::Int(n)) => {
                let v: Vec<_> = (0..n).map(|i| Ok(Val::Int(i))).collect();
                v.into_iter()
            }
            Ok(Val::Float(n)) => {
                let n = n as isize;
                let v: Vec<_> = (0..n).map(|i| Ok(Val::Int(i))).collect();
                v.into_iter()
            }
            Ok(other) => vec![Err(Error::Type(other, jaq_interpret::error::Type::Int))].into_iter(),
            Err(e) => vec![Err(e)].into_iter(),
        })
        .collect();
    Box::new(results.into_iter())
}

fn native_range2<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    let from_filter = args.get(0);
    let to_filter = args.get(1);
    let from_results: Vec<_> = from_filter.run((ctx.clone(), val.clone())).collect();
    let to_results: Vec<_> = to_filter.run((ctx, val)).collect();

    let mut out = Vec::new();
    for from_r in &from_results {
        for to_r in &to_results {
            match (from_r, to_r) {
                (Ok(Val::Int(from)), Ok(Val::Int(to))) => {
                    for i in *from..*to {
                        out.push(Ok(Val::Int(i)));
                    }
                }
                (Ok(Val::Float(from)), Ok(Val::Float(to))) => {
                    let mut i = *from;
                    while i < *to {
                        out.push(Ok(Val::Float(i)));
                        i += 1.0;
                    }
                }
                (Ok(Val::Int(from)), Ok(Val::Float(to))) => {
                    let mut i = *from as f64;
                    while i < *to {
                        out.push(Ok(Val::Float(i)));
                        i += 1.0;
                    }
                }
                (Ok(other), _) => {
                    out.push(Err(Error::Type(other.clone(), jaq_interpret::error::Type::Int)));
                }
                (Err(e), _) => {
                    out.push(Err(e.clone()));
                }
            }
        }
    }
    Box::new(out.into_iter())
}

// Internal helpers for prelude (sort_by_key, _group_by_sorted)
fn native_sort_by_key<'a>(args: Args<'a>, (ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    match val {
        Val::Arr(ref a) => {
            let f = args.get(0);
            let mut keyed: Vec<(Val, Val)> = Vec::new();
            for item in a.iter() {
                let key = f.clone().run((ctx.clone(), item.clone())).next();
                let key = match key {
                    Some(Ok(k)) => k,
                    Some(Err(e)) => return once_err(e),
                    None => Val::Null,
                };
                keyed.push((key, item.clone()));
            }
            keyed.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let sorted: Vec<Val> = keyed.into_iter().map(|(_, v)| v).collect();
            once_ok(sorted.into_iter().collect::<Val>())
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Arr)),
    }
}

fn native_group_by_sorted<'a>(_args: Args<'a>, (_ctx, val): (Ctx<'a>, Val)) -> ValR2s<'a> {
    // Groups consecutive equal elements (expects sorted input)
    match val {
        Val::Arr(ref a) => {
            if a.is_empty() {
                return once_ok(Val::Arr(Rc::new(Vec::new())));
            }
            let mut groups: Vec<Val> = Vec::new();
            let mut current_group: Vec<Val> = vec![a[0].clone()];
            for item in a.iter().skip(1) {
                if item == &current_group[0] {
                    current_group.push(item.clone());
                } else {
                    groups.push(current_group.into_iter().collect::<Val>());
                    current_group = vec![item.clone()];
                }
            }
            groups.push(current_group.into_iter().collect::<Val>());
            once_ok(groups.into_iter().collect::<Val>())
        }
        _ => once_err(Error::Type(val, jaq_interpret::error::Type::Arr)),
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

fn register_natives(defs: &mut ParseCtx) {
    let natives: Vec<(String, usize, Native)> = vec![
        // 0-arity
        ("length".into(), 0, Native::new(native_length)),
        ("keys".into(), 0, Native::new(native_keys)),
        ("keys_unsorted".into(), 0, Native::new(native_keys_unsorted)),
        ("values".into(), 0, Native::new(native_values)),
        ("type".into(), 0, Native::new(native_type)),
        ("empty".into(), 0, Native::new(native_empty)),
        ("error".into(), 0, Native::new(native_error)),
        ("not".into(), 0, Native::new(native_not)),
        ("null".into(), 0, Native::new(native_null)),
        ("sort".into(), 0, Native::new(native_sort)),
        ("reverse".into(), 0, Native::new(native_reverse)),
        ("unique".into(), 0, Native::new(native_unique)),
        ("flatten".into(), 0, Native::new(native_flatten)),
        ("add".into(), 0, Native::new(native_add)),
        ("min".into(), 0, Native::new(native_min)),
        ("max".into(), 0, Native::new(native_max)),
        ("tonumber".into(), 0, Native::new(native_tonumber)),
        ("tojson".into(), 0, Native::new(native_tojson)),
        ("fromjson".into(), 0, Native::new(native_fromjson)),
        ("to_entries".into(), 0, Native::new(native_to_entries)),
        ("from_entries".into(), 0, Native::new(native_from_entries)),
        ("explode".into(), 0, Native::new(native_explode)),
        ("implode".into(), 0, Native::new(native_implode)),
        ("paths".into(), 0, Native::new(native_paths)),
        ("floor".into(), 0, Native::new(native_floor)),
        ("ceil".into(), 0, Native::new(native_ceil)),
        ("round".into(), 0, Native::new(native_round)),
        ("sqrt".into(), 0, Native::new(native_sqrt)),
        ("fabs".into(), 0, Native::new(native_fabs)),
        ("nan".into(), 0, Native::new(native_nan)),
        ("infinite".into(), 0, Native::new(native_infinite)),
        ("isinfinite".into(), 0, Native::new(native_isinfinite)),
        ("isnan".into(), 0, Native::new(native_isnan)),
        ("isnormal".into(), 0, Native::new(native_isnormal)),
        ("debug".into(), 0, Native::new(native_debug)),
        ("builtins".into(), 0, Native::new(native_builtins)),
        // 1-arity
        ("has".into(), 1, Native::new(native_has)),
        ("contains".into(), 1, Native::new(native_contains)),
        ("inside".into(), 1, Native::new(native_inside)),
        ("split".into(), 1, Native::new(native_split)),
        ("join".into(), 1, Native::new(native_join)),
        ("test".into(), 1, Native::new(native_test)),
        ("match".into(), 1, Native::new(native_match)),
        ("startswith".into(), 1, Native::new(native_startswith)),
        ("endswith".into(), 1, Native::new(native_endswith)),
        ("ltrimstr".into(), 1, Native::new(native_ltrimstr)),
        ("rtrimstr".into(), 1, Native::new(native_rtrimstr)),
        ("getpath".into(), 1, Native::new(native_getpath)),
        ("indices".into(), 1, Native::new(native_indices)),
        ("range".into(), 1, Native::new(native_range1)),
        ("sort_by_key".into(), 1, Native::new(native_sort_by_key)),
        ("_group_by_sorted".into(), 0, Native::new(native_group_by_sorted)),
        // 2-arity
        ("range".into(), 2, Native::new(native_range2)),
    ];
    defs.insert_natives(natives);
}

pub fn execute(artifact: &OopArtifact, input: &Value, scratch_dir: &Path) -> OopResult {
    let expression = match input.get("expression").and_then(|v| v.as_str()) {
        Some(e) => e,
        None => {
            return OopResult::Error {
                code: "missing_expression".into(),
                message: "expression parameter is required".into(),
                retryable: false,
            }
        }
    };
    let raw_output = input
        .get("raw_output")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Read and parse JSON
    let content = match std::fs::read_to_string(&artifact.storage_path) {
        Ok(c) => c,
        Err(e) => {
            return OopResult::Error {
                code: "read_error".into(),
                message: format!("failed to read artifact: {e}"),
                retryable: false,
            }
        }
    };

    let input_val: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            return OopResult::Error {
                code: "json_parse_error".into(),
                message: format!("artifact is not valid JSON: {e}"),
                retryable: false,
            }
        }
    };

    // Build expression with prelude prepended
    let full_expression = format!("{JQ_PRELUDE}\n{expression}");

    // Parse jq expression (with prelude definitions)
    let (filter, errs) = jaq_parse::parse(&full_expression, jaq_parse::main());
    if !errs.is_empty() {
        // Retry without prelude in case of conflict
        let (filter2, errs2) = jaq_parse::parse(expression, jaq_parse::main());
        if errs2.is_empty() && filter2.is_some() {
            // Prelude had a conflict; use expression as-is
            return run_filter(filter2.unwrap(), expression, raw_output, input_val, scratch_dir);
        }
        let err_strs: Vec<String> = errs.iter().map(|e| format!("{e:?}")).collect();
        return OopResult::Error {
            code: "jq_parse_error".into(),
            message: format!("failed to parse jq expression: {}", err_strs.join(", ")),
            retryable: false,
        };
    }

    let filter = match filter {
        Some(f) => f,
        None => {
            return OopResult::Error {
                code: "jq_parse_error".into(),
                message: "jq expression parsed to nothing".into(),
                retryable: false,
            }
        }
    };

    run_filter(filter, expression, raw_output, input_val, scratch_dir)
}

fn run_filter(
    filter: jaq_syn::Main,
    expression: &str,
    raw_output: bool,
    input_val: Value,
    scratch_dir: &Path,
) -> OopResult {
    // Compile filter with native builtins
    let mut defs = ParseCtx::new(Vec::new());
    register_natives(&mut defs);
    let compiled = defs.compile(filter);
    if !defs.errs.is_empty() {
        return OopResult::Error {
            code: "jq_compile_error".into(),
            message: format!(
                "failed to compile jq expression ({}): undefined function or variable",
                expression,
            ),
            retryable: false,
        };
    }

    let inputs = RcIter::new(core::iter::empty());

    // Run filter and collect outputs
    let mut results: Vec<Value> = Vec::new();
    let mut total_size: usize = 0;

    for output in compiled.run((Ctx::new([], &inputs), Val::from(input_val))) {
        match output {
            Ok(val) => {
                let v: Value = val.into();
                let size = serde_json::to_string(&v).map(|s| s.len()).unwrap_or(0);
                total_size += size;
                if total_size > MAX_OUTPUT_SIZE {
                    break;
                }
                results.push(v);
            }
            Err(e) => {
                return OopResult::Error {
                    code: "jq_runtime_error".into(),
                    message: format!("jq evaluation error: {e}"),
                    retryable: false,
                };
            }
        }
    }

    // Build output
    let (filename, output_text) = if raw_output {
        let text: String = results
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => serde_json::to_string(other).unwrap_or_default(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        ("jq_result.txt".to_string(), text)
    } else {
        let text = if results.len() == 1 {
            serde_json::to_string_pretty(&results[0]).unwrap_or_default()
        } else {
            serde_json::to_string_pretty(&results).unwrap_or_default()
        };
        ("jq_result.json".to_string(), text)
    };

    let out_path = scratch_dir.join(&filename);
    if let Err(e) = std::fs::write(&out_path, &output_text) {
        return OopResult::Error {
            code: "write_error".into(),
            message: format!("failed to write jq output: {e}"),
            retryable: false,
        };
    }

    let output_lines = output_text.lines().count();
    let output_size = output_text.len();

    // Preview: first 20 lines
    let preview: String = output_text
        .lines()
        .take(20)
        .collect::<Vec<_>>()
        .join("\n");

    OopResult::Ok {
        output: json!({
            "expression": expression,
            "output_lines": output_lines,
            "output_size": output_size,
            "result_count": results.len(),
            "preview": preview,
            "hint": "Full jq output stored as artifact. Use file.read_range to inspect.",
        }),
        produced_files: vec![ProducedFile {
            filename,
            path: out_path,
            mime_type: if raw_output {
                Some("text/plain".into())
            } else {
                Some("application/json".into())
            },
            description: Some(format!("jq result: {expression}")),
        }],
    }
}
