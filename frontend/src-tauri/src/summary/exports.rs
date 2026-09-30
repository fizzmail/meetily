/// Advanced export functionality for AI meeting summaries.
///
/// The AI summary is stored as Blocknote JSON (a JSON array of Blocknote block
/// objects). This module converts that JSON into Markdown, and renders the
/// Markdown into PDF and DOCX files. It also exposes Tauri commands that let the
/// frontend pick a save location via the native dialog and write the file.
///
/// This module is purely additive: it introduces new commands and new
/// dependencies without modifying any existing command signatures.

use serde_json::Value;
use tauri::Runtime;
use tracing::{info, warn};

// ============================================================================
// Blocknote JSON -> Markdown conversion
// ============================================================================

/// Converts a Blocknote JSON document (a JSON array of block objects) into a
/// Markdown string.
///
/// Handles: heading (h1-h6), paragraph, bullet-list, numbered-list, code,
/// quote, and divider blocks. `content` may be a plain string or an array of
/// rich-text objects; rich-text bold / italic / code / link attributes are
/// rendered as Markdown.
///
/// # Errors
/// Returns `Err` if the input is not valid JSON or is not a JSON array.
pub fn blocknote_json_to_markdown(json: &str) -> Result<String, String> {
    let value: Value =
        serde_json::from_str(json).map_err(|e| format!("Invalid JSON: {}", e))?;

    let blocks = value
        .as_array()
        .ok_or_else(|| "Summary JSON must be an array of blocks".to_string())?;

    let mut out = String::new();
    for block in blocks {
        render_block(block, &mut out);
    }
    Ok(out)
}

/// Renders a single Blocknote block into Markdown, appending to `out`.
fn render_block(block: &Value, out: &mut String) {
    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let content = block.get("content");

    match block_type {
        "heading" => {
            let level = heading_level(block);
            let hashes: String = "#".repeat(level);
            let text = render_rich_text(content.unwrap_or(&Value::Null));
            out.push_str(&format!("{} {}\n", hashes, text));
        }
        "paragraph" => {
            let text = render_rich_text(content.unwrap_or(&Value::Null));
            out.push_str(&format!("{}\n", text));
        }
        "quote" => {
            let text = render_rich_text(content.unwrap_or(&Value::Null));
            out.push_str(&format!("> {}\n", text));
        }
        "divider" => {
            out.push_str("---\n");
        }
        "code" => {
            let code = match content {
                Some(Value::String(s)) => s.clone(),
                Some(other) => render_rich_text(other),
                None => String::new(),
            };
            let language = block
                .get("language")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let fence = if language.is_empty() {
                "```".to_string()
            } else {
                format!("```{}", language)
            };
            out.push_str(&format!("{}\n{}\n```\n", fence, code));
        }
        "bullet-list" => {
            render_list(block, false, 0, out);
        }
        "numbered-list" => {
            render_list(block, true, 0, out);
        }
        _ => {
            // Unknown block type: best-effort render of its content, if any.
            if let Some(c) = content {
                let text = render_rich_text(c);
                if !text.is_empty() {
                    out.push_str(&format!("{}\n", text));
                }
            }
        }
    }
}

/// Renders a (possibly nested) list block into Markdown.
///
/// `numbered` selects the `"1. "` prefix (vs `"- "`). `indent` is the nesting
/// depth, rendered as two spaces per level. Nested lists are found inside each
/// list item's `blocks` array.
fn render_list(block: &Value, numbered: bool, indent: usize, out: &mut String) {
    let items = match block.get("blocks").and_then(|v| v.as_array()) {
        Some(items) => items,
        None => return,
    };

    let indent_str: String = "  ".repeat(indent);
    let mut index = 1usize;
    for item in items {
        let prefix = if numbered {
            format!("{}. ", index)
        } else {
            "- ".to_string()
        };
        let text = render_rich_text(item.get("content").unwrap_or(&Value::Null));
        if !text.is_empty() {
            out.push_str(&format!("{}{}{}\n", indent_str, prefix, text));
        }

        // Recurse into nested lists (a list item's `blocks` are nested list blocks).
        if let Some(nested) = item.get("blocks").and_then(|v| v.as_array()) {
            for sub in nested {
                let sub_type = sub.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if sub_type == "bullet-list" || sub_type == "numbered-list" {
                    render_list(sub, sub_type == "numbered-list", indent + 1, out);
                }
            }
        }

        index += 1;
    }
}

