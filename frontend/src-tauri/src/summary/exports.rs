/// Advanced export functionality for AI meeting summaries.
///
/// The AI summary is stored as Blocknote JSON (a JSON array of Blocknote block
/// objects). This module converts that JSON into Markdown (for the .md export)
/// and into a structured block list (for the PDF and DOCX exports, which render
/// real headings, lists, and tables rather than raw Markdown text). It also
/// exposes Tauri commands that let the frontend pick a save location via the
/// native dialog (pre-filled with the meeting title + extension) and write the
/// file.
///
/// This module is purely additive: it introduces new commands and new
/// dependencies without modifying any existing command signatures.

use serde_json::Value;
use tauri::Runtime;
use tauri_plugin_dialog::DialogExt;
use tracing::{info, warn};

// ============================================================================
// Blocknote JSON -> Markdown conversion (for the .md export)
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
                let sub_type = sub
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
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
                // An element is either a plain string or a rich-text object.
                let text = if let Some(s) = item.as_str() {
                    s.to_string()
                } else {
                    item
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                };
                let mut s = text;

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
// Blocknote JSON -> structured blocks (for PDF / DOCX rendering)
// ============================================================================

/// A single renderable block in the summary document.
enum Block {
    Heading { level: usize, text: String },
    Paragraph { text: String },
    Bullet { text: String, indent: usize },
    Numbered { text: String, number: usize, indent: usize },
    Quote { text: String },
    Code { lang: String, code: String },
    Divider,
    Table { rows: Vec<Vec<String>>, header: bool },
}

/// Extracts plain text from a Blocknote `content` value (a plain string or an
/// array of rich-text objects). Inline styling (bold/italic/code/link) is
/// collapsed to plain text for the export; block-level structure (headings,
/// tables, lists) is preserved by the block type.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|it| {
                // An element is either a plain string or a rich-text object.
                if let Some(s) = it.as_str() {
                    s.to_string()
                } else {
                    it.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string()
                }
            })
            .collect(),
        _ => String::new(),
    }
}

/// Extracts the text of a table cell (whose `content` is an array of blocks).
fn cell_text(cell: &Value) -> String {
    match cell.get("content") {
        Some(Value::Array(inner)) => inner
            .iter()
            .map(|b| content_text(b.get("content").unwrap_or(&Value::Null)))
            .collect(),
        other => content_text(other.unwrap_or(&Value::Null)),
    }
}

/// Parses a Blocknote JSON document into a flat list of renderable blocks.
fn parse_blocks(json: &str) -> Result<Vec<Block>, String> {
    let value: Value =
        serde_json::from_str(json).map_err(|e| format!("Invalid JSON: {}", e))?;
    let blocks = value
        .as_array()
        .ok_or_else(|| "Summary JSON must be an array of blocks".to_string())?;

    let mut out = Vec::new();
    for block in blocks {
        render_block_struct(block, 0, &mut out);
    }
    Ok(out)
}

/// Recursively flattens a Blocknote block (and nested lists) into `out`.
fn render_block_struct(block: &Value, indent: usize, out: &mut Vec<Block>) {
    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match block_type {
        "heading" => {
            let level = heading_level(block);
            let text = content_text(block.get("content").unwrap_or(&Value::Null));
            if !text.is_empty() {
                out.push(Block::Heading { level, text });
            }
        }
        "paragraph" => {
            let text = content_text(block.get("content").unwrap_or(&Value::Null));
            if !text.is_empty() {
                out.push(Block::Paragraph { text });
            }
        }
        "quote" => {
            let text = content_text(block.get("content").unwrap_or(&Value::Null));
            if !text.is_empty() {
                out.push(Block::Quote { text });
            }
        }
        "divider" => {
            out.push(Block::Divider);
        }
        "code" => {
            let code = match block.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => content_text(other),
                None => String::new(),
            };
            let lang = block
                .get("language")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            out.push(Block::Code { lang, code });
        }
        "table" => {
            if let Some(table) = parse_table(block) {
                out.push(table);
            }
        }
        "bullet-list" | "numbered-list" => {
            render_list_struct(block, block_type == "numbered-list", indent, out);
        }
        _ => {
            // Unknown block: best-effort render of its content.
            if let Some(c) = block.get("content") {
                let text = content_text(c);
                if !text.is_empty() {
                    out.push(Block::Paragraph { text });
                }
            }
        }
    }
}

