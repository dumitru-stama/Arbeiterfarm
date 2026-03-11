pub mod agents;
pub mod artifact_describe;
pub mod artifact_search;
pub mod common;
pub mod dedup;
pub mod evidence_resolver;
pub mod family;
pub mod ghidra_analyze;
pub mod ghidra_decompile;
pub mod ghidra_rename;
pub mod ghidra_renames_db;
pub mod ghidra_suggest_renames;
pub mod ioc_extractor;
pub mod ioc_hook;
pub mod ioc_pivot;
pub mod plugin;
pub mod rizin_bininfo;
pub mod rizin_disasm;
pub mod rizin_xrefs;
pub mod specs;
pub mod strings_extract;
pub mod yara_generate;
pub mod yara_hook;
pub mod yara_list;
pub mod yara_scan;
pub mod yara_test;

pub mod transform_convert;
pub mod transform_csv;
pub mod transform_decode;
pub mod transform_jq;
pub mod transform_regex;
pub mod transform_unpack;

pub mod doc_chunk;
pub mod doc_ingest;
pub mod doc_parse;

use af_plugin_api::{
    EvidenceResolverRegistry, Migration, PluginDb, SpawnConfig, ToolExecutorRegistry,
    ToolSpecRegistry,
};
use std::path::Path;
use std::sync::Arc;

/// Pure phase: register tool specs. No runtime deps.
pub fn declare(specs: &mut ToolSpecRegistry) {
    for spec in specs::all_specs() {
        specs.register(spec).expect("failed to register RE tool spec");
    }
}

/// Runtime phase: register executors and evidence resolver.
///
/// If `executor_path` is provided, registers OOP executors that run inside bwrap.
/// Otherwise falls back to InProcess executors (no sandbox isolation).
pub fn wire(
    executors: &mut ToolExecutorRegistry,
    evidence: &mut EvidenceResolverRegistry,
    _plugin_db: Arc<dyn PluginDb>,
    rizin_path: &Path,
    executor_path: Option<&Path>,
) {
    if let Some(exec_path) = executor_path {
        // OOP mode: tools run as separate processes inside bwrap sandbox
        let config = SpawnConfig {
            binary_path: exec_path.to_path_buf(),
            protocol_version: 1,
            supported_tools: vec![
                ("rizin.bininfo".into(), 1),
                ("rizin.disasm".into(), 1),
                ("rizin.xrefs".into(), 1),
                ("strings.extract".into(), 1),
            ],
            context_extra: serde_json::json!({
                "rizin_path": rizin_path.to_string_lossy(),
            }),
        };
        executors
            .register_oop(config)
            .expect("failed to register RE OOP executors");
    } else {
        // InProcess fallback: no bwrap isolation
        executors
            .register(Box::new(rizin_bininfo::RizinBinInfoExecutor {
                rizin_path: rizin_path.to_path_buf(),
            }))
            .expect("failed to register rizin.bininfo executor");

        executors
            .register(Box::new(rizin_disasm::RizinDisasmExecutor {
                rizin_path: rizin_path.to_path_buf(),
            }))
            .expect("failed to register rizin.disasm executor");

        executors
            .register(Box::new(rizin_xrefs::RizinXrefsExecutor {
                rizin_path: rizin_path.to_path_buf(),
            }))
            .expect("failed to register rizin.xrefs executor");

        executors
            .register(Box::new(strings_extract::StringsExtractExecutor {
                rizin_path: rizin_path.to_path_buf(),
            }))
            .expect("failed to register strings.extract executor");
    }

    evidence.register(Box::new(evidence_resolver::ReEvidenceResolver));
}

/// Pure phase: register Ghidra tool specs. Separate from `declare()` because Ghidra is optional.
///
/// Accepts paths so the tool policies include proper bind mount configuration
/// for bwrap sandbox (cache_dir writable, ghidra_home + scripts_dir read-only).
pub fn declare_ghidra(
    specs: &mut ToolSpecRegistry,
    ghidra_home: &Path,
    cache_dir: &Path,
    scripts_dir: &Path,
    java_home: Option<&Path>,
) {
    for spec in specs::ghidra_specs(ghidra_home, cache_dir, scripts_dir, java_home) {
        specs
            .register(spec)
            .expect("failed to register Ghidra tool spec");
    }
}