/// Renders a Blocknote `content` value (a plain string or an array of rich-text
/// objects) into a Markdown string.
fn render_rich_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                let text = item
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let mut s = text.to_string();

                // code (innermost)
                if item.get("code").and_then(|v| v.as_bool()).unwrap_or(false) {
                    s = format!("`{}`", s);
                }
                // bold
                if item.get("bold").and_then(|v| v.as_bool()).unwrap_or(false) {
                    s = format!("**{}**", s);
                }
                // italic
                if item
                    .get("italic")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    s = format!("*{}*", s);
                }
                // link (outermost)
                if let Some(url) = item.get("url").and_then(|v| v.as_str()) {
                    if !url.is_empty() {
                        s = format!("[{}]({})", s, url);
                    }
                }

                out.push_str(&s);
            }
            out
        }
        _ => String::new(),
    }
}

/// Determines the heading level (1-6) from a Blocknote heading block.
fn heading_level(block: &Value) -> usize {
    match block.get("level") {
        Some(v) => {
            if let Some(n) = v.as_u64() {
                return clamp_level(n as usize);
            }
            // Tolerate a "h1".."h6" style string, just in case.
            if let Some(s) = v.as_str() {
                let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = digits.parse::<usize>() {
                    return clamp_level(n);
                }
            }
            1
        }
        None => 1,
    }
}

fn clamp_level(n: usize) -> usize {
    if (1..=6).contains(&n) {
        n
    } else {
        1
    }
}

// ============================================================================
// Markdown -> plain text (symbol stripping) for PDF / DOCX
// ============================================================================

/// Strips leading Markdown block symbols from a single line, keeping the words.
///
/// Removes leading hashes (headings), `>` (quotes), `-`/`*` (bullets), numbered
/// list markers (`1. `), and code-fence lines. Also removes inline backticks.
fn strip_markdown_line(line: &str) -> String {
    let trimmed = line.trim_start();

    // Skip code-fence delimiter lines (``` or ```lang).
    if trimmed.starts_with("```") {
        return String::new();
    }

    let mut s = trimmed;
    loop {
        let t = s.trim_start();
        let next = t
            .strip_prefix('#')
            .or_else(|| t.strip_prefix('>'))
            .or_else(|| t.strip_prefix("- "))
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix('-'))
            .or_else(|| t.strip_prefix('*'))
            .or_else(|| strip_numbered_prefix(t));
        match next {
            Some(rest) => s = rest,
            None => break,
        }
    }

    // Remove inline code backticks.
    s.replace('`', "")
}

/// Strips a leading numbered-list marker such as `"1. "` (requires a space after
/// the dot to avoid false positives like `"1.5"`).
fn strip_numbered_prefix(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i < bytes.len() && bytes[i] == b'.' {
        let after = i + 1;
        if after >= bytes.len() || bytes[after] == b' ' {
            return Some(&s[after..]);
        }
    }
    None
}

// ============================================================================
// File helpers
// ============================================================================

/// Writes `content` to `path`, creating the parent directory if it is missing.
///
/// Mirrors the existing `save_transcript` pattern.
pub fn save_markdown_file(path: &str, content: &str) -> Result<(), String> {
    ensure_parent_dir(path)?;
    std::fs::write(path, content)
        .map_err(|e| format!("Failed to write file: {}", e))?;
    Ok(())
}

/// Creates the parent directory of `path` if it does not already exist.
fn ensure_parent_dir(path: &str) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        }
    }
    Ok(())
}

// ============================================================================
// PDF export (printpdf)
// ============================================================================