/// Flattens a (possibly nested) list block into Bullet/Numbered blocks.
/// Numbered items carry a running counter per indent depth.
fn render_list_struct(
    block: &Value,
    numbered: bool,
    indent: usize,
    out: &mut Vec<Block>,
) {
    let items = match block.get("blocks").and_then(|v| v.as_array()) {
        Some(items) => items,
        None => return,
    };
    let mut counter = 1usize;
    for item in items {
        let text = content_text(item.get("content").unwrap_or(&Value::Null));
        if !text.is_empty() {
            if numbered {
                out.push(Block::Numbered {
                    text,
                    number: counter,
                    indent,
                });
            } else {
                out.push(Block::Bullet { text, indent });
            }
            counter += 1;
        }
        // Recurse into nested lists.
        if let Some(nested) = item.get("blocks").and_then(|v| v.as_array()) {
            for sub in nested {
                let sub_type = sub
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if sub_type == "bullet-list" || sub_type == "numbered-list" {
                    render_list_struct(sub, sub_type == "numbered-list", indent + 1, out);
                }
            }
        }
    }
}

/// Parses a Blocknote `table` block into a Table block (rows of cell text).
fn parse_table(block: &Value) -> Option<Block> {
    let rows = block.get("content").and_then(|v| v.as_array())?;
    let mut out_rows: Vec<Vec<String>> = Vec::new();
    let mut has_header = false;
    for row in rows {
        let row_type = row.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if row_type != "tableRow" {
            continue;
        }
        let cells: &[Value] = row
            .get("content")
            .and_then(|v| v.as_array())
            .map(|a| a.as_slice())
            .unwrap_or(&[]);
        let mut row_cells: Vec<String> = Vec::new();
        for cell in cells {
            let cell_type = cell.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if cell_type == "tableHeader" {
                has_header = true;
            }
            row_cells.push(cell_text(cell));
        }
        if !row_cells.is_empty() {
            out_rows.push(row_cells);
        }
    }
    if out_rows.is_empty() {
        None
    } else {
        Some(Block::Table {
            rows: out_rows,
            header: has_header,
        })
    }
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
// PDF export (printpdf 0.5.3)
// ============================================================================

/// Renders the structured blocks into a readable PDF at `path`.
///
/// The title is drawn as a larger bold line at the top; headings, paragraphs,
/// bullets, numbered items, quotes, code, and (ruled) tables are each rendered
/// with their own styling so the PDF mirrors the on-screen summary rather than
/// raw Markdown text. Page breaks are handled manually.
pub fn build_pdf(path: &str, title: &str, blocks: &[Block]) -> Result<(), String> {
    use printpdf::{
        BuiltinFont, Color, Greyscale, Line, Mm, PdfDocument, PdfLayerIndex, PdfPageIndex, Point,
    };
    use std::io::Write;

    // A4 portrait in millimetres. 20 mm margins.
    let page_w = 210.0;
    let page_h = 297.0;
    let margin = 20.0;
    let top_y = page_h - margin; // y grows from the bottom edge

    // Create the document, first page, and its first layer (4-arg tuple API).
    let (doc, page1, layer1) =
        PdfDocument::new("Meetily Summary", Mm(page_w), Mm(page_h), "Layer 1");

    let font = doc
        .add_builtin_font(BuiltinFont::Helvetica)
        .map_err(|e| format!("Failed to add PDF font: {}", e))?;
    let font_bold = doc
        .add_builtin_font(BuiltinFont::HelveticaBold)
        .map_err(|e| format!("Failed to add PDF font (bold): {}", e))?;
    let font_mono = doc
        .add_builtin_font(BuiltinFont::Courier)
        .map_err(|e| format!("Failed to add PDF font (mono): {}", e))?;

    // Colors.
    let text_color = Color::Greyscale(Greyscale::new(0.08, None));
    let grid_color = Color::Greyscale(Greyscale::new(0.55, None));
    let header_fill = Color::Greyscale(Greyscale::new(0.92, None));

    // The current page/layer, so lines keep landing on the right page after a
    // break. `cur_page` is the index, `cur_layer` its layer index.
    let mut cur_page = page1;
    let mut cur_layer = layer1;
    let mut y = top_y - 18.0; // start below the title

    // Text width heuristic in mm. Proportional ~0.5 * size per char; mono ~0.6.
    fn text_w(s: &str, size: f64, mono: bool) -> f64 {
        let f = if mono { 0.6 } else { 0.5 };
        s.chars().count() as f64 * f * size / 10.0
    }

    // Wrap text to fit `max_w` mm.
    fn wrap(s: &str, size: f64, max_w: f64) -> Vec<String> {
        let mut lines = Vec::new();
        for para in s.split('\n') {
            let words: Vec<&str> = para.split(' ').filter(|w| !w.is_empty()).collect();
            if words.is_empty() {
                lines.push(String::new());
                continue;
            }
            let mut cur = String::new();
            for w in words {
                let candidate = if cur.is_empty() {
                    w.to_string()
                } else {
                    format!("{} {}", cur, w)
                };
                if !cur.is_empty() && text_w(&candidate, size, false) > max_w {
                    lines.push(cur.clone());
                    cur = w.to_string();
                } else {
                    cur = candidate;
                }
            }
            lines.push(cur);
        }
        lines
    }

    // Draw one text line at (x, y); page-break first if needed.
    let draw_line = |layer: &printpdf::PdfLayerReference,
                    page: &mut PdfPageIndex,
                    layer_idx: &mut PdfLayerIndex,
                    y: &mut f64,
                    text: &str,
                    size: f64,
                    f: &printpdf::IndirectFontRef,
                    x: f64| {
        if *y < margin {
            let (np, nl) = doc.add_page(Mm(page_w), Mm(page_h), "Layer 1");
            *page = np;
            *layer_idx = nl;
            *y = top_y;
        }
        let l = doc.get_page(*page).get_layer(*layer_idx);
        l.set_fill_color(text_color.clone());
        l.use_text(text, size, Mm(x), Mm(*y), f);
        *y -= size * 1.5 / 10.0; // line height
        let _ = layer;
    };

    // Title (larger, bold).
    {
        let l = doc.get_page(page1).get_layer(layer1);
        l.set_fill_color(text_color.clone());
        l.use_text(title, 16.0, Mm(margin), Mm(top_y), &font_bold);
    }

    let content_w = page_w - 2.0 * margin;

    for block in blocks {
        match block {
            Block::Heading { level, text } => {
                let (size, f) = match level {
                    1 => (18.0, &font_bold),
                    2 => (15.0, &font_bold),
                    3 => (13.0, &font_bold),
                    _ => (12.0, &font_bold),
                };
                for line in wrap(text, size, content_w) {
                    draw_line(
                        &doc.get_page(cur_page).get_layer(cur_layer),
                        &mut cur_page,
                        &mut cur_layer,
                        &mut y,
                        &line,
                        size,
                        f,
                        margin,
                    );
                }
                y -= 2.0;
            }
            Block::Paragraph { text } => {
                for line in wrap(text, 11.0, content_w) {
                    draw_line(
                        &doc.get_page(cur_page).get_layer(cur_layer),
                        &mut cur_page,
                        &mut cur_layer,
                        &mut y,
                        &line,
                        11.0,
                        &font,
                        margin,
                    );
                }
                y -= 4.0;
            }
            Block::Bullet { text, indent } => {
                let x = margin + *indent as f64 * 8.0;
                let w = content_w - *indent as f64 * 8.0;
                for (i, line) in wrap(text, 11.0, w - 6.0).iter().enumerate() {
                    let prefix = if i == 0 { "•  " } else { "    " };
                    let full = format!("{}{}", prefix, line);
                    draw_line(
                        &doc.get_page(cur_page).get_layer(cur_layer),
                        &mut cur_page,
                        &mut cur_layer,
                        &mut y,
                        &full,
                        11.0,
                        &font,
                        x,
                    );
                }
                y -= 2.0;
            }
            Block::Numbered { text, number, indent } => {
                let x = margin + *indent as f64 * 8.0;
                let w = content_w - *indent as f64 * 8.0;
                for (i, line) in wrap(text, 11.0, w - 8.0).iter().enumerate() {
                    let prefix = if i == 0 {
                        format!("{}. ", number)
                    } else {
                        "    ".to_string()
                    };
                    let full = format!("{}{}", prefix, line);
                    draw_line(
                        &doc.get_page(cur_page).get_layer(cur_layer),
                        &mut cur_page,
                        &mut cur_layer,
                        &mut y,
                        &full,
                        11.0,
                        &font,
                        x,
                    );
                }
                y -= 2.0;
            }
            Block::Quote { text } => {
                for line in wrap(text, 11.0, content_w - 10.0) {
                    let full = format!("| {}", line);
                    draw_line(
                        &doc.get_page(cur_page).get_layer(cur_layer),
                        &mut cur_page,
                        &mut cur_layer,
                        &mut y,
                        &full,
                        11.0,
                        &font,
                        margin + 4.0,
                    );
                }
                y -= 4.0;
            }
            Block::Code { code, .. } => {
                for line in wrap(code, 9.5, content_w - 8.0) {
                    draw_line(
                        &doc.get_page(cur_page).get_layer(cur_layer),
                        &mut cur_page,
                        &mut cur_layer,
                        &mut y,
                        &line,
                        9.5,
                        &font_mono,
                        margin + 4.0,
                    );
                }
                y -= 4.0;
            }
            Block::Divider => {
                // A thin horizontal rule.
                if y < margin {
                    let (np, nl) = doc.add_page(Mm(page_w), Mm(page_h), "Layer 1");
                    cur_page = np;
                    cur_layer = nl;
                    y = top_y;
                }
                let l = doc.get_page(cur_page).get_layer(cur_layer);
                l.set_outline_color(grid_color.clone());
                l.set_outline_thickness(0.4);
                let rule = Line::from_iter(vec![
                    (Point::new(Mm(margin), Mm(y)), false),
                    (Point::new(Mm(page_w - margin), Mm(y)), false),
                ]);
                l.add_shape(rule);
                y -= 8.0;
            }
            Block::Table { rows, .. } => {
                if rows.is_empty() {
                    continue;
                }
                let n_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                if n_cols == 0 {
                    continue;
                }

                // Page-break if the table won't fit.
                let row_h = 8.0;
                let table_h = rows.len() as f64 * row_h;
                if y - table_h < margin {
                    let (np, nl) = doc.add_page(Mm(page_w), Mm(page_h), "Layer 1");
                    cur_page = np;
                    cur_layer = nl;
                    y = top_y;
                }
                let l = doc.get_page(cur_page).get_layer(cur_layer);

                // Column widths: max char length per column (mono), + padding.
                let mut col_w: Vec<f64> = vec![0.0; n_cols];
                for row in rows {
                    for (c, cell) in row.iter().enumerate() {
                        let longest = cell.chars().count();
                        let w = longest as f64 * 0.6 * 9.5 / 10.0 + 6.0;
                        if w > col_w[c] {
                            col_w[c] = w;
                        }
                    }
                }
                let total_w: f64 = col_w.iter().sum();
                let x0 = margin;
                let table_top = y;

                // Header row shading (filled, closed rectangle).
                let mut cx = x0;
                l.set_fill_color(header_fill.clone());
                for c in 0..n_cols {
                    let rect = Line::from_iter(vec![
                        (Point::new(Mm(cx), Mm(table_top)), false),
                        (Point::new(Mm(cx + col_w[c]), Mm(table_top)), false),
                        (Point::new(Mm(cx + col_w[c]), Mm(table_top - row_h)), false),
                        (Point::new(Mm(cx), Mm(table_top - row_h)), false),
                    ]);
                    let mut rect = rect;
                    rect.set_closed(true);
                    rect.set_fill(true);
                    l.add_shape(rect);
                    cx += col_w[c];
                }

                // Grid lines.
                l.set_outline_color(grid_color.clone());
                l.set_outline_thickness(0.3);
                for r in 0..=rows.len() {
                    let ly = table_top - r as f64 * row_h;
                    let line = Line::from_iter(vec![
                        (Point::new(Mm(x0), Mm(ly)), false),
                        (Point::new(Mm(x0 + total_w), Mm(ly)), false),
                    ]);
                    l.add_shape(line);
                }
                let mut vx = x0;
                for c in 0..=n_cols {
                    let line = Line::from_iter(vec![
                        (Point::new(Mm(vx), Mm(table_top)), false),
                        (Point::new(Mm(vx), Mm(table_top - table_h)), false),
                    ]);
                    l.add_shape(line);
                    if c < n_cols {
                        vx += col_w[c];
                    }
                }

                // Cell text.
                l.set_fill_color(text_color.clone());
                for (r, row) in rows.iter().enumerate() {
                    let mut cx = x0;
                    let is_header = r == 0;
                    for (c, cell) in row.iter().enumerate() {
                        if c >= n_cols {
                            break;
                        }
                        let f = if is_header { &font_bold } else { &font_mono };
                        let lines = wrap(cell, 9.5, col_w[c] - 6.0);
                        let mut ty = table_top - r as f64 * row_h - 2.5;
                        for line in lines.iter().take(2) {
                            l.use_text(line, 9.5, Mm(cx + 3.0), Mm(ty), f);
                            ty -= 9.5 * 1.3 / 10.0;
                        }
                        cx += col_w[c];
                    }
                }
                y = table_top - table_h - 6.0;
            }
        }
    }

    ensure_parent_dir(path)?;
    let mut file = std::fs::File::create(path)
        .map_err(|e| format!("Failed to create PDF file: {}", e))?;
    let mut writer = std::io::BufWriter::new(&mut file);
    doc.save(&mut writer)
        .map_err(|e| format!("Failed to save PDF: {}", e))?;
    writer
        .flush()
        .map_err(|e| format!("Failed to flush PDF: {}", e))?;
    Ok(())
}

// ============================================================================
// DOCX export (hand-written OOXML via zip — no external docx crate)
// ============================================================================

/// Escapes a string for use inside an XML text node.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Builds a single `<w:p>` paragraph from `text` with the given run properties.
/// `size_half_points` is the font size in half-points (22 = 11pt, 32 = 16pt).
/// `bold` and `italic` set the run flags. `space_before`/`space_after` are in
/// 1/20 pt units.
fn docx_paragraph(
    text: &str,
    size_half_points: u32,
    bold: bool,
    italic: bool,
    space_before: u32,
    space_after: u32,
) -> String {
    let mut rpr = String::new();
    if bold {
        rpr.push_str("<w:b/>");
    }
    if italic {
        rpr.push_str("<w:i/>");
    }
    rpr.push_str(&format!("<w:sz w:val=\"{}\"/>", size_half_points));
    format!(
        "<w:p><w:pPr><w:spacing w:before=\"{}\" w:after=\"{}\"/></w:pPr><w:r><w:rPr>{}</w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
        space_before, space_after, rpr, xml_escape(text)
    )
}

/// Builds a DOCX `<w:tbl>` (a table with grid, borders, and shaded header row).
fn docx_table(rows: &[Vec<String>], header: bool) -> String {
    let n_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if n_cols == 0 {
        return String::new();
    }

    // Table-level borders (single, 4 = 0.5pt).
    let borders = concat!(
        "<w:tblBorders>",
        "<w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "</w:tblBorders>"
    );

    let mut grid = String::new();
    for _ in 0..n_cols {
        grid.push_str("<w:gridCol/>");
    }

    let mut trs = String::new();
    for (r, row) in rows.iter().enumerate() {
        let is_header = header && r == 0;
        let mut tcs = String::new();
        for c in 0..n_cols {
            let cell_text = row.get(c).map(|s| s.as_str()).unwrap_or("");
            let shading = if is_header {
                "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"E8E8E8\"/>"
            } else {
                ""
            };
            let run = if is_header {
                docx_paragraph(cell_text, 20, true, false, 0, 0)
            } else {
                docx_paragraph(cell_text, 20, false, false, 0, 0)
            };
            tcs.push_str(&format!(
                "<w:tc><w:tcPr>{}</w:tcPr>{}</w:tc>",
                shading, run
            ));
        }
        trs.push_str(&format!("<w:tr>{}</w:tr>", tcs));
    }

    format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/>{}</w:tblPr><w:tblGrid>{}</w:tblGrid>{}</w:tbl>",
        borders, grid, trs
    )
}

