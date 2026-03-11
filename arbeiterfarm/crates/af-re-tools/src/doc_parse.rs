//! `doc.parse` — Extract readable text from document artifacts.
//!
//! Supports: PDF, HTML, Markdown, DOCX, XLSX, EPUB, plain text.
//! Auto-detects format from magic bytes + extension.

use af_builtin_tools::envelope::{OopArtifact, OopResult, ProducedFile};
use serde_json::{json, Value};
use std::path::Path;

/// Max extracted text size: 64 MB.
const MAX_TEXT_SIZE: usize = 64 * 1024 * 1024;

pub fn execute(artifact: &OopArtifact, input: &Value, scratch_dir: &Path) -> OopResult {
    let data = match std::fs::read(&artifact.storage_path) {
        Ok(d) => d,
        Err(e) => {
            return OopResult::Error {
                code: "read_error".into(),
                message: format!("failed to read artifact: {e}"),
                retryable: false,
            }
        }
    };

    let format_override = input.get("format").and_then(|v| v.as_str());
    let pages = input.get("pages").and_then(|v| v.as_str());

    let format = match format_override {
        Some(f) => f.to_string(),
        None => detect_format(&data, &artifact.filename),
    };

    let result = match format.as_str() {
        "pdf" => extract_pdf(&data, pages),
        "html" => extract_html(&data),
        "markdown" => extract_markdown(&data),
        "docx" => extract_docx(&data),
        "xlsx" => extract_xlsx(&artifact.storage_path),
        "epub" => extract_epub(&data),
        "text" | "csv" | "json" | "yaml" | "toml" | "xml" => extract_text(&data),
        other => Err(format!("unsupported format: {other}")),
    };

    let text = match result {
        Ok(t) => t,
        Err(e) => {
            return OopResult::Error {
                code: "parse_error".into(),
                message: e,
                retryable: false,
            }
        }
    };

    if text.len() > MAX_TEXT_SIZE {
        return OopResult::Error {
            code: "output_too_large".into(),
            message: format!(
                "extracted text is {} bytes, exceeds {} MB limit",
                text.len(),
                MAX_TEXT_SIZE / (1024 * 1024)
            ),
            retryable: false,
        };
    }

    let char_count = text.chars().count();
    let word_count = text.split_whitespace().count();
    let preview: String = text.chars().take(500).collect();
    let preview = if char_count > 500 {
        format!("{preview}...")
    } else {
        preview
    };

    let out_path = scratch_dir.join("parsed_text.txt");
    if let Err(e) = std::fs::write(&out_path, &text) {
        return OopResult::Error {
            code: "write_error".into(),
            message: format!("failed to write parsed text: {e}"),
            retryable: false,
        };
    }

    OopResult::Ok {
        output: json!({
            "format": format,
            "char_count": char_count,
            "word_count": word_count,
            "preview": preview,
            "hint": "Full extracted text stored as artifact. Use file.read_range to inspect.",
        }),
        produced_files: vec![ProducedFile {
            filename: "parsed_text.txt".into(),
            path: out_path,
            mime_type: Some("text/plain".into()),
            description: Some(format!(
                "Extracted text from {format}: {char_count} chars, {word_count} words"
            )),
        }],
    }
}

/// Inner function for use by doc_ingest — returns the extracted text directly.
pub fn execute_inner(
    data: &[u8],
    filename: &str,
    format_override: Option<&str>,
    pages: Option<&str>,
) -> Result<(String, String), String> {
    let format = match format_override {
        Some(f) => f.to_string(),
        None => detect_format(data, filename),
    };

    let text = match format.as_str() {
        "pdf" => extract_pdf(data, pages)?,
        "html" => extract_html(data)?,
        "markdown" => extract_markdown(data)?,
        "docx" => extract_docx(data)?,
        // xlsx needs a file path — caller should handle this case
        "xlsx" => return Err("xlsx requires file path; use execute() directly".into()),
        "epub" => extract_epub(data)?,
        "text" | "csv" | "json" | "yaml" | "toml" | "xml" => extract_text(data)?,
        other => return Err(format!("unsupported format: {other}")),
    };

    if text.len() > MAX_TEXT_SIZE {
        return Err(format!(
            "extracted text is {} bytes, exceeds {} MB limit",
            text.len(),
            MAX_TEXT_SIZE / (1024 * 1024)
        ));
    }

    Ok((text, format))
}