/// Wire Ghidra tool executors. Separate from `wire()` because Ghidra is optional.
///
/// ghidra.analyze and ghidra.decompile: OOP (bwrap sandbox) or InProcess fallback.
/// ghidra.rename: always InProcess (DB-only, no JVM needed).
pub fn wire_ghidra(
    executors: &mut ToolExecutorRegistry,
    plugin_db: Arc<dyn PluginDb>,
    ghidra_home: &Path,
    cache_dir: &Path,
    scripts_dir: &Path,
    executor_path: Option<&Path>,
) {
    // ghidra.rename: always in-process (DB-only, no JVM)
    executors
        .register(Box::new(ghidra_rename::GhidraRenameExecutor {
            plugin_db: Arc::clone(&plugin_db),
            cache_dir: cache_dir.to_path_buf(),
        }))
        .expect("failed to register ghidra.rename executor");

    // ghidra.suggest_renames: always in-process (DB-only)
    executors
        .register(Box::new(ghidra_suggest_renames::GhidraSuggestRenamesExecutor {
            plugin_db,
        }))
        .expect("failed to register ghidra.suggest_renames executor");

    if let Some(exec_path) = executor_path {
        // ghidra.analyze + ghidra.decompile: OOP (ghidra.rename removed from OOP list)
        let config = SpawnConfig {
            binary_path: exec_path.to_path_buf(),
            protocol_version: 1,
            supported_tools: vec![
                ("ghidra.analyze".into(), 1),
                ("ghidra.decompile".into(), 1),
            ],
            context_extra: serde_json::json!({
                "ghidra_home": ghidra_home.to_string_lossy(),
                "cache_dir": cache_dir.to_string_lossy(),
                "scripts_dir": scripts_dir.to_string_lossy(),
            }),
        };
        executors
            .register_oop(config)
            .expect("failed to register Ghidra OOP executors");
    } else {
        executors
            .register(Box::new(ghidra_analyze::GhidraAnalyzeExecutor {
                ghidra_home: ghidra_home.to_path_buf(),
                cache_dir: cache_dir.to_path_buf(),
                scripts_dir: scripts_dir.to_path_buf(),
            }))
            .expect("failed to register ghidra.analyze executor");

        executors
            .register(Box::new(ghidra_decompile::GhidraDecompileExecutor {
                ghidra_home: ghidra_home.to_path_buf(),
                cache_dir: cache_dir.to_path_buf(),
                scripts_dir: scripts_dir.to_path_buf(),
            }))
            .expect("failed to register ghidra.decompile executor");
    }
}

/// Pure phase: register IOC pivot tool specs. Requires DB but not rizin/Ghidra.
pub fn declare_ioc(specs: &mut ToolSpecRegistry) {
    for spec in specs::ioc_specs() {
        specs
            .register(spec)
            .expect("failed to register IOC tool spec");
    }
}

/// Wire IOC pivot tool executors. Requires PluginDb.
pub fn wire_ioc(executors: &mut ToolExecutorRegistry, plugin_db: Arc<dyn PluginDb>) {
    executors
        .register(Box::new(ioc_pivot::IocListExecutor {
            plugin_db: Arc::clone(&plugin_db),
        }))
        .expect("failed to register re-ioc.list executor");

    executors
        .register(Box::new(ioc_pivot::IocPivotExecutor {
            plugin_db: Arc::clone(&plugin_db),
        }))
        .expect("failed to register re-ioc.pivot executor");

    executors
        .register(Box::new(ioc_pivot::IocSearchExecutor {
            plugin_db,
        }))
        .expect("failed to register re-ioc.search executor");
}