/// Renders `markdown` into a simple, readable single-column PDF at `path`.
///
/// The title is drawn as a larger line at the top; the body is rendered as
/// plain text (Markdown symbols stripped) at 11pt with 2cm margins and automatic
/// page breaks via printpdf's text layout.
pub fn build_pdf(path: &str, markdown: &str, title: &str) -> Result<(), String> {
    use printpdf::{Font, Pdf, Point, Text, TextLayout};

    let font = Font::new().map_err(|e| format!("Failed to load PDF font: {}", e))?;

    let mut pdf = Pdf::new("A4", "Meetily", "Meeting Summary", "Exported summary");

    // Page geometry (points). 1 cm = 28.3465 pt, so a 2 cm margin = 56.69 pt.
    let page_w = 595.28;
    let page_h = 841.89;
    let margin = 56.69;
    let left = margin;
    let top = page_h - margin;
    let text_width = page_w - 2.0 * margin;

    // Title (larger; the default font has no bold variant, so use a larger size).
    let title_text = Text::new(
        title.to_string(),
        Point::new(left, top),
        16.0,
        font.clone(),
    );
    pdf.add_text(title_text)
        .map_err(|e| format!("Failed to add PDF title: {}", e))?;

    // Body via TextLayout for automatic page breaks.
    let body_start_y = top - 40.0;
    let first_page_height = body_start_y - margin;
    let mut layout = TextLayout::new(
        &pdf,
        11.0,
        font.clone(),
        Point::new(left, body_start_y),
        text_width,
        first_page_height,
    );
    for line in markdown.lines() {
        layout.add_paragraph(strip_markdown_line(line));
    }
    pdf.add_text_layout(&layout)
        .map_err(|e| format!("Failed to add PDF body: {}", e))?;

    ensure_parent_dir(path)?;
    pdf.save(path).map_err(|e| format!("Failed to save PDF: {}", e))?;
    Ok(())
}

// ============================================================================
// DOCX export (docx-rs)
// ============================================================================

/// Renders `markdown` into a simple, readable DOCX document at `path`.
///
/// The title is a bold, larger paragraph; the body is rendered as plain-text
/// paragraphs (Markdown symbols stripped).
pub fn build_docx(path: &str, markdown: &str, title: &str) -> Result<(), String> {
    use docx::paragraph::Paragraph;
    use docx::text_run::TextRun;
    use docx::Document;

    let mut doc = Document::new();

    // Title paragraph (bold, larger font: 32 half-points = 16pt).
    let title_run = TextRun::new(title.to_string()).bold().size(32);
    doc.add_paragraph(Paragraph::new(vec![title_run]));

    // Body paragraphs (11pt = 22 half-points).
    for line in markdown.lines() {
        let run = TextRun::new(strip_markdown_line(line));
        doc.add_paragraph(Paragraph::new(vec![run]));
    }

    ensure_parent_dir(path)?;
    doc.save(path).map_err(|e| format!("Failed to save DOCX: {}", e))?;
    Ok(())
}

// ============================================================================
// Tauri commands
// ============================================================================

/// Picks a save location via the native dialog and returns the chosen path,
/// ensuring it carries the expected file extension.
async fn pick_save_path<R: Runtime>(
    app: &tauri::AppHandle<R>,
    default_name: &str,
    extension: &str,
) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    // The dialog does not let us preset the default filename, so we fall back to
    // appending the extension to whatever path the user chooses. `default_name`
    // documents the intended default (derived from the meeting id).
    let _ = default_name;

    let chosen = app
        .dialog()
        .file()
        .set_title("Save Summary")
        .save()
        .await
        .ok()
        .flatten()
        .ok_or_else(|| "No save path selected".to_string())?;

    Ok(ensure_extension(chosen, extension))
}

/// Returns `path` with `extension` appended if it does not already end in it.
fn ensure_extension(path: std::path::PathBuf, extension: &str) -> String {
    let has_ext = path
        .extension()
        .map(|e| e.to_string_lossy().eq_ignore_ascii_case(extension))
        .unwrap_or(false);
    if has_ext {
        path.to_string_lossy().to_string()
    } else {
        format!("{}.{}", path.to_string_lossy(), extension)
    }
}

/// Converts the Blocknote JSON summary to Markdown, lets the user pick a save
/// location, writes the file, and returns the saved path.
#[tauri::command]
pub async fn api_export_summary_markdown<R: Runtime>(
    app: tauri::AppHandle<R>,
    meeting_id: String,
    content_json: String,
) -> Result<String, String> {
    info!("api_export_summary_markdown called for meeting_id: {}", meeting_id);

    let markdown = match blocknote_json_to_markdown(&content_json) {
        Ok(m) => m,
        Err(e) => {
            warn!("Markdown conversion failed: {}", e);
            return Err(e);
        }
    };

    let path = pick_save_path(&app, &format!("{}.md", meeting_id), "md").await?;
    save_markdown_file(&path, &markdown)?;

    info!("Markdown summary saved to: {}", path);
    Ok(path)
}

