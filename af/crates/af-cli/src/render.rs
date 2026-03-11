use af_core::ToolRenderer;

/// Register built-in renderers into the registry.
pub fn register_builtin(registry: &mut af_core::ToolRendererRegistry) {
    registry.register("file.info", Box::new(FileInfoRenderer));
    registry.register("vt.file_report", Box::new(VtReportRenderer));
}

pub struct FileInfoRenderer;

impl ToolRenderer for FileInfoRenderer {
    fn render(&self, output: &serde_json::Value) -> String {
        render_file_info(output)
    }
}

pub struct VtReportRenderer;

impl ToolRenderer for VtReportRenderer {
    fn render(&self, output: &serde_json::Value) -> String {
        render_vt_report(output)
    }
}

fn render_file_info(output: &serde_json::Value) -> String {
    format!(
        "File: {}\nSize: {} bytes\nType: {}\nMD5:  {}\nSHA256: {}",
        output["filename"].as_str().unwrap_or("?"),
        output["size_bytes"].as_u64().unwrap_or(0),
        output["magic_type"].as_str().unwrap_or("unknown"),
        output["md5"].as_str().unwrap_or("?"),
        output["sha256"].as_str().unwrap_or("?"),
    )
}

fn render_vt_report(output: &serde_json::Value) -> String {
    let positives = output["positives"].as_u64().unwrap_or(0);
    let total = output["total"].as_u64().unwrap_or(0);

    let tags = output["tags"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let families = output["top_families"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    format!(
        "Detection: {}/{}\nFirst seen: {}\nTags: {}\nFamilies: {}",
        positives,
        total,
        output["first_seen"].as_str().unwrap_or("unknown"),
        if tags.is_empty() { "-" } else { &tags },
        if families.is_empty() { "-" } else { &families },
    )
}
