use af_plugin_api::{PluginDb, PostToolHook};
use std::sync::Arc;

/// Post-tool hook that extracts and persists IOCs from tool output.
pub struct IocPostToolHook {
    pub plugin_db: Arc<dyn PluginDb>,
}

#[async_trait::async_trait]
impl PostToolHook for IocPostToolHook {
    async fn on_tool_result(
        &self,
        _tool_name: &str,
        output_json: &serde_json::Value,
        project_id: uuid::Uuid,
        user_id: Option<uuid::Uuid>,
    ) -> Result<(), String> {
        let text = serde_json::to_string_pretty(output_json).unwrap_or_default();
        let extractor = crate::ioc_extractor::IocExtractor::new(self.plugin_db.clone());
        let ids = extractor
            .extract_and_store(&text, project_id, None, user_id)
            .await
            .map_err(|e| format!("IOC extraction failed: {e}"))?;

        if !ids.is_empty() {
            let detail = serde_json::json!({
                "project_id": project_id,
                "ioc_count": ids.len(),
            });
            let _ = self.plugin_db.audit_log("ioc_extracted", user_id, Some(&detail)).await;
        }

        Ok(())
    }
}