/// Renders the structured blocks into a DOCX document at `path`.
///
/// A .docx is just a zip of a few XML files; we write the minimum set
/// ([Content_Types].xml, _rels/.rels, word/document.xml, word/_rels/document.xml.rels)
/// directly with the `zip` crate. Headings, paragraphs, lists, quotes, code,
/// and (ruled) tables are each rendered with their own OOXML styling so the
/// document mirrors the on-screen summary rather than raw Markdown text.
pub fn build_docx(path: &str, title: &str, blocks: &[Block]) -> Result<(), String> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    let mut body = String::new();

    // Title (bold, 16pt = 32 half-points).
    body.push_str(&docx_paragraph(title, 32, true, false, 0, 240));

    for block in blocks {
        match block {
            Block::Heading { level, text } => {
                let size = match level {
                    1 => 32,
                    2 => 28,
                    3 => 24,
                    _ => 22,
                };
                body.push_str(&docx_paragraph(text, size, true, false, 160, 80));
            }
            Block::Paragraph { text } => {
                body.push_str(&docx_paragraph(text, 22, false, false, 0, 120));
            }
            Block::Bullet { text, indent } => {
                let indent_units = 200 * (indent + 1);
                let p = docx_paragraph(text, 22, false, false, 0, 60);
                // Wrap with a left indent.
                body.push_str(&format!(
                    "<w:p><w:pPr><w:ind w:left=\"{}\"/><w:spacing w:before=\"0\" w:after=\"60\"/></w:pPr><w:r><w:rPr><w:sz w:val=\"22\"/></w:rPr><w:t xml:space=\"preserve\">• {}</w:t></w:r></w:p>",
                    indent_units,
                    xml_escape(text)
                ));
                let _ = p;
            }
            Block::Numbered { text, number, indent } => {
                let indent_units = 200 * (indent + 1);
                body.push_str(&format!(
                    "<w:p><w:pPr><w:ind w:left=\"{}\"/><w:spacing w:before=\"0\" w:after=\"60\"/></w:pPr><w:r><w:rPr><w:sz w:val=\"22\"/></w:rPr><w:t xml:space=\"preserve\">{}. {}</w:t></w:r></w:p>",
                    indent_units,
                    number,
                    xml_escape(text)
                ));
            }
            Block::Quote { text } => {
                body.push_str(&docx_paragraph(text, 22, false, true, 80, 80));
            }
            Block::Code { code, .. } => {
                // Each line as a monospace paragraph.
                for line in code.split('\n') {
                    body.push_str(&format!(
                        "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/></w:pPr><w:r><w:rPr><w:sz w:val=\"20\"/><w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\"/></w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
                        xml_escape(line)
                    ));
                }
                body.push_str(&docx_paragraph("", 22, false, false, 0, 120));
            }
            Block::Divider => {
                // A paragraph with a bottom border acts as a horizontal rule.
                body.push_str(
                    "<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"999999\"/></w:pBdr></w:pPr></w:p>",
                );
            }
            Block::Table { rows, header } => {
                body.push_str(&docx_table(rows, *header));
                // Spacer paragraph after the table (Word requires a paragraph
                // after a table at the end of a document).
                body.push_str(&docx_paragraph("", 22, false, false, 0, 120));
            }
        }
    }

    let document_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{}</w:body></w:document>",
        body
    );
    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let doc_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#;

    ensure_parent_dir(path)?;
    let mut file = std::fs::File::create(path)
        .map_err(|e| format!("Failed to create DOCX file: {}", e))?;
    let mut zip = ZipWriter::new(&mut file);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("[Content_Types].xml", opts)
        .map_err(|e| format!("Failed to write DOCX: {}", e))?;
    zip.write_all(content_types.as_bytes())
        .map_err(|e| format!("Failed to write DOCX: {}", e))?;
    zip.start_file("_rels/.rels", opts)
        .map_err(|e| format!("Failed to write DOCX: {}", e))?;
    zip.write_all(rels.as_bytes())
        .map_err(|e| format!("Failed to write DOCX: {}", e))?;
    zip.start_file("word/document.xml", opts)
        .map_err(|e| format!("Failed to write DOCX: {}", e))?;
    zip.write_all(document_xml.as_bytes())
        .map_err(|e| format!("Failed to write DOCX: {}", e))?;
    zip.start_file("word/_rels/document.xml.rels", opts)
        .map_err(|e| format!("Failed to write DOCX: {}", e))?;
    zip.write_all(doc_rels.as_bytes())
        .map_err(|e| format!("Failed to write DOCX: {}", e))?;
    zip.finish()
        .map_err(|e| format!("Failed to save DOCX: {}", e))?;
    Ok(())
}

