use af_llm::CompletionResponse;

/// Parsed response from the LLM.
#[derive(Debug)]
pub enum ParsedResponse {
    /// LLM wants to call a tool (Mode B native or Mode A JSON-block).
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    /// LLM returned a final text response.
    FinalText(String),
}

/// Parse an LLM response, trying Mode B (native tool calls) first, then Mode A (JSON blocks).
pub fn parse_response(response: &CompletionResponse) -> Vec<ParsedResponse> {
    // Mode B: native tool calls from the API
    if !response.tool_calls.is_empty() {
        return response
            .tool_calls
            .iter()
            .map(|tc| ParsedResponse::ToolCall {
                id: tc.id.clone(),
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            })
            .collect();
    }

    // Mode A: parse JSON blocks from content
    let content = &response.content;
    if let Some(parsed) = try_parse_json_block(content) {
        return vec![parsed];
    }

    // Plain text response
    vec![ParsedResponse::FinalText(content.clone())]
}

/// Try to parse a JSON tool_call block from content.
fn try_parse_json_block(content: &str) -> Option<ParsedResponse> {
    // Look for ```json ... ``` blocks first
    if let Some(start) = content.find("```json") {
        let json_start = start + 7;
        if let Some(end) = content[json_start..].find("```") {
            let json_str = content[json_start..json_start + end].trim();
            if let Some(parsed) = try_parse_tool_call_json(json_str) {
                return Some(parsed);
            }
        }
    }

    // Try parsing the whole content as JSON
    if let Some(parsed) = try_parse_tool_call_json(content.trim()) {
        return Some(parsed);
    }

    // Look for { ... } anywhere in the content
    if let Some(start) = content.find('{') {
        if let Some(end) = content.rfind('}') {
            if end > start {
                let json_str = &content[start..=end];
                if let Some(parsed) = try_parse_tool_call_json(json_str) {
                    return Some(parsed);
                }
            }
        }
    }

    None
}

fn try_parse_tool_call_json(json_str: &str) -> Option<ParsedResponse> {
    let val: serde_json::Value = serde_json::from_str(json_str).ok()?;

    // Expected format: {"tool_call": {"name": "...", "arguments": {...}}}
    let tool_call = val.get("tool_call")?;
    let name = tool_call.get("name")?.as_str()?.to_string();
    let arguments = tool_call
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::Value::Object(Default::default()));

    Some(ParsedResponse::ToolCall {
        id: format!("json-block-{}", uuid::Uuid::new_v4()),
        name,
        arguments,
    })
}