/// Inner function for xlsx that needs a path.
pub fn execute_inner_xlsx(path: &Path) -> Result<(String, String), String> {
    let text = extract_xlsx(path)?;
    if text.len() > MAX_TEXT_SIZE {
        return Err(format!(
            "extracted text is {} bytes, exceeds {} MB limit",
            text.len(),
            MAX_TEXT_SIZE / (1024 * 1024)
        ));
    }
    Ok((text, "xlsx".to_string()))
}

/// Detect document format from magic bytes + file extension.
pub fn detect_format(data: &[u8], filename: &str) -> String {
    // Magic bytes first
    if data.len() >= 5 && &data[..5] == b"%PDF-" {
        return "pdf".into();
    }
    if data.len() >= 4 && data[0] == 0x50 && data[1] == 0x4B && data[2] == 0x03 && data[3] == 0x04
    {
        // ZIP-based: check internal structure
        if let Ok(mut zip_archive) = zip::ZipArchive::new(std::io::Cursor::new(data)) {
            for i in 0..zip_archive.len().min(20) {
                if let Ok(entry) = zip_archive.by_index_raw(i) {
                    let name = entry.name().to_lowercase();
                    if name == "word/document.xml" || name.starts_with("word/") {
                        return "docx".into();
                    }
                    if name.starts_with("xl/") {
                        return "xlsx".into();
                    }
                    if name == "meta-inf/container.xml" {
                        return "epub".into();
                    }
                }
            }
        }
        // Generic ZIP — treat as unknown, user should specify format
        return "text".into();
    }

    // Extension-based fallback
    let lower = filename.to_lowercase();
    if lower.ends_with(".pdf") {
        return "pdf".into();
    }
    if lower.ends_with(".html") || lower.ends_with(".htm") || lower.ends_with(".xhtml") {
        return "html".into();
    }
    if lower.ends_with(".md") || lower.ends_with(".markdown") {
        return "markdown".into();
    }
    if lower.ends_with(".docx") {
        return "docx".into();
    }
    if lower.ends_with(".xlsx") || lower.ends_with(".xls") {
        return "xlsx".into();
    }
    if lower.ends_with(".epub") {
        return "epub".into();
    }
    if lower.ends_with(".csv") {
        return "csv".into();
    }
    if lower.ends_with(".json") {
        return "json".into();
    }
    if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        return "yaml".into();
    }
    if lower.ends_with(".toml") {
        return "toml".into();
    }
    if lower.ends_with(".xml") {
        return "xml".into();
    }

    // Try UTF-8
    if std::str::from_utf8(data).is_ok() {
        return "text".into();
    }

    "text".into()
}

fn extract_text(data: &[u8]) -> Result<String, String> {
    Ok(String::from_utf8_lossy(data).into_owned())
}

fn extract_pdf(data: &[u8], pages: Option<&str>) -> Result<String, String> {
    let full_text =
        pdf_extract::extract_text_from_mem(data).map_err(|e| format!("PDF extraction error: {e}"))?;

    if let Some(page_range) = pages {
        let page_texts: Vec<&str> = full_text.split('\x0C').collect();
        let selected = select_pages(&page_texts, page_range)?;
        Ok(selected.join("\n\n"))
    } else {
        Ok(full_text)
    }
}

fn extract_html(data: &[u8]) -> Result<String, String> {
    html2text::from_read(data, usize::MAX)
        .map_err(|e| format!("HTML extraction error: {e}"))
}

fn extract_markdown(data: &[u8]) -> Result<String, String> {
    let source = String::from_utf8_lossy(data);
    let parser = pulldown_cmark::Parser::new(&source);

    let mut text = String::new();
    for event in parser {
        match event {
            pulldown_cmark::Event::Text(t) => text.push_str(&t),
            pulldown_cmark::Event::Code(c) => {
                text.push('`');
                text.push_str(&c);
                text.push('`');
            }
            pulldown_cmark::Event::SoftBreak | pulldown_cmark::Event::HardBreak => {
                text.push('\n');
            }
            pulldown_cmark::Event::End(_) => {
                if !text.ends_with('\n') {
                    text.push('\n');
                }
            }
            _ => {}
        }
    }

    Ok(text)
}