// ============================================================================
// Tauri commands
// ============================================================================

/// Picks a save location via the native dialog and returns the chosen path,
/// ensuring it carries the expected file extension. `default_name` is used to
/// pre-fill the dialog's file-name field (e.g. the meeting title + extension).
async fn pick_save_path<R: Runtime>(
    app: &tauri::AppHandle<R>,
    default_name: &str,
    extension: &str,
) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    // Pre-fill the dialog's file-name field with `default_name` (the meeting
    // title + extension). The native dialog does not let us preset the
    // "Save as type" dropdown, so the extension is also re-asserted on the
    // returned path below.
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<std::path::PathBuf>>();
    app
        .dialog()
        .file()
        .set_title("Save Summary")
        .set_file_name(default_name)
        .save_file(move |path| {
            let _ = tx.send(path.and_then(|p| p.into_path().ok()));
        });
    let chosen = rx
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
/// location (pre-filled with the meeting title), writes the file, and returns
/// the saved path.
#[tauri::command]
pub async fn api_export_summary_markdown<R: Runtime>(
    app: tauri::AppHandle<R>,
    meeting_id: String,
    meeting_name: String,
    content_json: String,
) -> Result<String, String> {
    info!(
        "api_export_summary_markdown called for meeting_id: {}",
        meeting_id
    );

    let markdown = match blocknote_json_to_markdown(&content_json) {
        Ok(m) => m,
        Err(e) => {
            warn!("Markdown conversion failed: {}", e);
            return Err(e);
        }
    };

    let default_name = sanitize_file_name(&meeting_name, "md");
    let path = pick_save_path(&app, &default_name, "md").await?;
    save_markdown_file(&path, &markdown)?;

    info!("Markdown summary saved to: {}", path);
    Ok(path)
}

