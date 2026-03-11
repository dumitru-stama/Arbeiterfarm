use std::collections::HashMap;

/// Renders tool output for terminal display.
pub trait ToolRenderer: Send + Sync {
    fn render(&self, output: &serde_json::Value) -> String;
}

/// Default renderer: pretty-printed JSON.
pub struct DefaultJsonRenderer;

impl ToolRenderer for DefaultJsonRenderer {
    fn render(&self, output: &serde_json::Value) -> String {
        serde_json::to_string_pretty(output).unwrap_or_else(|_| format!("{output:?}"))
    }
}

/// Registry of tool renderers. Falls back to DefaultJsonRenderer.
pub struct ToolRendererRegistry {
    renderers: HashMap<String, Box<dyn ToolRenderer>>,
    default: DefaultJsonRenderer,
}

impl ToolRendererRegistry {
    pub fn new() -> Self {
        Self {
            renderers: HashMap::new(),
            default: DefaultJsonRenderer,
        }
    }

    pub fn register(&mut self, tool_name: &str, renderer: Box<dyn ToolRenderer>) {
        self.renderers.insert(tool_name.to_string(), renderer);
    }

    pub fn get(&self, tool_name: &str) -> &dyn ToolRenderer {
        self.renderers
            .get(tool_name)
            .map(|r| r.as_ref())
            .unwrap_or(&self.default)
    }
}