/// Pure phase: register artifact tool specs. Requires DB but not rizin/Ghidra.
pub fn declare_artifact(specs: &mut ToolSpecRegistry) {
    for spec in specs::artifact_specs() {
        specs
            .register(spec)
            .expect("failed to register artifact tool spec");
    }
}

/// Wire artifact tool executors. Requires PluginDb.
pub fn wire_artifact(executors: &mut ToolExecutorRegistry, plugin_db: Arc<dyn PluginDb>) {
    executors
        .register(Box::new(artifact_describe::ArtifactDescribeExecutor {
            plugin_db: Arc::clone(&plugin_db),
        }))
        .expect("failed to register artifact.describe executor");

    executors
        .register(Box::new(artifact_search::ArtifactSearchExecutor {
            plugin_db,
        }))
        .expect("failed to register artifact.search executor");
}

/// Pure phase: register family tool specs. Requires DB but not rizin/Ghidra.
pub fn declare_family(specs: &mut ToolSpecRegistry) {
    for spec in specs::family_specs() {
        specs
            .register(spec)
            .expect("failed to register family tool spec");
    }
}

/// Wire family tool executors. Requires PluginDb.
pub fn wire_family(executors: &mut ToolExecutorRegistry, plugin_db: Arc<dyn PluginDb>) {
    executors
        .register(Box::new(family::FamilyTagExecutor {
            plugin_db: Arc::clone(&plugin_db),
        }))
        .expect("failed to register family.tag executor");

    executors
        .register(Box::new(family::FamilyListExecutor {
            plugin_db: Arc::clone(&plugin_db),
        }))
        .expect("failed to register family.list executor");

    executors
        .register(Box::new(family::FamilySearchExecutor {
            plugin_db: Arc::clone(&plugin_db),
        }))
        .expect("failed to register family.search executor");

    executors
        .register(Box::new(family::FamilyUntagExecutor {
            plugin_db,
        }))
        .expect("failed to register family.untag executor");
}

/// Pure phase: register YARA tool specs.
///
/// If `executor_path` is provided and yara is available, registers OOP executors for
/// yara.scan and yara.generate, plus InProcess executors for yara.test and yara.list.
pub fn declare_yara(specs: &mut ToolSpecRegistry, yara_rules_dir: Option<&std::path::Path>) {
    for spec in specs::yara_specs(yara_rules_dir) {
        specs
            .register(spec)
            .expect("failed to register YARA tool spec");
    }
}

/// Wire YARA tool executors.
///
/// OOP: yara.scan, yara.generate (sandboxed via bwrap)
/// InProcess: yara.test, yara.list (Trusted, need DB + exec)
pub fn wire_yara(
    executors: &mut ToolExecutorRegistry,
    yara_path: &Path,
    yara_rules_dir: Option<&Path>,
    executor_path: Option<&Path>,
    plugin_db: Arc<dyn PluginDb>,
) {
    if let Some(exec_path) = executor_path {
        let mut context_extra = serde_json::json!({
            "yara_path": yara_path.to_string_lossy(),
        });
        if let Some(dir) = yara_rules_dir {
            context_extra["yara_rules_dir"] = serde_json::json!(dir.to_string_lossy());
        }
        let config = SpawnConfig {
            binary_path: exec_path.to_path_buf(),
            protocol_version: 1,
            supported_tools: vec![
                ("yara.scan".into(), 1),
                ("yara.generate".into(), 1),
            ],
            context_extra,
        };
        executors
            .register_oop(config)
            .expect("failed to register YARA OOP executors");
    }

    // InProcess executors (always registered — they need DB access)
    executors
        .register(Box::new(yara_test::YaraTestExecutor {
            plugin_db: Arc::clone(&plugin_db),
            yara_path: yara_path.to_path_buf(),
            yara_rules_dir: yara_rules_dir.map(|p| p.to_path_buf()),
        }))
        .expect("failed to register yara.test executor");

    executors
        .register(Box::new(yara_list::YaraListExecutor {
            plugin_db,
            yara_rules_dir: yara_rules_dir.map(|p| p.to_path_buf()),
        }))
        .expect("failed to register yara.list executor");
}