/// Converts the Blocknote JSON summary into structured blocks, renders them to
/// PDF, lets the user pick a save location (pre-filled with the meeting title),
/// writes the file, and returns the saved path.
#[tauri::command]
pub async fn api_export_summary_pdf<R: Runtime>(
    app: tauri::AppHandle<R>,
    meeting_id: String,
    meeting_name: String,
    content_json: String,
) -> Result<String, String> {
    info!("api_export_summary_pdf called for meeting_id: {}", meeting_id);

    let blocks = match parse_blocks(&content_json) {
        Ok(b) => b,
        Err(e) => {
            warn!("Block parsing failed: {}", e);
            return Err(e);
        }
    };

    let default_name = sanitize_file_name(&meeting_name, "pdf");
    let path = pick_save_path(&app, &default_name, "pdf").await?;
    build_pdf(&path, &meeting_name, &blocks)?;

    info!("PDF summary saved to: {}", path);
    Ok(path)
}

/// Converts the Blocknote JSON summary into structured blocks, renders them to
/// DOCX, lets the user pick a save location (pre-filled with the meeting title),
/// writes the file, and returns the saved path.
#[tauri::command]
pub async fn api_export_summary_docx<R: Runtime>(
    app: tauri::AppHandle<R>,
    meeting_id: String,
    meeting_name: String,
    content_json: String,
) -> Result<String, String> {
    info!("api_export_summary_docx called for meeting_id: {}", meeting_id);

    let blocks = match parse_blocks(&content_json) {
        Ok(b) => b,
        Err(e) => {
            warn!("Block parsing failed: {}", e);
            return Err(e);
        }
    };

    let default_name = sanitize_file_name(&meeting_name, "docx");
    let path = pick_save_path(&app, &default_name, "docx").await?;
    build_docx(&path, &meeting_name, &blocks)?;

    info!("DOCX summary saved to: {}", path);
    Ok(path)
}