fn extract_docx(data: &[u8]) -> Result<String, String> {
    let cursor = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("DOCX zip error: {e}"))?;

    let mut xml_data = Vec::new();
    {
        let doc = archive
            .by_name("word/document.xml")
            .map_err(|e| format!("DOCX missing word/document.xml: {e}"))?;
        // Bounded read: prevent decompression bombs
        let mut limited = std::io::Read::take(doc, MAX_TEXT_SIZE as u64 + 1);
        std::io::Read::read_to_end(&mut limited, &mut xml_data)
            .map_err(|e| format!("DOCX read error: {e}"))?;
    }

    if xml_data.len() > MAX_TEXT_SIZE {
        return Err(format!("DOCX XML too large: {} bytes (exceeds {} MB limit)", xml_data.len(), MAX_TEXT_SIZE / (1024 * 1024)));
    }

    // Parse XML and extract <w:t> text nodes
    let mut text = String::new();
    let mut reader = quick_xml::Reader::from_reader(xml_data.as_slice());
    let mut in_text_node = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(ref e))
            | Ok(quick_xml::events::Event::Empty(ref e)) => {
                let local_name = e.local_name();
                let local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                if local == "t" {
                    in_text_node = true;
                } else if local == "p" && !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                } else if local == "br" {
                    text.push('\n');
                } else if local == "tab" {
                    text.push('\t');
                }
            }
            Ok(quick_xml::events::Event::Text(ref e)) => {
                if in_text_node {
                    let decoded = e.unescape().unwrap_or_default();
                    text.push_str(&decoded);
                }
            }
            Ok(quick_xml::events::Event::End(ref e)) => {
                let local_name = e.local_name();
                let local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                if local == "t" {
                    in_text_node = false;
                } else if local == "p" {
                    text.push('\n');
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => return Err(format!("DOCX XML parse error: {e}")),
            _ => {}
        }
        buf.clear();
    }

    Ok(text)
}

fn extract_xlsx(path: &Path) -> Result<String, String> {
    use calamine::{open_workbook_auto, Data, Reader};

    let mut workbook = open_workbook_auto(path).map_err(|e| format!("XLSX open error: {e}"))?;

    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
    let mut output = String::new();

    for (idx, name) in sheet_names.iter().enumerate() {
        if idx > 0 {
            output.push_str("\n\n");
        }
        output.push_str(&format!("=== Sheet: {} ===\n", name));

        if let Ok(range) = workbook.worksheet_range(name) {
            for row in range.rows() {
                let cells: Vec<String> = row
                    .iter()
                    .map(|cell| match cell {
                        Data::Empty => String::new(),
                        Data::String(s) => s.clone(),
                        Data::Float(f) => {
                            if *f == (*f as i64) as f64 {
                                format!("{}", *f as i64)
                            } else {
                                format!("{f}")
                            }
                        }
                        Data::Int(i) => format!("{i}"),
                        Data::Bool(b) => format!("{b}"),
                        Data::Error(e) => format!("#ERR:{e:?}"),
                        Data::DateTime(dt) => format!("{dt}"),
                        Data::DateTimeIso(s) => s.clone(),
                        Data::DurationIso(s) => s.clone(),
                    })
                    .collect();
                output.push_str(&cells.join("\t"));
                output.push('\n');
            }
        }
    }

    Ok(output)
}