/// Pure phase: register dedup tool specs. Requires DB but not rizin/Ghidra.
pub fn declare_dedup(specs: &mut ToolSpecRegistry) {
    for spec in specs::dedup_specs() {
        specs
            .register(spec)
            .expect("failed to register dedup tool spec");
    }
}

/// Wire dedup tool executors. Requires PluginDb.
pub fn wire_dedup(executors: &mut ToolExecutorRegistry, plugin_db: Arc<dyn PluginDb>) {
    executors
        .register(Box::new(dedup::DedupPriorAnalysisExecutor {
            plugin_db,
        }))
        .expect("failed to register dedup.prior_analysis executor");
}

/// Pure phase: register transform tool specs. Always available (pure Rust, no external deps).
pub fn declare_transform(specs: &mut ToolSpecRegistry) {
    for spec in specs::transform_specs() {
        specs
            .register(spec)
            .expect("failed to register transform tool spec");
    }
}

/// Wire transform tool executors (OOP only — pure Rust, no InProcess fallback needed).
pub fn wire_transform(executors: &mut ToolExecutorRegistry, executor_path: Option<&Path>) {
    if let Some(exec_path) = executor_path {
        let config = SpawnConfig {
            binary_path: exec_path.to_path_buf(),
            protocol_version: 1,
            supported_tools: vec![
                ("transform.decode".into(), 1),
                ("transform.unpack".into(), 1),
                ("transform.jq".into(), 1),
                ("transform.csv".into(), 1),
                ("transform.convert".into(), 1),
                ("transform.regex".into(), 1),
            ],
            context_extra: serde_json::Value::Null,
        };
        executors
            .register_oop(config)
            .expect("failed to register transform OOP executors");
    }
}

/// Pure phase: register document tool specs. Always available (pure Rust, no external deps).
pub fn declare_doc(specs: &mut ToolSpecRegistry) {
    for spec in specs::doc_specs() {
        specs
            .register(spec)
            .expect("failed to register doc tool spec");
    }
}

/// Wire document tool executors (OOP only — pure Rust, no InProcess fallback needed).
pub fn wire_doc(executors: &mut ToolExecutorRegistry, executor_path: Option<&Path>) {
    if let Some(exec_path) = executor_path {
        let config = SpawnConfig {
            binary_path: exec_path.to_path_buf(),
            protocol_version: 1,
            supported_tools: vec![
                ("doc.parse".into(), 1),
                ("doc.chunk".into(), 1),
                ("doc.ingest".into(), 1),
            ],
            context_extra: serde_json::Value::Null,
        };
        executors
            .register_oop(config)
            .expect("failed to register doc OOP executors");
    }
}

/// RE plugin migrations.
pub fn migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 1,
            name: "create re.iocs table".to_string(),
            sql: include_str!("../../../migrations/001_iocs.sql").to_string(),
        },
        Migration {
            version: 3,
            name: "create re.artifact_families table".to_string(),
            sql: include_str!("../../../migrations/003_artifact_families.sql").to_string(),
        },
        Migration {
            version: 4,
            name: "cross-schema views for public tables".to_string(),
            sql: include_str!("../../../migrations/004_cross_schema_views.sql").to_string(),
        },
        Migration {
            version: 5,
            name: "create re.yara_rules and re.yara_scan_results tables".to_string(),
            sql: include_str!("../../../migrations/005_yara.sql").to_string(),
        },
        Migration {
            version: 6,
            name: "create re.ghidra_function_renames table".to_string(),
            sql: include_str!("../../../migrations/006_ghidra_renames.sql").to_string(),
        },
        Migration {
            version: 7,
            name: "NDA hardening: RLS for ghidra_function_renames and yara_rules".to_string(),
            sql: include_str!("../../../migrations/007_nda_hardening.sql").to_string(),
        },
    ]
}