/// Builds a safe default file name from the meeting title and the target
/// extension (e.g. "Team Sync" + "pdf" -> "Team Sync.pdf"). Falls back to
/// "summary.<ext>" if the title is empty or would produce an invalid name.
fn sanitize_file_name(meeting_name: &str, extension: &str) -> String {
    let cleaned: String = meeting_name
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        format!("summary.{}", extension)
    } else {
        format!("{}.{}", trimmed, extension)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heading() {
        let json = r#"[{"type":"heading","level":2,"content":[{"text":"Hello"}]}]"#;
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
                        "content": [{"text": "Top item"}],
                        "blocks": [
                            {
                                "type": "bullet-list",
                                "blocks": [
                                    { "type": "list-item", "content": [{"text": "Nested item"}] }
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
                    { "type": "list-item", "content": [{"text": "One"}] },
                    { "type": "list-item", "content": [{"text": "Two"}] }
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

    #[test]
    fn test_sanitize_file_name() {
        assert_eq!(sanitize_file_name("Team Sync", "pdf"), "Team Sync.pdf");
        assert_eq!(sanitize_file_name("", "pdf"), "summary.pdf");
        assert_eq!(sanitize_file_name("A/B\\C", "md"), "ABC.md");
    }

    #[test]
    fn test_parse_blocks_heading() {
        let json = r#"[{"type":"heading","level":1,"content":["Title"]}]"#;
        let blocks = parse_blocks(json).unwrap();
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::Heading { level, text } => {
                assert_eq!(*level, 1);
                assert_eq!(text, "Title");
            }
            _ => panic!("expected heading"),
        }
    }

    #[test]
    fn test_parse_blocks_table() {
        let json = r#"[
            {
                "type": "table",
                "content": [
                    {
                        "type": "tableRow",
                        "content": [
                            { "type": "tableHeader", "content": [{ "type": "paragraph", "content": ["Name"] }] },
                            { "type": "tableCell", "content": [{ "type": "paragraph", "content": ["Value"] }] }
                        ]
                    }
                ]
            }
        ]"#;
        let blocks = parse_blocks(json).unwrap();
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::Table { rows, header } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].len(), 2);
                assert!(*header);
            }
            _ => panic!("expected table"),
        }
    }
}