fn extract_epub(data: &[u8]) -> Result<String, String> {
    let cursor = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("EPUB zip error: {e}"))?;

    // Metadata XML cap: 1 MB is plenty for container.xml / content.opf
    const MAX_META_SIZE: u64 = 1024 * 1024;

    // 1. Parse META-INF/container.xml to find content.opf path
    let opf_path = {
        let mut container_data = Vec::new();
        {
            let container = archive
                .by_name("META-INF/container.xml")
                .map_err(|e| format!("EPUB missing container.xml: {e}"))?;
            let mut limited = std::io::Read::take(container, MAX_META_SIZE + 1);
            std::io::Read::read_to_end(&mut limited, &mut container_data)
                .map_err(|e| format!("EPUB container read error: {e}"))?;
        }
        if container_data.len() as u64 > MAX_META_SIZE {
            return Err("EPUB container.xml too large".into());
        }
        parse_opf_path(&container_data)?
    };

    // 2. Parse content.opf to get spine order
    let opf_dir = Path::new(&opf_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let spine_hrefs = {
        let mut opf_data = Vec::new();
        {
            let opf = archive
                .by_name(&opf_path)
                .map_err(|e| format!("EPUB missing {opf_path}: {e}"))?;
            let mut limited = std::io::Read::take(opf, MAX_META_SIZE + 1);
            std::io::Read::read_to_end(&mut limited, &mut opf_data)
                .map_err(|e| format!("EPUB opf read error: {e}"))?;
        }
        if opf_data.len() as u64 > MAX_META_SIZE {
            return Err("EPUB content.opf too large".into());
        }
        parse_spine_hrefs(&opf_data)?
    };

    // 3. Extract text from each spine chapter (bounded reads)
    let mut all_text = String::new();
    for href in &spine_hrefs {
        let full_path = if opf_dir.is_empty() {
            href.clone()
        } else {
            format!("{}/{}", opf_dir, href)
        };

        let mut chapter_data = Vec::new();
        if let Ok(entry) = archive.by_name(&full_path) {
            let mut limited = std::io::Read::take(entry, MAX_TEXT_SIZE as u64 + 1);
            if std::io::Read::read_to_end(&mut limited, &mut chapter_data).is_ok() {
                if chapter_data.len() <= MAX_TEXT_SIZE {
                    if let Ok(chapter_text) =
                        html2text::from_read(chapter_data.as_slice(), usize::MAX)
                    {
                        if !all_text.is_empty() {
                            all_text.push_str("\n\n");
                        }
                        all_text.push_str(&chapter_text);
                    }
                }
            }
        }

        if all_text.len() > MAX_TEXT_SIZE {
            break;
        }
    }

    Ok(all_text)
}

/// Parse META-INF/container.xml to extract the content.opf path.
fn parse_opf_path(data: &[u8]) -> Result<String, String> {
    let mut reader = quick_xml::Reader::from_reader(data);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Empty(ref e))
            | Ok(quick_xml::events::Event::Start(ref e)) => {
                let local_name = e.local_name();
                let local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                if local == "rootfile" {
                    for attr in e.attributes().flatten() {
                        let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                        if key == "full-path" {
                            let val = attr.unescape_value().unwrap_or_default();
                            return Ok(val.to_string());
                        }
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => return Err(format!("container.xml parse error: {e}")),
            _ => {}
        }
        buf.clear();
    }

    Err("could not find rootfile in container.xml".into())
}

/// Parse content.opf to extract spine item hrefs in order.
fn parse_spine_hrefs(data: &[u8]) -> Result<Vec<String>, String> {
    let mut reader = quick_xml::Reader::from_reader(data);
    let mut buf = Vec::new();

    let mut manifest: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut spine_ids: Vec<String> = Vec::new();
    let mut in_manifest = false;
    let mut in_spine = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(ref e)) => {
                let local_name = e.local_name();
                let local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                if local == "manifest" {
                    in_manifest = true;
                } else if local == "spine" {
                    in_spine = true;
                } else if local == "item" && in_manifest {
                    collect_manifest_item(e, &mut manifest);
                } else if local == "itemref" && in_spine {
                    collect_spine_id(e, &mut spine_ids);
                }
            }
            Ok(quick_xml::events::Event::Empty(ref e)) => {
                let local_name = e.local_name();
                let local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                if local == "item" && in_manifest {
                    collect_manifest_item(e, &mut manifest);
                } else if local == "itemref" && in_spine {
                    collect_spine_id(e, &mut spine_ids);
                }
            }
            Ok(quick_xml::events::Event::End(ref e)) => {
                let local_name = e.local_name();
                let local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                if local == "manifest" {
                    in_manifest = false;
                } else if local == "spine" {
                    in_spine = false;
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => return Err(format!("content.opf parse error: {e}")),
            _ => {}
        }
        buf.clear();
    }

    let hrefs: Vec<String> = spine_ids
        .iter()
        .filter_map(|id| manifest.get(id).cloned())
        .collect();

    if hrefs.is_empty() {
        return Err("no spine items found in content.opf".into());
    }

    Ok(hrefs)
}