/// Converts the Blocknote JSON summary to Markdown, renders it to PDF, lets the
/// user pick a save location, writes the file, and returns the saved path.
#[tauri::command]
pub async fn api_export_summary_pdf<R: Runtime>(
    app: tauri::AppHandle<R>,
    meeting_id: String,
    content_json: String,
) -> Result<String, String> {
    info!("api_export_summary_pdf called for meeting_id: {}", meeting_id);

    let markdown = match blocknote_json_to_markdown(&content_json) {
        Ok(m) => m,
        Err(e) => {
            warn!("Markdown conversion failed: {}", e);
            return Err(e);
        }
    };

    let path = pick_save_path(&app, &format!("{}.pdf", meeting_id), "pdf").await?;
    build_pdf(&path, &markdown, &meeting_id)?;

    info!("PDF summary saved to: {}", path);
    Ok(path)
}

/// Converts the Blocknote JSON summary to Markdown, renders it to DOCX, lets the
/// user pick a save location, writes the file, and returns the saved path.
#[tauri::command]
pub async fn api_export_summary_docx<R: Runtime>(
    app: tauri::AppHandle<R>,
    meeting_id: String,
    content_json: String,
) -> Result<String, String> {
    info!("api_export_summary_docx called for meeting_id: {}", meeting_id);

    let markdown = match blocknote_json_to_markdown(&content_json) {
        Ok(m) => m,
        Err(e) => {
            warn!("Markdown conversion failed: {}", e);
            return Err(e);
        }
    };

    let path = pick_save_path(&app, &format!("{}.docx", meeting_id), "docx").await?;
    build_docx(&path, &markdown, &meeting_id)?;

    info!("DOCX summary saved to: {}", path);
    Ok(path)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heading() {
        let json = r#"[{"type":"heading","level":2,"content":["Hello"]}]"#;
        let md = blocknote_json_to_markdown(json).unwrap();
        assert!(md.contains("## Hello"));
    }

    #[test]
    fn test_paragraph() {
        let json = r#"[{"type":"paragraph","content":"Just text"}]"#;
        let md = blocknote_json_to_markdown(json).unwrap();
        assert!(md.contains("Just text"));
    }

    #[test]
    fn test_bullet_list_with_nesting() {
        let json = r#"[
            {
                "type": "bullet-list",
                "blocks": [
                    {
                        "type": "list-item",
                        "content": ["Top item"],
                        "blocks": [
                            {
                                "type": "bullet-list",
                                "blocks": [
                                    { "type": "list-item", "content": ["Nested item"] }
                                ]
                            }
                        ]
                    }
                ]
            }
        ]"#;
        let md = blocknote_json_to_markdown(json).unwrap();
        assert!(md.contains("- Top item"));
        assert!(md.contains("  - Nested item"));
    }

    #[test]
    fn test_rich_text_bold_and_link() {
        let json = r#"[
            {
                "type": "paragraph",
                "content": [
                    { "text": "bold", "bold": true },
                    { "text": "link", "url": "https://example.com" }
                ]
            }
        ]"#;
        let md = blocknote_json_to_markdown(json).unwrap();
        assert!(md.contains("**bold**"));
        assert!(md.contains("[link](https://example.com)"));
    }

    #[test]
    fn test_numbered_list() {
        let json = r#"[
            {
                "type": "numbered-list",
                "blocks": [
                    { "type": "list-item", "content": ["One"] },
                    { "type": "list-item", "content": ["Two"] }
                ]
            }
        ]"#;
        let md = blocknote_json_to_markdown(json).unwrap();
        assert!(md.contains("1. One"));
        assert!(md.contains("2. Two"));
    }

    #[test]
    fn test_invalid_json() {
        assert!(blocknote_json_to_markdown("not json").is_err());
    }

    #[test]
    fn test_non_array_json() {
        assert!(blocknote_json_to_markdown(r#"{"type":"paragraph"}"#).is_err());
    }
}
