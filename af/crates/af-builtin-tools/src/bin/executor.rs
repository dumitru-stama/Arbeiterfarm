use af_builtin_tools::envelope::{
    HandshakeResponse, OopEnvelope, OopResponse, OopResult, SupportedTool,
};
use std::io::{self, Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--handshake") {
        let handshake = HandshakeResponse {
            protocol_version: 1,
            supported_tools: vec![
                SupportedTool {
                    name: "file.info".into(),
                    version: 1,
                },
                SupportedTool {
                    name: "file.read_range".into(),
                    version: 1,
                },
                SupportedTool {
                    name: "file.strings".into(),
                    version: 1,
                },
                SupportedTool {
                    name: "file.hexdump".into(),
                    version: 1,
                },
                SupportedTool {
                    name: "file.grep".into(),
                    version: 1,
                },
            ],
        };
        let json = serde_json::to_string_pretty(&handshake).unwrap();
        println!("{json}");
        return;
    }

    // Read envelope from stdin
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        let resp = OopResponse {
            result: OopResult::Error {
                code: "stdin_error".into(),
                message: format!("failed to read stdin: {e}"),
                retryable: false,
            },
        };
        write_response(&resp);
        return;
    }

    let envelope: OopEnvelope = match serde_json::from_str(&input) {
        Ok(e) => e,
        Err(e) => {
            let resp = OopResponse {
                result: OopResult::Error {
                    code: "parse_error".into(),
                    message: format!("failed to parse envelope: {e}"),
                    retryable: false,
                },
            };
            write_response(&resp);
            return;
        }
    };

    // Get first artifact (all file tools need exactly one)
    let artifact = match envelope.context.artifacts.first() {
        Some(a) => a,
        None => {
            let resp = OopResponse {
                result: OopResult::Error {
                    code: "no_artifact".into(),
                    message: "no artifacts provided in context".into(),
                    retryable: false,
                },
            };
            write_response(&resp);
            return;
        }
    };

    let result = match envelope.tool_name.as_str() {
        "file.info" => af_builtin_tools::file_info::execute(artifact),
        "file.read_range" => af_builtin_tools::file_read_range::execute(artifact, &envelope.input),
        "file.strings" => af_builtin_tools::file_strings::execute(artifact, &envelope.input, &envelope.context.scratch_dir),
        "file.hexdump" => af_builtin_tools::file_hexdump::execute(artifact, &envelope.input),
        "file.grep" => af_builtin_tools::file_grep::execute(artifact, &envelope.input, &envelope.context.scratch_dir),
        other => OopResult::Error {
            code: "unknown_tool".into(),
            message: format!("unknown tool: {other}"),
            retryable: false,
        },
    };

    write_response(&OopResponse { result });
}

fn write_response(resp: &OopResponse) {
    let json = serde_json::to_string(resp).unwrap();
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let _ = handle.write_all(json.as_bytes());
    let _ = handle.write_all(b"\n");
    let _ = handle.flush();
}