fn collect_manifest_item(
    e: &quick_xml::events::BytesStart<'_>,
    manifest: &mut std::collections::HashMap<String, String>,
) {
    let mut id = String::new();
    let mut href = String::new();
    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
        let val = attr.unescape_value().unwrap_or_default();
        if key == "id" {
            id = val.to_string();
        } else if key == "href" {
            href = val.to_string();
        }
    }
    if !id.is_empty() && !href.is_empty() {
        manifest.insert(id, href);
    }
}

fn collect_spine_id(
    e: &quick_xml::events::BytesStart<'_>,
    spine_ids: &mut Vec<String>,
) {
    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
        if key == "idref" {
            let val = attr.unescape_value().unwrap_or_default();
            spine_ids.push(val.to_string());
        }
    }
}

/// Parse a page range string like "1-10", "5", "1,3,7-9" into selected pages.
fn select_pages<'a>(pages: &'a [&'a str], range: &str) -> Result<Vec<&'a str>, String> {
    let total = pages.len();
    let mut selected_indices = std::collections::BTreeSet::new();

    for part in range.split(',') {
        let part = part.trim();
        if part.contains('-') {
            let mut parts = part.splitn(2, '-');
            let start: usize = parts
                .next()
                .unwrap_or("1")
                .trim()
                .parse()
                .map_err(|_| format!("invalid page number in range: {part}"))?;
            let end: usize = parts
                .next()
                .unwrap_or("1")
                .trim()
                .parse()
                .map_err(|_| format!("invalid page number in range: {part}"))?;
            if start == 0 || end == 0 {
                return Err("page numbers are 1-based".into());
            }
            for i in start..=end.min(total) {
                selected_indices.insert(i - 1);
            }
        } else {
            let page: usize = part
                .parse()
                .map_err(|_| format!("invalid page number: {part}"))?;
            if page == 0 {
                return Err("page numbers are 1-based".into());
            }
            if page <= total {
                selected_indices.insert(page - 1);
            }
        }
    }

    Ok(selected_indices
        .iter()
        .filter_map(|&i| pages.get(i).copied())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_format_pdf() {
        let data = b"%PDF-1.4 fake pdf";
        assert_eq!(detect_format(data, "test.bin"), "pdf");
    }

    #[test]
    fn test_detect_format_by_extension() {
        let data = b"hello world";
        assert_eq!(detect_format(data, "report.html"), "html");
        assert_eq!(detect_format(data, "notes.md"), "markdown");
        assert_eq!(detect_format(data, "data.csv"), "csv");
        assert_eq!(detect_format(data, "config.yaml"), "yaml");
        assert_eq!(detect_format(data, "config.toml"), "toml");
    }

    #[test]
    fn test_extract_text() {
        let data = b"Hello, world!";
        let result = extract_text(data).unwrap();
        assert_eq!(result, "Hello, world!");
    }

    #[test]
    fn test_extract_markdown() {
        let data = b"# Title\n\nSome **bold** text.\n\n- item 1\n- item 2";
        let result = extract_markdown(data).unwrap();
        assert!(result.contains("Title"));
        assert!(result.contains("bold"));
        assert!(result.contains("item 1"));
    }

    #[test]
    fn test_extract_html() {
        let data = b"<html><body><h1>Title</h1><p>Some text.</p></body></html>";
        let result = extract_html(data).unwrap();
        assert!(result.contains("Title"));
        assert!(result.contains("Some text"));
    }

    #[test]
    fn test_select_pages() {
        let pages = vec!["page1", "page2", "page3", "page4", "page5"];
        let selected = select_pages(&pages, "1-3").unwrap();
        assert_eq!(selected, vec!["page1", "page2", "page3"]);

        let selected = select_pages(&pages, "2,4").unwrap();
        assert_eq!(selected, vec!["page2", "page4"]);

        let selected = select_pages(&pages, "1,3-5").unwrap();
        assert_eq!(selected, vec!["page1", "page3", "page4", "page5"]);
    }

    #[test]
    fn test_select_pages_out_of_range() {
        let pages = vec!["page1", "page2"];
        let selected = select_pages(&pages, "1-10").unwrap();
        assert_eq!(selected, vec!["page1", "page2"]);
    }
}