#[cfg(test)]
mod e2e_render {
    use super::*;

    #[test]
    fn render_real_pdf_and_docx() {
        // Realistic Blocknote JSON: heading, paragraph, table, list.
        let json = r#"[
            {"type":"heading","level":1,"content":[{"text":"Q3 Planning"}]},
            {"type":"paragraph","content":[{"text":"Goals and owners for the quarter."}]},
            {"type":"table","content":[
                {"type":"tableRow","content":[
                    {"type":"tableHeader","content":[{"type":"paragraph","content":[{"text":"Item"}]}]},
                    {"type":"tableCell","content":[{"type":"paragraph","content":[{"text":"Owner"}]}]}
                ]},
                {"type":"tableRow","content":[
                    {"type":"tableCell","content":[{"type":"paragraph","content":[{"text":"Ship exports"}]}]},
                    {"type":"tableCell","content":[{"type":"paragraph","content":[{"text":"Faraz"}]}]}
                ]}
            ]},
            {"type":"bullet-list","blocks":[
                {"type":"list-item","content":[{"text":"First action"}]},
                {"type":"list-item","content":[{"text":"Second action"}]}
            ]}
        ]"#;

        let blocks = parse_blocks(json).unwrap();
        assert_eq!(blocks.len(), 5); // heading + paragraph + table + 2 list items

        // Render a real PDF.
        let pdf_path = std::env::temp_dir().join("e2e_test.pdf");
        build_pdf(pdf_path.to_str().unwrap(), "Q3 Planning", &blocks).unwrap();
        let pdf_bytes = std::fs::read(&pdf_path).unwrap();
        assert!(pdf_bytes.starts_with(b"%PDF-"), "PDF magic missing");
        assert!(pdf_bytes.len() > 500, "PDF too small: {}", pdf_bytes.len());

        // Render a real DOCX.
        let docx_path = std::env::temp_dir().join("e2e_test.docx");
        build_docx(docx_path.to_str().unwrap(), "Q3 Planning", &blocks).unwrap();
        let docx_bytes = std::fs::read(&docx_path).unwrap();
        assert!(docx_bytes.starts_with(b"PK"), "DOCX (zip) magic missing");
        assert!(docx_bytes.len() > 200, "DOCX too small: {}", docx_bytes.len());

        // Verify the DOCX is a valid zip (PK magic) with non-trivial size.
        // (Content inspection is done separately with Python.)
        assert!(docx_bytes.starts_with(b"PK"));
        println!("E2E OK: pdf={} bytes, docx={} bytes", pdf_bytes.len(), docx_bytes.len());
    }
}
