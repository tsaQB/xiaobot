use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use crate::bot::models::{
    InputMedia, InputRichMessage, InputRichMessageMedia, RichBlock, RichBlockListItem,
    RichBlockTableCell,
};
use crate::parser::latex::sanitize_latex_for_telegram;
use crate::parser::rtl;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum ParserError {
    InvalidCoordinate(String),
    InvalidTag(String),
    MediaValidation(String),
    MalformedHtml(String),
}

impl std::fmt::Display for ParserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCoordinate(msg) => write!(f, "Invalid coordinate: {msg}"),
            Self::InvalidTag(msg) => write!(f, "Invalid tag: {msg}"),
            Self::MediaValidation(msg) => write!(f, "Media validation error: {msg}"),
            Self::MalformedHtml(msg) => write!(f, "Malformed HTML: {msg}"),
        }
    }
}

impl std::error::Error for ParserError {}

impl From<String> for ParserError {
    fn from(s: String) -> Self {
        Self::MediaValidation(s)
    }
}

#[path = "markdown/extended.rs"]
mod extended;
#[path = "markdown/inline.rs"]
mod inline;
#[path = "markdown/links.rs"]
mod links;

#[cfg(test)]
pub(crate) use extended::flatten_extended_inline;
pub(crate) use extended::{
    defined_footnotes, flatten_extended_inline_outside_code, flatten_extended_inline_with,
};
pub use inline::parse_inline;

#[path = "markdown/media.rs"]
mod media;

pub(crate) use media::*;

static RE_THINK_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(?:think|thought|reasoning|reflection)\b.*?</(?:think|thought|reasoning|reflection)>").expect("valid static regex")
});
static RE_TOOL_CALL_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(?:tool_call|function_calls?)\b.*?</(?:tool_call|function_calls?)>")
        .expect("valid static regex")
});
static RE_SQUARE_THINK_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)\[(?:think|thinking|thought|reasoning|reflection)\].*?\[/(?:think|thinking|thought|reasoning|reflection)\]")
        .expect("valid static regex")
});
static RE_UNCLOSED_THINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(?:think|thought|reasoning|reflection)\b.*$").expect("valid static regex")
});
static RE_UNCLOSED_SQUARE_THINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)\[(?:think|thinking|thought|reasoning|reflection)\](?:[^(].*|$)")
        .expect("valid static regex")
});
static RE_TRAILING_INCOMPLETE_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<\s*(?:t(?:h(?:i(?:n(?:k)?)?)?)?|t(?:h(?:o(?:u(?:g(?:h(?:t)?)?)?)?)?)?|r(?:e(?:a(?:s(?:o(?:n(?:i(?:n(?:g)?)?)?)?)?)?)?)?|r(?:e(?:f(?:l(?:e(?:c(?:t(?:i(?:o(?:n)?)?)?)?)?)?)?)?)?)?$").expect("valid static regex")
});
static RE_TRAILING_INCOMPLETE_SQUARE_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\[\s*(?:t(?:h(?:i(?:n(?:k(?:i(?:n(?:g)?)?)?)?)?)?)?|t(?:h(?:o(?:u(?:g(?:h(?:t)?)?)?)?)?)?|r(?:e(?:a(?:s(?:o(?:n(?:i(?:n(?:g)?)?)?)?)?)?)?)?|r(?:e(?:f(?:l(?:e(?:c(?:t(?:i(?:o(?:n)?)?)?)?)?)?)?)?)?)?$").expect("valid static regex")
});
static RE_LEAKED_CONTROL_TAGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)</?(?:think|thought|reasoning|reflection|tool_call|function_calls?)\b[^>]*>")
        .expect("valid static regex")
});

pub fn sanitize_leaked_llm_artifacts(text: &str) -> String {
    // Fast-path: if text contains no opening tag indicators, return text directly
    if !text.contains('<') && !text.contains('[') {
        return text.to_string();
    }

    // 1. Strip closed thinking / reflection / tool blocks
    let step1 = RE_THINK_BLOCK.replace_all(text, "");
    let step2 = RE_TOOL_CALL_BLOCK.replace_all(&step1, "");
    let step3 = RE_SQUARE_THINK_BLOCK.replace_all(&step2, "").into_owned();

    // 2. Strip unclosed thinking blocks to EOF
    let step4 = RE_UNCLOSED_THINK.replace_all(&step3, "");
    let step5 = RE_UNCLOSED_SQUARE_THINK
        .replace_all(&step4, "")
        .into_owned();

    // 3. Strip trailing partial opening tags (e.g. "<", "<th", "[th")
    let step6 = RE_TRAILING_INCOMPLETE_TAG
        .replace_all(&step5, "")
        .into_owned();
    let step7 = RE_TRAILING_INCOMPLETE_SQUARE_TAG
        .replace_all(&step6, "")
        .into_owned();

    // 4. Strip any residual leaked tags or control tokens
    let mut cleaned = RE_LEAKED_CONTROL_TAGS.replace_all(&step7, "").into_owned();
    cleaned = cleaned
        .replace("<|im_start|>", "")
        .replace("<|im_end|>", "")
        .replace("<|endoftext|>", "");

    cleaned
}

pub fn extract_thinking_and_answer(raw: &str) -> (Option<String>, String) {
    static RE_EXTRACT_THINK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)<(?:think|thought|reasoning|reflection)\b[^>]*>(.*?)</(?:think|thought|reasoning|reflection)>").expect("valid static regex")
    });
    static RE_EXTRACT_SQUARE_THINK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)\[(?:think|thinking|thought|reasoning|reflection)\](.*?)(?:\[/(?:think|thinking|thought|reasoning|reflection)\]|$)").expect("valid static regex")
    });

    let thinking = RE_EXTRACT_THINK
        .captures(raw)
        .or_else(|| RE_EXTRACT_SQUARE_THINK.captures(raw))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().trim().to_string())
        .filter(|s| !s.is_empty());

    let answer = sanitize_leaked_llm_artifacts(raw).trim().to_string();
    (thinking, answer)
}

fn compute_column_rtl_flags(rows: &[Vec<&str>]) -> Vec<bool> {
    let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut col_is_rtl = vec![false; col_count];
    for row in rows {
        for (col_idx, cell) in row.iter().enumerate() {
            if col_idx < col_is_rtl.len() && rtl::has_rtl_characters(cell) {
                col_is_rtl[col_idx] = true;
            }
        }
    }
    col_is_rtl
}

fn resolve_table_cell_align<'a>(
    explicit: Option<&'a str>,
    col_is_rtl: bool,
    cell_text: &str,
) -> &'a str {
    if let Some(align) = explicit {
        return align;
    }
    if col_is_rtl || rtl::has_rtl_characters(cell_text) {
        "right"
    } else {
        "left"
    }
}

fn is_ascii_numeric_cell(c: &str) -> bool {
    c.chars()
        .all(|ch| ch.is_ascii_digit() || ch.is_whitespace() || ch == '.' || ch == ',')
        && c.chars().any(|ch| ch.is_ascii_digit())
}

/// Splits a table row into cell strings, respecting escaping, code spans, and math blocks
/// so that pipes `|` inside `$ ... $`, `$$ ... $$`, `\( ... \)`, `\[ ... \]`, or ` `...` `
/// (e.g. absolute value `|x|`, norm `|v|_p`, or set builder `{x | x > 0}`) are preserved
/// inside the cell content rather than splitting the table columns prematurely.
fn split_table_row_cells(row_str: &str, is_box_table: bool) -> Vec<String> {
    let trimmed = row_str.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let is_delim = |c: char| -> bool {
        if is_box_table {
            "│|║┃".contains(c)
        } else {
            c == '|'
        }
    };

    let chars: Vec<char> = trimmed.chars().collect();
    let n = chars.len();

    // Determine if there is a leading outer border delimiter
    let start_idx = if n > 0 && is_delim(chars[0]) { 1 } else { 0 };

    // Determine if there is a trailing outer border delimiter (not escaped)
    let end_idx = if n > start_idx && is_delim(chars[n - 1]) {
        if n >= 2 && chars[n - 2] == '\\' {
            n
        } else {
            n - 1
        }
    } else {
        n
    };

    let mut cells: Vec<String> = Vec::new();
    let mut current_cell = String::new();

    let mut in_code = false;
    let mut in_inline_math = false;
    let mut in_display_math = false;
    let mut in_paren_math = false;
    let mut in_bracket_math = false;

    let mut i = start_idx;
    while i < end_idx {
        let ch = chars[i];

        // Check for escaping
        if ch == '\\' {
            if i + 1 < end_idx {
                let next = chars[i + 1];
                if next == '|' {
                    // Escaped pipe \| -> keep pipe in cell!
                    current_cell.push('|');
                    i += 2;
                    continue;
                } else if next == '(' && !in_code {
                    in_paren_math = true;
                    current_cell.push(ch);
                    current_cell.push(next);
                    i += 2;
                    continue;
                } else if next == ')' && !in_code {
                    in_paren_math = false;
                    current_cell.push(ch);
                    current_cell.push(next);
                    i += 2;
                    continue;
                } else if next == '[' && !in_code {
                    in_bracket_math = true;
                    current_cell.push(ch);
                    current_cell.push(next);
                    i += 2;
                    continue;
                } else if next == ']' && !in_code {
                    in_bracket_math = false;
                    current_cell.push(ch);
                    current_cell.push(next);
                    i += 2;
                    continue;
                }
            }
            current_cell.push(ch);
            i += 1;
            continue;
        }

        // Code span
        if ch == '`' {
            in_code = !in_code;
            current_cell.push(ch);
            i += 1;
            continue;
        }

        // Math handling when not in code
        if !in_code && ch == '$' {
            if i + 1 < end_idx && chars[i + 1] == '$' {
                in_display_math = !in_display_math;
                current_cell.push('$');
                current_cell.push('$');
                i += 2;
                continue;
            } else if !in_display_math {
                in_inline_math = !in_inline_math;
                current_cell.push('$');
                i += 1;
                continue;
            }
        }

        let in_math = in_inline_math || in_display_math || in_paren_math || in_bracket_math;

        if is_delim(ch) && !in_code && !in_math {
            cells.push(current_cell.trim().to_string());
            current_cell.clear();
        } else {
            current_cell.push(ch);
        }

        i += 1;
    }

    cells.push(current_cell.trim().to_string());
    cells
}

/// Upper bound on lines inspected when probing for a box-drawing table.
const MAX_TABLE_SCAN_LINES: usize = 500;

fn try_parse_table(
    lines: &[String],
    i: usize,
    is_message_rtl: bool,
) -> (Option<Vec<Vec<RichBlockTableCell>>>, bool, usize) {
    let n = lines.len();
    let line = lines[i].trim();

    // 1. Standard Markdown Table (| Col 1 | Col 2 |\n| --- | --- |)
    if line.contains('|') && i + 1 < n {
        let next_line = lines[i + 1].trim();
        let sep_cells = split_table_row_cells(next_line, false);

        let is_sep = !sep_cells.is_empty()
            && sep_cells.iter().any(|c| {
                !c.is_empty() && c.trim_matches(':').chars().all(|ch| ch == '-' || ch == '=')
            })
            && sep_cells.iter().all(|c| {
                if c.is_empty() {
                    return true;
                }
                let trimmed = c.trim_matches(':');
                !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '-' || ch == '=')
            });

        if is_sep {
            let mut explicit_aligns: Vec<Option<&str>> = Vec::new();
            for c in &sep_cells {
                if c.starts_with(':') && c.ends_with(':') {
                    explicit_aligns.push(Some("center"));
                } else if c.ends_with(':') {
                    explicit_aligns.push(Some("right"));
                } else if c.starts_with(':') {
                    explicit_aligns.push(Some("left"));
                } else {
                    explicit_aligns.push(None);
                }
            }

            let header_raw = split_table_row_cells(line, false);
            let mut raw_rows: Vec<Vec<String>> = Vec::new();
            let mut idx_line = i + 2;

            while idx_line < n {
                let row_str = lines[idx_line].trim();
                if row_str.is_empty() || !row_str.contains('|') {
                    break;
                }
                let row_raw = split_table_row_cells(row_str, false);
                raw_rows.push(row_raw);
                idx_line += 1;
            }

            let headers_str: Vec<&str> = header_raw.iter().map(|s| s.as_str()).collect();
            let table_is_rtl = rtl::is_table_predominantly_rtl(&headers_str);

            let col_is_rtl = {
                let mut all_rows: Vec<Vec<&str>> = Vec::with_capacity(raw_rows.len() + 1);
                all_rows.push(headers_str.clone());
                for r in &raw_rows {
                    all_rows.push(r.iter().map(|s| s.as_str()).collect());
                }
                compute_column_rtl_flags(&all_rows)
            };

            let mut header_row: Vec<RichBlockTableCell> = header_raw
                .into_iter()
                .enumerate()
                .map(|(idx, h)| {
                    let explicit = explicit_aligns.get(idx).copied().flatten();
                    let is_rtl = col_is_rtl.get(idx).copied().unwrap_or(false);
                    let align = resolve_table_cell_align(explicit, is_rtl, &h);
                    RichBlockTableCell::new(parse_inline(&h), true, Some(align))
                })
                .collect();

            let mut data_rows: Vec<Vec<RichBlockTableCell>> = Vec::with_capacity(raw_rows.len());
            for row_raw in raw_rows {
                let data_row: Vec<RichBlockTableCell> = row_raw
                    .into_iter()
                    .enumerate()
                    .map(|(idx, c)| {
                        let explicit = explicit_aligns.get(idx).copied().flatten();
                        let is_rtl = col_is_rtl.get(idx).copied().unwrap_or(false);
                        let align = resolve_table_cell_align(explicit, is_rtl, &c);
                        let is_numeric = is_ascii_numeric_cell(&c);
                        let formatted_cell = if (table_is_rtl || is_rtl) && is_numeric {
                            rtl::to_eastern_arabic_digits(&c)
                        } else {
                            c
                        };
                        RichBlockTableCell::new(parse_inline(&formatted_cell), false, Some(align))
                    })
                    .collect();
                data_rows.push(data_row);
            }

            if table_is_rtl && !is_message_rtl {
                header_row.reverse();
                for r in &mut data_rows {
                    r.reverse();
                }
            }

            let mut table_cells = Vec::with_capacity(data_rows.len() + 1);
            table_cells.push(header_row);
            table_cells.extend(data_rows);

            return (Some(table_cells), true, idx_line);
        }
    }

    // 2. Unicode Box or ASCII Grid Table (┌─┬─┐ or +---+---+)
    let is_unicode_box = line.chars().any(|c| "┌╔┏┬┰├┝┼╂".contains(c))
        || line
            .strip_prefix('│')
            .is_some_and(|rest| rest.contains('│'));
    let is_ascii_grid = line
        .strip_prefix('+')
        .is_some_and(|rest| rest.contains('+'))
        && (line.contains('-') || line.contains('='));

    if is_unicode_box || is_ascii_grid {
        let mut table_lines = Vec::new();
        let mut curr_i = i;
        // Bounded look-ahead: this probe runs for every paragraph line, so an
        // unbounded scan made long runs of box-drawing text quadratic.
        let scan_end = n.min(i.saturating_add(MAX_TABLE_SCAN_LINES));

        while curr_i < scan_end {
            let curr = lines[curr_i].trim();
            if curr.is_empty() {
                break;
            }
            if curr
                .chars()
                .any(|c| "┌╔┏┬┰├┝┼╂└╚┗┴┸┤┥│║┃|┐┘┒┙╗╝┚┖┓┛".contains(c))
                || curr
                    .strip_prefix('+')
                    .is_some_and(|rest| rest.contains('+'))
            {
                table_lines.push(curr);
                curr_i += 1;
            } else {
                break;
            }
        }

        if table_lines.len() >= 2 {
            let mut raw_rows = Vec::new();
            let mut has_header = false;
            let mut first_row_done = false;

            for l in &table_lines {
                if is_border_line(l) {
                    if first_row_done {
                        has_header = true;
                    }
                    continue;
                }
                let cols: Vec<String> = split_table_row_cells(l, true);

                if !cols.is_empty() && cols.iter().any(|c| !c.is_empty()) {
                    raw_rows.push(cols);
                    first_row_done = true;
                }
            }

            if !raw_rows.is_empty() {
                let headers_str: Vec<&str> = raw_rows[0].iter().map(|s| s.as_str()).collect();
                let table_is_rtl = if has_header {
                    rtl::is_table_predominantly_rtl(&headers_str)
                } else {
                    false
                };

                let col_is_rtl = {
                    let mut all_rows: Vec<Vec<&str>> = Vec::with_capacity(raw_rows.len());
                    for r in &raw_rows {
                        all_rows.push(r.iter().map(|s| s.as_str()).collect());
                    }
                    compute_column_rtl_flags(&all_rows)
                };
                let mut table_cells = Vec::with_capacity(raw_rows.len());

                for (r_idx, row_cols) in raw_rows.into_iter().enumerate() {
                    let is_hdr = r_idx == 0 && has_header;
                    let mut row: Vec<RichBlockTableCell> = row_cols
                        .into_iter()
                        .enumerate()
                        .map(|(idx, c)| {
                            let is_rtl = col_is_rtl.get(idx).copied().unwrap_or(false);
                            let align = resolve_table_cell_align(None, is_rtl, &c);
                            let is_numeric = is_ascii_numeric_cell(&c);
                            let formatted_c = if (table_is_rtl || is_rtl) && is_numeric {
                                rtl::to_eastern_arabic_digits(&c)
                            } else {
                                c
                            };
                            RichBlockTableCell::new(parse_inline(&formatted_c), is_hdr, Some(align))
                        })
                        .collect();
                    if table_is_rtl && !is_message_rtl {
                        row.reverse();
                    }
                    table_cells.push(row);
                }

                if table_cells.len() >= 2 || (!table_cells.is_empty() && has_header) {
                    return (Some(table_cells), has_header, curr_i);
                }
            }
        }
    }

    static RE_TABLE_UNDERLINE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^-{3,}$").expect("valid static regex"));
    static RE_SPACE_SPLIT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s{2,}|\t+").expect("valid static regex"));

    // 3. Plain Underline Table: Header \n ---------------- \n Data
    let underline_match = i + 1 < n && RE_TABLE_UNDERLINE.is_match(lines[i + 1].trim());
    if underline_match {
        let cols_hdr: Vec<&str> = RE_SPACE_SPLIT
            .split(line)
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
            .collect();

        if cols_hdr.len() >= 2 {
            let mut raw_rows: Vec<Vec<&str>> = vec![cols_hdr];
            let mut curr_i = i + 2;

            while curr_i < n {
                let curr = lines[curr_i].trim();
                if curr.is_empty() {
                    break;
                }
                if RE_TABLE_UNDERLINE.is_match(curr) {
                    curr_i += 1;
                    continue;
                }
                let data_cols: Vec<&str> = RE_SPACE_SPLIT
                    .split(curr)
                    .map(|c| c.trim())
                    .filter(|c| !c.is_empty())
                    .collect();
                if !data_cols.is_empty() {
                    raw_rows.push(data_cols);
                }
                curr_i += 1;
            }

            if raw_rows.len() >= 2 {
                let table_is_rtl = rtl::is_table_predominantly_rtl(&raw_rows[0]);

                let col_is_rtl = compute_column_rtl_flags(&raw_rows);
                let mut table_cells = Vec::with_capacity(raw_rows.len());

                for (r_idx, row_cols) in raw_rows.into_iter().enumerate() {
                    let is_hdr = r_idx == 0;
                    let mut row: Vec<RichBlockTableCell> = row_cols
                        .into_iter()
                        .enumerate()
                        .map(|(idx, c)| {
                            let is_rtl = col_is_rtl.get(idx).copied().unwrap_or(false);
                            let align = resolve_table_cell_align(None, is_rtl, c);
                            let is_numeric = is_ascii_numeric_cell(c);
                            let formatted_c = if (table_is_rtl || is_rtl) && is_numeric {
                                rtl::to_eastern_arabic_digits(c)
                            } else {
                                c.to_string()
                            };
                            RichBlockTableCell::new(parse_inline(&formatted_c), is_hdr, Some(align))
                        })
                        .collect();
                    if table_is_rtl && !is_message_rtl {
                        row.reverse();
                    }
                    table_cells.push(row);
                }

                return (Some(table_cells), true, curr_i);
            }
        }
    }

    (None, false, i)
}

fn extract_html_cite(text: &str) -> (String, Option<String>) {
    if let Some(start) = text.find("<cite>") {
        if let Some(end) = text[start + 6..].find("</cite>") {
            let credit = text[start + 6..start + 6 + end].trim().to_string();
            let mut body = text[..start].to_string();
            body.push_str(&text[start + 6 + end + 7..]);
            let credit_opt = if credit.is_empty() {
                None
            } else {
                Some(credit)
            };
            return (body.trim().to_string(), credit_opt);
        }
    }
    (text.to_string(), None)
}

fn extract_quote_credit(lines: &[String]) -> (String, Option<String>) {
    let combined = lines.join("\n");
    let (body, cite) = extract_html_cite(&combined);
    if cite.is_some() {
        return (body, cite);
    }
    if lines.len() > 1 {
        if let Some(last) = lines.last() {
            let trimmed = last.trim();
            for prefix in &["— ", "– ", "-- "] {
                if let Some(credit) = trimmed.strip_prefix(prefix) {
                    let credit = credit.trim();
                    if !credit.is_empty() {
                        let text = lines[..lines.len() - 1].join("\n");
                        return (text, Some(credit.to_string()));
                    }
                }
            }
        }
    }
    (combined, None)
}

/// Parse an accumulated streaming Markdown buffer without exposing syntax that
/// is still provisional. Completed syntax is rendered through the canonical
/// Rich Message parser; an incomplete tail is reduced to safe semantic text.
/// This lets a draft converge naturally without a second completion repaint.
pub fn parse_streaming_markdown_to_rich_blocks(text: &str) -> Vec<RichBlock> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let sanitized = sanitize_leaked_llm_artifacts(text);
    if sanitized.trim().is_empty() {
        return Vec::new();
    }

    let unstable_at = provisional_markdown_start(&sanitized).unwrap_or(sanitized.len());
    let mut blocks = parse_markdown_to_rich_blocks(&sanitized[..unstable_at]);
    if unstable_at < sanitized.len() {
        let provisional = sanitize_provisional_markdown(&sanitized[unstable_at..]);
        if !provisional.trim().is_empty() {
            blocks.push(RichBlock::Paragraph {
                text: Value::String(provisional),
            });
        }
    }
    blocks
}

fn provisional_markdown_start(text: &str) -> Option<usize> {
    let mut openings = Vec::new();

    // Fenced code dominates all inline syntax until the matching fence.
    let mut fence_open: Option<usize> = None;
    let mut offset = 0usize;
    for segment in text.split_inclusive('\n') {
        let trimmed = segment.trim_start();
        if trimmed.starts_with("```") {
            let marker = offset + (segment.len() - trimmed.len());
            if fence_open.is_some() {
                fence_open = None;
            } else {
                fence_open = Some(marker);
            }
        }
        offset += segment.len();
    }
    if let Some(index) = fence_open {
        openings.push(index);
    }

    // Inline code and emphasis are deliberately conservative: if a delimiter
    // is unmatched, the entire construct remains provisional rather than
    // flashing the raw opener to Telegram.
    for marker in ["**", "__", "`", "||", "~~", "++"] {
        let mut open: Option<usize> = None;
        let mut cursor = 0usize;
        while let Some(relative) = text[cursor..].find(marker) {
            let index = cursor + relative;
            if marker == "`" && text[index..].starts_with("```") {
                cursor = index + 3;
                continue;
            }
            if marker == "**" && is_exponent_marker(text, index) {
                cursor = index + 2;
                continue;
            }
            open = if open.is_some() { None } else { Some(index) };
            cursor = index + marker.len();
        }
        if let Some(index) = open {
            openings.push(index);
        }
    }

    // A single underscore used as an emphasis opener is provisional. Limit
    // detection to word-boundary-ish positions so identifiers such as foo_bar
    // are not unnecessarily hidden.
    let mut underscore_open: Option<usize> = None;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (position, (index, ch)) in chars.iter().enumerate() {
        if *ch != '_' {
            continue;
        }
        let prev = position
            .checked_sub(1)
            .and_then(|p| chars.get(p))
            .map(|(_, c)| *c);
        let next = chars.get(position + 1).map(|(_, c)| *c);
        let delimiter_like = prev.is_none_or(|c| c.is_whitespace() || "([{>".contains(c))
            || next.is_none_or(|c| c.is_whitespace() || ".,!?;:)]}".contains(c));
        if delimiter_like {
            underscore_open = if underscore_open.is_some() {
                None
            } else {
                Some(*index)
            };
        }
    }
    if let Some(index) = underscore_open {
        openings.push(index);
    }

    // Line-oriented Markdown markers can themselves arrive split across chunks.
    // Keep an otherwise marker-only current line provisional until it becomes
    // a valid heading/divider/list item or ordinary text.
    let line_start = text.rfind('\n').map_or(0, |index| index + 1);
    let current_line = &text[line_start..];
    let leading_ws = current_line.len() - current_line.trim_start().len();
    let marker_start = line_start + leading_ws;
    let marker = current_line.trim();
    let incomplete_heading =
        !marker.is_empty() && marker.chars().all(|ch| ch == '#') && marker.chars().count() <= 6;
    let incomplete_divider = matches!(
        marker,
        "-" | "--" | "*" | "**" | "_" | "__" | "|" | "||" | "~" | "~~"
    );
    let incomplete_quote = matches!(marker, ">" | "**>" | ">>" | ">>>");
    let numeric_list_prefix = marker
        .strip_suffix('.')
        .or_else(|| marker.strip_suffix(')'));
    let incomplete_list = marker == "-"
        || marker == "*"
        || numeric_list_prefix.is_some_and(|prefix| {
            !prefix.is_empty() && prefix.chars().all(|ch| ch.is_ascii_digit())
        });
    if incomplete_heading || incomplete_divider || incomplete_list || incomplete_quote {
        openings.push(marker_start);
    }

    // Incomplete links: keep from `[` provisional until both `](` and `)` are
    // available. Nested link destinations are intentionally treated
    // conservatively rather than attempting a full Markdown grammar here.
    let mut search = 0usize;
    while let Some(rel) = text[search..].find('[') {
        let start = search + rel;
        let rest = &text[start + 1..];
        match rest.find(']') {
            None => {
                openings.push(start);
                break;
            }
            Some(close_rel) => {
                let after_close = start + 1 + close_rel + 1;
                if text[after_close..].starts_with('(') {
                    if let Some(dest_close) = text[after_close + 1..].find(')') {
                        search = after_close + 1 + dest_close + 1;
                    } else {
                        openings.push(start);
                        break;
                    }
                } else {
                    search = after_close;
                }
            }
        }
    }

    // Incomplete HTML media tags: keep unclosed `<img`, `<tg-...`, `<audio` provisional
    // until the matching `>` completes the tag.
    let lower = text.to_ascii_lowercase();
    for tag_prefix in &[
        "<img",
        "<tg-photo",
        "<tg-video",
        "<tg-audio",
        "<tg-document",
        "<tg-map",
        "<tg-collage",
        "<tg-slideshow",
        "<audio",
    ] {
        let mut cursor = 0usize;
        while let Some(rel) = lower[cursor..].find(tag_prefix) {
            let start = cursor + rel;
            if !text[start..].contains('>') {
                openings.push(start);
                break;
            }
            cursor = start + tag_prefix.len();
        }
    }

    openings.into_iter().min()
}

static RE_UNCLOSED_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^\)]*$").expect("valid static regex"));
static RE_DRAFT_HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*#{1,6}\s*").expect("valid static regex"));
static RE_DRAFT_LIST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(?:[-*•]|\d+[.)])\s+").expect("valid static regex"));
static RE_DRAFT_DIVIDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:-{1,}|\*{3,}|_{3,})\s*$").expect("valid static regex")
});
static RE_DRAFT_QUOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(?:\*\*>|>>>|>)\s*").expect("valid static regex"));

/// `**` used as an exponent operator (`a**2`, `(x+1)**3`) rather than as a
/// bold delimiter: an alphanumeric or `)` directly before and a digit directly
/// after. Treating it as bold turned `a**2 + b**2` into `a` + bold `2 + b`.
pub(crate) fn is_exponent_marker(text: &str, index: usize) -> bool {
    let prev = text[..index].chars().next_back();
    let next = text.get(index + 2..).and_then(|tail| tail.chars().next());
    prev.is_some_and(|c| c.is_ascii_alphanumeric() || c == ')')
        && next.is_some_and(|c| c.is_ascii_digit())
}

/// Finds the next `**` in `text` at or after `from` that is a bold delimiter.
fn find_bold_marker(text: &str, from: usize) -> Option<usize> {
    let mut cursor = from;
    while let Some(relative) = text.get(cursor..)?.find("**") {
        let index = cursor + relative;
        if !is_exponent_marker(text, index) {
            return Some(index);
        }
        cursor = index + 2;
    }
    None
}

fn strip_bold_markers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(index) = find_bold_marker(text, cursor) {
        out.push_str(&text[cursor..index]);
        cursor = index + 2;
    }
    out.push_str(&text[cursor..]);
    out
}

fn sanitize_provisional_markdown(tail: &str) -> String {
    let mut safe = strip_bold_markers(&tail.replace("```", ""))
        .replace("__", "")
        .replace("||", "")
        .replace("~~", "")
        .replace("++", "");
    safe = safe.replace('`', "");

    safe = RE_UNCLOSED_LINK.replace_all(&safe, "$1").into_owned();
    safe = RE_DRAFT_HEADING.replace_all(&safe, "").into_owned();
    safe = RE_DRAFT_LIST.replace_all(&safe, "").into_owned();
    safe = RE_DRAFT_DIVIDER.replace_all(&safe, "").into_owned();
    safe = RE_DRAFT_QUOTE.replace_all(&safe, "").into_owned();

    // Remove only obvious unmatched edge delimiters; do not blanket-delete
    // underscores from identifiers or ordinary punctuation.
    let trimmed = safe
        .trim_start_matches(['_', '*', '[', '|', '~', '>'])
        .trim_end_matches(['_', '*', '[', ']', '|', '~', '>']);
    trimmed.to_string()
}

static RE_BLOCK_HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#{1,6})\s*([^\s#].*)$").expect("valid static regex"));
static RE_HTML_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)^<h([1-6])(?:\s+[^>]*)?>(.*?)</h[1-6]>$"#).expect("valid static regex")
});
static RE_HTML_HEADING_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)^<h([1-6])(?:\s+[^>]*)?>"#).expect("valid static regex"));
static RE_BLOCK_DIVIDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\-{3,}|\*{3,}|_{3,}|─{3,}|—{2,})$").expect("valid static regex")
});
static RE_BLOCK_BULLET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-*•]\s+").expect("valid static regex"));
static RE_BLOCK_CHECKBOX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-*•]\s+\[([ xX])\]\s+").expect("valid static regex"));
static RE_BLOCK_NUMBERED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+|[\u0660-\u0669]+)[\.)]\s+").expect("valid static regex"));

fn emit_math_or_quote_blocks(blocks: &mut Vec<RichBlock>, math_lines: &[String]) {
    if math_lines.iter().any(|l| rtl::has_rtl_characters(l)) {
        let clean_lines: Vec<String> = math_lines
            .iter()
            .map(|l| rtl::extract_text_from_pseudo_math(l.trim()))
            .filter(|l| !l.is_empty())
            .collect();
        if !clean_lines.is_empty() {
            let joined = clean_lines.join("\n");
            let lrm_text = rtl::ensure_lrm_if_needed(&joined, false);
            blocks.push(RichBlock::BlockQuotation {
                blocks: vec![json!({
                    "type": "paragraph",
                    "text": parse_inline(&lrm_text)
                })],
            });
        }
    } else {
        for line in math_lines {
            let trimmed_line = line.trim();
            if !trimmed_line.is_empty() {
                let sanitized = sanitize_latex_for_telegram(trimmed_line);
                if !sanitized.is_empty() {
                    blocks.push(RichBlock::MathematicalExpression {
                        expression: sanitized,
                    });
                }
            }
        }
    }
}

pub fn parse_markdown_to_rich_blocks(text: &str) -> Vec<RichBlock> {
    let mut blocks = parse_markdown_blocks(text);
    // Section links and footnotes are resolved against the finished blocks.
    links::resolve(&mut blocks);
    blocks
}

fn push_heading(blocks: &mut Vec<RichBlock>, raw_text: &str, level: usize) {
    blocks.push(RichBlock::SectionHeading {
        text: parse_inline(raw_text),
        level: level.min(6),
    });
}

fn parse_markdown_blocks(text: &str) -> Vec<RichBlock> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let sanitized = sanitize_leaked_llm_artifacts(text);
    if sanitized.trim().is_empty() {
        return Vec::new();
    }

    let is_message_rtl = rtl::is_rtl_text(&sanitized);
    let isolated = isolate_embedded_media_blocks(&sanitized);
    let lines: Vec<String> = isolated
        .replace("\r\n", "\n")
        .split('\n')
        .map(|s| s.to_string())
        .collect();
    let mut blocks: Vec<RichBlock> = Vec::new();

    let mut i = 0;
    let n = lines.len();

    while i < n {
        let line = &lines[i];
        let stripped = line.trim();

        // 1. Skip blank lines
        if stripped.is_empty() {
            i += 1;
            continue;
        }

        // 2. Fenced Code Block (```lang ... ```)
        if let Some(after_fence) = stripped.strip_prefix("```") {
            let lang = after_fence.trim();
            let language = if lang.is_empty() {
                None
            } else {
                Some(lang.to_string())
            };
            let mut code_lines = Vec::new();
            i += 1;
            while i < n && !lines[i].trim().starts_with("```") {
                code_lines.push(lines[i].clone());
                i += 1;
            }
            if i < n && lines[i].trim().starts_with("```") {
                i += 1;
            }
            blocks.push(RichBlock::Preformatted {
                text: code_lines.join("\n"),
                language,
            });
            continue;
        }

        // 3. Math Block ($$...$$ or \[...\])
        if stripped.starts_with("$$") || stripped.starts_with(r"\[") {
            let is_bracket = stripped.starts_with(r"\[");
            let closing_token = if is_bracket { r"\]" } else { "$$" };
            let start_len = 2;
            let mut math_lines = Vec::new();

            if stripped.ends_with(closing_token) && stripped.len() > (start_len * 2) {
                math_lines.push(
                    stripped[start_len..stripped.len() - closing_token.len()]
                        .trim()
                        .to_string(),
                );
                i += 1;
            } else {
                if stripped.len() > start_len {
                    math_lines.push(stripped[start_len..].trim().to_string());
                }
                i += 1;
                while i < n && !lines[i].trim().ends_with(closing_token) {
                    math_lines.push(lines[i].clone());
                    i += 1;
                }
                if i < n && lines[i].trim().ends_with(closing_token) {
                    let end_line = lines[i].trim();
                    if end_line.len() > closing_token.len() {
                        math_lines.push(
                            end_line[..end_line.len() - closing_token.len()]
                                .trim()
                                .to_string(),
                        );
                    }
                    i += 1;
                }
            }

            emit_math_or_quote_blocks(&mut blocks, &math_lines);
            continue;
        }

        // Standalone LaTeX math formula line (\text{...} or \frac{...})
        if (stripped.starts_with(r"\text{")
            || stripped.starts_with(r"\frac")
            || stripped.starts_with(r"\sqrt"))
            && (stripped.contains(r"\frac")
                || stripped.contains('=')
                || stripped.contains(r"\times"))
        {
            let mut math_lines = vec![stripped.to_string()];
            i += 1;
            while i < n {
                let curr_s = lines[i].trim();
                if curr_s.is_empty()
                    || ![
                        r"\frac", r"\text", "=", r"\times", r"\sqrt", "^", "_", "+", "-", "{", "}",
                    ]
                    .iter()
                    .any(|k| curr_s.contains(k))
                {
                    break;
                }
                math_lines.push(curr_s.to_string());
                i += 1;
            }
            emit_math_or_quote_blocks(&mut blocks, &math_lines);
            continue;
        }

        // Map Block ([map: lat, lon] or <tg-map .../>)
        if stripped.starts_with("[map:")
            || stripped.starts_with("[location:")
            || stripped.starts_with("![map]")
            || stripped.starts_with("![location]")
            || stripped.starts_with("<tg-map")
        {
            if let Some(map_block) = try_parse_map_block(stripped) {
                blocks.push(map_block);
                i += 1;
                continue;
            }
        }

        // Document Block ([document: name](tg://...) or <tg-document .../>)
        if stripped.starts_with("[document:") || stripped.starts_with("<tg-document") {
            if let Some(doc_block) = try_parse_doc_block(stripped) {
                blocks.push(doc_block);
                i += 1;
                continue;
            }
        }

        // Multi-line or container Media Block (<tg-collage>...</tg-collage>, etc.)
        if stripped.starts_with("<tg-collage")
            || stripped.starts_with("<tg-slideshow")
            || (stripped.starts_with("[collage") && !stripped.contains('('))
            || (stripped.starts_with("[kolase") && !stripped.contains('('))
            || (stripped.starts_with("[slideshow") && !stripped.contains('('))
        {
            if let Some((container_block, next_i)) = try_parse_container_media_block(&lines, i) {
                blocks.push(container_block);
                i = next_i;
                continue;
            }
        }

        // Media Block (Photo, Video, Audio, VoiceNote, Animation, Collage, Slideshow)
        if let Some(media_block) = try_parse_media_block(stripped) {
            blocks.push(media_block);
            i += 1;
            continue;
        }

        // 4. Horizontal Divider (---, ***, ___, ───, or <hr>, <hr/>)
        if RE_BLOCK_DIVIDER.is_match(stripped)
            || stripped.eq_ignore_ascii_case("<hr>")
            || stripped.eq_ignore_ascii_case("<hr/>")
            || stripped.eq_ignore_ascii_case("<hr />")
        {
            blocks.push(RichBlock::Divider {});
            i += 1;
            continue;
        }

        // 5a. HTML Heading (<h1>...</h1> to <h6>...</h6>)
        if let Some(caps) = RE_HTML_HEADING_START.captures(stripped) {
            let level = caps
                .get(1)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(1);
            let after_open = &stripped[caps.get(0).map_or(0, |m| m.end())..];
            let close_tag = format!("</h{level}>");

            if let Some(end) = after_open.to_ascii_lowercase().rfind(&close_tag) {
                push_heading(&mut blocks, after_open[..end].trim(), level);
                i += 1;
                continue;
            }
            if let Some(end) = after_open.to_ascii_lowercase().rfind("</h") {
                if let Some(_gt) = after_open[end..].find('>') {
                    push_heading(&mut blocks, after_open[..end].trim(), level);
                    i += 1;
                    continue;
                }
            }

            // Multi-line heading
            let mut h_lines = Vec::new();
            if !after_open.trim().is_empty() {
                h_lines.push(after_open.trim().to_string());
            }
            i += 1;
            while i < n {
                let line_str = lines[i].trim();
                let lower = line_str.to_ascii_lowercase();
                if let Some(end) = lower.rfind(&close_tag) {
                    let before = line_str[..end].trim();
                    if !before.is_empty() {
                        h_lines.push(before.to_string());
                    }
                    i += 1;
                    break;
                } else if let Some(end) = lower.rfind("</h") {
                    if let Some(_gt) = lower[end..].find('>') {
                        let before = line_str[..end].trim();
                        if !before.is_empty() {
                            h_lines.push(before.to_string());
                        }
                        i += 1;
                        break;
                    }
                }
                if !line_str.is_empty() {
                    h_lines.push(line_str.to_string());
                }
                i += 1;
            }
            push_heading(&mut blocks, &h_lines.join(" "), level);
            continue;
        }

        // 5b. HTML Paragraph (<p>...</p>)
        if stripped.starts_with("<p>") || stripped.starts_with("<p ") {
            let mut p_lines = Vec::new();
            let mut curr = stripped.to_string();
            if let Some(pos) = curr.find('>') {
                curr = curr[pos + 1..].to_string();
            }
            if let Some(end) = curr.rfind("</p>") {
                let inner = curr[..end].trim();
                if !inner.is_empty() {
                    let lrm_text = rtl::ensure_lrm_if_needed(inner, is_message_rtl);
                    blocks.push(RichBlock::Paragraph {
                        text: parse_inline(&lrm_text),
                    });
                }
                i += 1;
                continue;
            }
            if !curr.trim().is_empty() {
                p_lines.push(curr.trim().to_string());
            }
            i += 1;
            while i < n {
                let line_str = lines[i].trim();
                if let Some(end) = line_str.rfind("</p>") {
                    let before = line_str[..end].trim();
                    if !before.is_empty() {
                        p_lines.push(before.to_string());
                    }
                    i += 1;
                    break;
                }
                if !line_str.is_empty() {
                    p_lines.push(line_str.to_string());
                }
                i += 1;
            }
            if !p_lines.is_empty() {
                let joined = p_lines.join(" ");
                let lrm_text = rtl::ensure_lrm_if_needed(&joined, is_message_rtl);
                blocks.push(RichBlock::Paragraph {
                    text: parse_inline(&lrm_text),
                });
            }
            continue;
        }

        // 5. Section Heading (# Heading, ## Subheading, etc.)
        if let Some(caps) = RE_BLOCK_HEADING.captures(stripped) {
            let level = caps.get(1).map(|m| m.as_str().len()).unwrap_or(1);
            let heading_text = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            push_heading(&mut blocks, heading_text, level);
            i += 1;
            continue;
        }

        // 5c. Footnote definition ([^1]: note). A note may also start on
        // the next line; a definition without any note stays plain text.
        if let Some((id, note)) = links::footnote_definition(stripped) {
            let next_line = lines.get(i + 1).map(|line| line.trim()).unwrap_or("");
            if !note.is_empty() {
                blocks.push(links::footnote_note_block(id, parse_inline(note)));
                i += 1;
                continue;
            }
            if !next_line.is_empty() && links::footnote_definition(next_line).is_none() {
                blocks.push(links::footnote_note_block(id, parse_inline(next_line)));
                i += 2;
                continue;
            }
        }

        // 6a. Pullquote (>>> quote)
        if stripped.starts_with(">>>") {
            let mut quote_lines = Vec::new();
            while i < n && lines[i].trim().starts_with(">>>") {
                let q = lines[i].trim();
                let stripped_q = q.strip_prefix(">>>").unwrap_or(q).trim_start();
                quote_lines.push(stripped_q.to_string());
                i += 1;
            }
            let (quote_text, credit) = extract_quote_credit(&quote_lines);
            blocks.push(RichBlock::PullQuotation {
                text: parse_inline(&quote_text),
                credit: credit.map(|c| parse_inline(&c)),
            });
            continue;
        }

        // 6b. Expandable Blockquote (**> quote or <blockquote expandable>)
        if stripped.starts_with("**>") {
            let mut quote_lines = Vec::new();
            while i < n {
                let curr_stripped = lines[i].trim();
                if curr_stripped.starts_with("**>") {
                    quote_lines.push(
                        curr_stripped
                            .strip_prefix("**>")
                            .unwrap_or(curr_stripped)
                            .trim_start()
                            .to_string(),
                    );
                    i += 1;
                } else if curr_stripped.starts_with('>') && !curr_stripped.starts_with(">>>") {
                    quote_lines.push(
                        curr_stripped
                            .strip_prefix('>')
                            .unwrap_or(curr_stripped)
                            .trim_start()
                            .to_string(),
                    );
                    i += 1;
                } else {
                    break;
                }
            }
            let (quote_text, credit) = extract_quote_credit(&quote_lines);
            blocks.push(RichBlock::ExpandableBlockQuotation {
                text: parse_inline(&quote_text),
                credit: credit.map(|c| parse_inline(&c)),
            });
            continue;
        }

        if stripped.starts_with("<blockquote") && stripped.contains("expandable") {
            let mut quote_lines = Vec::new();
            let mut first_line = stripped.to_string();
            if let Some(pos) = first_line.find('>') {
                first_line = first_line[pos + 1..].to_string();
            }
            if let Some(end) = first_line.find("</blockquote>") {
                let inner = first_line[..end].trim();
                let (quote_text, credit) = extract_html_cite(inner);
                blocks.push(RichBlock::ExpandableBlockQuotation {
                    text: parse_inline(&quote_text),
                    credit: credit.map(|c| parse_inline(&c)),
                });
                i += 1;
                continue;
            }
            if !first_line.trim().is_empty() {
                quote_lines.push(first_line.trim().to_string());
            }
            i += 1;
            while i < n {
                let curr = lines[i].trim();
                if let Some(end) = curr.find("</blockquote>") {
                    let before = curr[..end].trim();
                    if !before.is_empty() {
                        quote_lines.push(before.to_string());
                    }
                    i += 1;
                    break;
                }
                quote_lines.push(curr.to_string());
                i += 1;
            }
            let (quote_text, credit) = extract_quote_credit(&quote_lines);
            blocks.push(RichBlock::ExpandableBlockQuotation {
                text: parse_inline(&quote_text),
                credit: credit.map(|c| parse_inline(&c)),
            });
            continue;
        }

        // 6c. HTML Blockquote (<blockquote> ... </blockquote>)
        if stripped.starts_with("<blockquote") && !stripped.contains("expandable") {
            let mut quote_lines = Vec::new();
            let mut first_line = stripped.to_string();
            if let Some(pos) = first_line.find('>') {
                first_line = first_line[pos + 1..].to_string();
            }
            if let Some(end) = first_line.find("</blockquote>") {
                let inner = first_line[..end].trim();
                let lrm_text = rtl::ensure_lrm_if_needed(inner, is_message_rtl);
                blocks.push(RichBlock::BlockQuotation {
                    blocks: vec![json!({
                        "type": "paragraph",
                        "text": parse_inline(&lrm_text)
                    })],
                });
                i += 1;
                continue;
            }
            if !first_line.trim().is_empty() {
                quote_lines.push(first_line.trim().to_string());
            }
            i += 1;
            while i < n {
                let curr = lines[i].trim();
                if let Some(end) = curr.find("</blockquote>") {
                    let before = curr[..end].trim();
                    if !before.is_empty() {
                        quote_lines.push(before.to_string());
                    }
                    i += 1;
                    break;
                }
                quote_lines.push(curr.to_string());
                i += 1;
            }
            let joined = quote_lines.join("\n");
            let lrm_text = rtl::ensure_lrm_if_needed(&joined, is_message_rtl);
            blocks.push(RichBlock::BlockQuotation {
                blocks: vec![json!({
                    "type": "paragraph",
                    "text": parse_inline(&lrm_text)
                })],
            });
            continue;
        }

        // 6. Blockquote (> quote)
        if stripped.starts_with('>') {
            let mut quote_lines = Vec::new();
            while i < n && lines[i].trim().starts_with('>') && !lines[i].trim().starts_with(">>>") {
                let q = lines[i].trim();
                let stripped_q = q.strip_prefix('>').unwrap_or(q).trim_start();
                quote_lines.push(stripped_q.to_string());
                i += 1;
            }
            if let Some(first) = quote_lines.first_mut() {
                let alerts = [
                    ("[!NOTE]", "ℹ️ **Catatan:**"),
                    ("[!note]", "ℹ️ **Catatan:**"),
                    ("[!TIP]", "💡 **Tips:**"),
                    ("[!tip]", "💡 **Tips:**"),
                    ("[!IMPORTANT]", "📌 **Penting:**"),
                    ("[!important]", "📌 **Penting:**"),
                    ("[!WARNING]", "⚠️ **Peringatan:**"),
                    ("[!warning]", "⚠️ **Peringatan:**"),
                    ("[!CAUTION]", "🚨 **Perhatian:**"),
                    ("[!caution]", "🚨 **Perhatian:**"),
                ];
                for (marker, replacement) in alerts {
                    if first.starts_with(marker) {
                        let rest = first[marker.len()..].trim();
                        if rest.is_empty() {
                            *first = replacement.to_string();
                        } else {
                            *first = format!("{replacement} {rest}");
                        }
                        break;
                    }
                }
            }
            let joined = quote_lines.join("\n");
            let lrm_text = rtl::ensure_lrm_if_needed(&joined, is_message_rtl);
            blocks.push(RichBlock::BlockQuotation {
                blocks: vec![json!({
                    "type": "paragraph",
                    "text": parse_inline(&lrm_text)
                })],
            });
            continue;
        }

        // 7. Table (Markdown, Unicode, ASCII, Underline)
        if let Some(inner) = stripped
            .strip_prefix("[table:")
            .or_else(|| stripped.strip_prefix("[caption:"))
            .and_then(|r| r.strip_suffix(']'))
        {
            let cap = inner.trim();
            if !cap.is_empty() && i + 1 < n {
                let (t_cells, has_hdr, next_i) = try_parse_table(&lines, i + 1, is_message_rtl);
                if let Some(cells) = t_cells {
                    blocks.push(RichBlock::Table {
                        cells,
                        has_header: has_hdr,
                        is_bordered: false,
                        is_striped: false,
                        is_compact: true,
                        caption: Some(cap.to_string()),
                    });
                    i = next_i;
                    continue;
                }
            }
        }

        let (t_cells, has_hdr, next_i) = try_parse_table(&lines, i, is_message_rtl);
        if let Some(cells) = t_cells {
            let mut caption: Option<String> = None;
            let mut final_next_i = next_i;
            if final_next_i < n {
                let next_line = lines[final_next_i].trim();
                if let Some(inner) = next_line
                    .strip_prefix("[table:")
                    .or_else(|| next_line.strip_prefix("[caption:"))
                    .and_then(|r| r.strip_suffix(']'))
                {
                    let cap = inner.trim();
                    if !cap.is_empty() {
                        caption = Some(cap.to_string());
                        final_next_i += 1;
                    }
                }
            }
            blocks.push(RichBlock::Table {
                cells,
                has_header: has_hdr,
                is_bordered: false,
                is_striped: false,
                is_compact: true,
                caption,
            });
            i = final_next_i;
            continue;
        }

        // 8. List Items (- item, * item, - [ ] checkbox, 1. item)
        let is_checkbox = RE_BLOCK_CHECKBOX.is_match(stripped);
        let is_bullet = !is_checkbox && RE_BLOCK_BULLET.is_match(stripped);
        let is_numbered = RE_BLOCK_NUMBERED.is_match(stripped);

        if is_checkbox || is_bullet || is_numbered {
            let mut list_items = Vec::new();
            let is_ordered = is_numbered;
            let is_task_list = is_checkbox;

            while i < n {
                let curr = lines[i].trim();
                if curr.is_empty() {
                    break;
                }
                if is_ordered && RE_BLOCK_NUMBERED.is_match(curr) {
                    let item_text = RE_BLOCK_NUMBERED.replace(curr, "").trim().to_string();
                    let value = curr.split_once(['.', ')']).and_then(|(prefix, _)| {
                        let prefix_clean = prefix.trim();
                        let ascii = rtl::from_eastern_arabic_digits(prefix_clean);
                        ascii
                            .parse::<i64>()
                            .ok()
                            .or_else(|| prefix_clean.parse::<i64>().ok())
                    });
                    let lrm_text = rtl::ensure_lrm_if_needed(&item_text, is_message_rtl);
                    list_items.push(RichBlockListItem::ordered(
                        vec![json!({
                            "type": "paragraph",
                            "text": parse_inline(&lrm_text)
                        })],
                        value,
                    ));
                    i += 1;
                } else if is_task_list && RE_BLOCK_CHECKBOX.is_match(curr) {
                    let is_checked = RE_BLOCK_CHECKBOX
                        .captures(curr)
                        .and_then(|caps| caps.get(1))
                        .map(|m| m.as_str().eq_ignore_ascii_case("x"))
                        .unwrap_or(false);
                    let item_text = RE_BLOCK_CHECKBOX.replace(curr, "").trim().to_string();
                    let lrm_text = rtl::ensure_lrm_if_needed(&item_text, is_message_rtl);
                    list_items.push(RichBlockListItem::checkbox(
                        vec![json!({
                            "type": "paragraph",
                            "text": parse_inline(&lrm_text)
                        })],
                        is_checked,
                    ));
                    i += 1;
                } else if !is_ordered
                    && !is_task_list
                    && RE_BLOCK_BULLET.is_match(curr)
                    && !RE_BLOCK_CHECKBOX.is_match(curr)
                {
                    let item_text = RE_BLOCK_BULLET.replace(curr, "").trim().to_string();
                    let lrm_text = rtl::ensure_lrm_if_needed(&item_text, is_message_rtl);
                    list_items.push(RichBlockListItem::bullet(vec![json!({
                        "type": "paragraph",
                        "text": parse_inline(&lrm_text)
                    })]));
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(RichBlock::List { items: list_items });
            continue;
        }

        // 9. Regular Paragraph
        let mut para_lines = Vec::new();
        while i < n {
            let curr = &lines[i];
            let s_curr = curr.trim();
            if s_curr.is_empty()
                || s_curr.starts_with("```")
                || s_curr.starts_with("$$")
                || s_curr.starts_with(r"\[")
                || RE_BLOCK_HEADING.is_match(s_curr)
                || RE_HTML_HEADING.is_match(s_curr)
                || links::footnote_definition(s_curr).is_some()
                || s_curr.starts_with("<p>")
                || s_curr.starts_with("<p ")
                || s_curr.eq_ignore_ascii_case("<hr>")
                || s_curr.eq_ignore_ascii_case("<hr/>")
                || s_curr.eq_ignore_ascii_case("<hr />")
                || s_curr.starts_with("**>")
                || s_curr.starts_with("<blockquote")
                || s_curr.starts_with('>')
                || s_curr.starts_with(">>>")
                || s_curr.starts_with("[table:")
                || s_curr.starts_with("[caption:")
                || s_curr.starts_with("<tg-map")
                || s_curr.starts_with("<tg-document")
                || s_curr.starts_with("<tg-collage")
                || s_curr.starts_with("<tg-slideshow")
                || s_curr.starts_with("<audio")
                || s_curr.starts_with("<tg-audio")
                || s_curr.starts_with("<img")
                || s_curr.starts_with("<tg-photo")
                || try_parse_media_block(s_curr).is_some()
                || try_parse_doc_block(s_curr).is_some()
                || try_parse_map_block(s_curr).is_some()
                || RE_BLOCK_BULLET.is_match(s_curr)
                || RE_BLOCK_NUMBERED.is_match(s_curr)
                || RE_BLOCK_DIVIDER.is_match(s_curr)
                || try_parse_table(&lines, i, is_message_rtl).0.is_some()
            {
                break;
            }
            para_lines.push(curr.clone());
            i += 1;
        }

        if !para_lines.is_empty() {
            let joined = para_lines.join("\n");
            let lrm_text = rtl::ensure_lrm_if_needed(&joined, is_message_rtl);
            blocks.push(RichBlock::Paragraph {
                text: parse_inline(&lrm_text),
            });
        } else if i < n {
            // Defensive loop termination guard: if a line matched a paragraph break
            // delimiter but was rejected by every specific block parser, consume it
            // as a standalone fallback paragraph line so `i` always advances.
            let fallback_line = &lines[i];
            let lrm_text = rtl::ensure_lrm_if_needed(fallback_line, is_message_rtl);
            blocks.push(RichBlock::Paragraph {
                text: parse_inline(&lrm_text),
            });
            i += 1;
        }
    }

    blocks
}

pub fn build_full_rich_message(answer_text: &str, footer_text: Option<&str>) -> InputRichMessage {
    let mut blocks = parse_markdown_to_rich_blocks(answer_text);
    if blocks.is_empty() {
        let is_msg_rtl = rtl::is_rtl_text(answer_text);
        let lrm_text = rtl::ensure_lrm_if_needed(answer_text.trim(), is_msg_rtl);
        blocks.push(RichBlock::Paragraph {
            text: parse_inline(&lrm_text),
        });
    }
    if let Some(footer) = footer_text.map(str::trim).filter(|m| !m.is_empty()) {
        blocks.push(RichBlock::Footer {
            text: parse_inline(footer),
        });
    }
    let mut message = InputRichMessage::new(blocks);
    rtl::apply_rtl_direction(&mut message, answer_text);
    message
}

/// Validates container tags and geographic map parameters within rich HTML.
#[allow(dead_code)]
fn validate_rich_html_containers(html: &str) -> Result<(), ParserError> {
    static RE_COLLAGE_CONTAINER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)<tg-collage(?:\s+[^>]*)?>(.*?)</tg-collage>"#)
            .expect("valid static regex")
    });

    for caps in RE_COLLAGE_CONTAINER.captures_iter(html) {
        let content = caps.get(1).map_or("", |m| m.as_str());
        let lower = content.to_lowercase();
        if lower.contains("<audio")
            || lower.contains("<tg-audio")
            || lower.contains("<tg-document")
            || lower.contains("<document")
        {
            return Err(ParserError::InvalidTag(
                "Collage album cannot mix audio or documents with visual items".to_string(),
            ));
        }
    }

    static RE_MAP_TAG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)<tg-map(?:\s+[^>]*)?/?>"#).expect("valid static regex")
    });

    for caps in RE_MAP_TAG.captures_iter(html) {
        let tag = caps.get(0).map_or("", |m| m.as_str());
        let lat_s = extract_html_attribute(tag, "lat").ok_or_else(|| {
            ParserError::InvalidCoordinate("Missing 'lat' attribute on <tg-map>".to_string())
        })?;
        let lon_s = extract_html_attribute(tag, "lon").ok_or_else(|| {
            ParserError::InvalidCoordinate("Missing 'lon' attribute on <tg-map>".to_string())
        })?;
        let lat = lat_s.parse::<f64>().map_err(|_| {
            ParserError::InvalidCoordinate(format!("Invalid latitude float value: '{lat_s}'"))
        })?;
        let lon = lon_s.parse::<f64>().map_err(|_| {
            ParserError::InvalidCoordinate(format!("Invalid longitude float value: '{lon_s}'"))
        })?;

        if !lat.is_finite() || !lon.is_finite() {
            return Err(ParserError::InvalidCoordinate(
                "Geo coordinates must be finite numbers".to_string(),
            ));
        }
        if !(-90.0..=90.0).contains(&lat) {
            return Err(ParserError::InvalidCoordinate(format!(
                "Latitude {lat} out of range [-90.0, 90.0]"
            )));
        }
        if !(-180.0..=180.0).contains(&lon) {
            return Err(ParserError::InvalidCoordinate(format!(
                "Longitude {lon} out of range [-180.0, 180.0]"
            )));
        }

        if let Some(zoom_s) = extract_html_attribute(tag, "zoom") {
            let zoom = zoom_s.parse::<i32>().map_err(|_| {
                ParserError::InvalidCoordinate(format!("Invalid zoom integer value: '{zoom_s}'"))
            })?;
            if !(1..=20).contains(&zoom) {
                return Err(ParserError::InvalidCoordinate(format!(
                    "Zoom level {zoom} out of range [1, 20]"
                )));
            }
        }
    }

    Ok(())
}

/// Extracts and validates media items referenced in HTML tags, resolving their IDs and metadata.
#[allow(dead_code)]
pub fn extract_rich_html_media(html: &str) -> Result<Vec<InputRichMessageMedia>, ParserError> {
    static RE_HTML_MEDIA_TAGS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)<(img|tg-photo|audio|tg-audio|video|tg-video|tg-document|document)(?:\s+[^>]*?)(?:/>|>.*?</(?:img|tg-photo|audio|tg-audio|video|tg-video|tg-document|document)>|>)"#)
            .expect("valid static regex")
    });

    let mut media_items: Vec<InputRichMessageMedia> = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut counter: usize = 1;

    for caps in RE_HTML_MEDIA_TAGS.captures_iter(html) {
        let tag_match = caps.get(0).map_or("", |m| m.as_str());
        let tag_name = caps.get(1).map_or("", |m| m.as_str()).to_lowercase();

        let src = match extract_html_attribute(tag_match, "src") {
            Some(s) if !s.trim().is_empty() => s.trim(),
            _ => continue,
        };

        let is_photo = matches!(tag_name.as_str(), "img" | "tg-photo");
        let is_audio = matches!(tag_name.as_str(), "audio" | "tg-audio");
        let is_video = matches!(tag_name.as_str(), "video" | "tg-video");
        let is_doc = matches!(tag_name.as_str(), "tg-document" | "document");

        let id = if let Some(stripped) = src.strip_prefix("tg://") {
            let (scheme_type, query) = stripped.split_once('?').unwrap_or((stripped, ""));
            let extracted_id = if let Some(id_val) = query.strip_prefix("id=") {
                id_val.split('&').next().unwrap_or(id_val)
            } else if let Some((_, val)) = query
                .split('&')
                .filter_map(|p| p.split_once('='))
                .find(|(k, _)| *k == "id")
            {
                val
            } else {
                ""
            };

            if extracted_id.is_empty() {
                return Err(ParserError::InvalidTag(format!(
                    "Missing ID in tg:// scheme: '{src}'"
                )));
            }

            if (is_photo && scheme_type != "photo")
                || (is_audio && scheme_type != "audio")
                || (is_video && scheme_type != "video")
                || (is_doc && scheme_type != "document")
            {
                return Err(ParserError::InvalidTag(format!(
                    "Tag <{tag_name}> cannot reference scheme 'tg://{scheme_type}'"
                )));
            }

            InputRichMessageMedia::validate_id(extracted_id)
                .map_err(ParserError::MediaValidation)?;
            extracted_id.to_string()
        } else if let Some(key) = src.strip_prefix("attach://") {
            let extracted_id = extract_html_attribute(tag_match, "id").unwrap_or(key);
            InputRichMessageMedia::validate_id(extracted_id)
                .map_err(ParserError::MediaValidation)?;
            extracted_id.to_string()
        } else {
            if let Some(explicit_id) = extract_html_attribute(tag_match, "id") {
                InputRichMessageMedia::validate_id(explicit_id)
                    .map_err(ParserError::MediaValidation)?;
                explicit_id.to_string()
            } else {
                let prefix = if is_photo {
                    "photo"
                } else if is_audio {
                    "audio"
                } else if is_video {
                    "video"
                } else {
                    "doc"
                };
                let generated = format!("{prefix}_{counter}");
                counter += 1;
                generated
            }
        };

        let caption = extract_html_attribute(tag_match, "caption")
            .or_else(|| {
                if is_photo {
                    extract_html_attribute(tag_match, "alt")
                } else {
                    None
                }
            })
            .map(str::to_string);

        let input_media = if is_photo {
            InputMedia::photo(src, caption, None)
        } else if is_audio {
            let title = extract_html_attribute(tag_match, "title").map(str::to_string);
            let performer = extract_html_attribute(tag_match, "performer").map(str::to_string);
            InputMedia::audio(src, caption, None, title, performer)
        } else if is_video {
            InputMedia::video(src, caption, None)
        } else if is_doc {
            InputMedia::document(src, caption, None)
        } else {
            continue;
        };

        let media_item = InputRichMessageMedia {
            id: id.clone(),
            media: input_media,
        };
        media_item
            .validate()
            .map_err(ParserError::MediaValidation)?;

        if seen_ids.insert(id.clone()) {
            if media_items.len() >= 50 {
                return Err(ParserError::MediaValidation(
                    "Rich Message media count exceeds Telegram limit of 50".to_string(),
                ));
            }
            media_items.push(media_item);
        } else if let Some(existing) = media_items.iter().find(|m| m.id == id) {
            if existing.media.media_url() != src {
                return Err(ParserError::MediaValidation(format!(
                    "Duplicate media ID found with conflicting URL: '{id}'"
                )));
            }
        }
    }

    Ok(media_items)
}

/// Normalizes HTML blocks by ensuring block-level tags reside on separate lines.
#[allow(dead_code)]
fn normalize_html_blocks(html: &str) -> String {
    static RE_NORM_BLOCKS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)(</?(?:p|h[1-6]|blockquote|pre|table|hr|tg-collage|tg-slideshow|tg-map|audio|tg-audio|img|tg-photo|video|tg-video|tg-document|document)(?:\s+[^>]*)?/?>)"#)
            .expect("valid static regex")
    });

    let mut result = String::with_capacity(html.len() + 128);
    let mut last_end = 0;
    // Computed once: lowercasing the whole document per matched tag made this
    // loop quadratic in the input size.
    let html_lower = html.to_ascii_lowercase();
    let has_audio_close = html_lower.contains("</audio>");
    let has_tg_audio_close = html_lower.contains("</tg-audio>");

    for mat in RE_NORM_BLOCKS.find_iter(html) {
        let text_before = &html[last_end..mat.start()];
        let tag = mat.as_str().trim();

        if !text_before.trim().is_empty() {
            result.push_str(text_before);
        }

        let is_closing = tag.starts_with("</");
        let tag_lower = tag.to_ascii_lowercase();
        let is_self_closing = tag.ends_with("/>")
            || tag_lower == "<hr>"
            || tag_lower.starts_with("<hr ")
            || tag_lower.starts_with("<img")
            || tag_lower.starts_with("<tg-photo")
            || tag_lower.starts_with("<tg-map")
            || (tag_lower.starts_with("<audio") && !has_audio_close)
            || (tag_lower.starts_with("<tg-audio") && !has_tg_audio_close);

        if is_closing {
            result.push_str(tag);
            result.push_str("\n\n");
        } else if is_self_closing {
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(tag);
            result.push_str("\n\n");
        } else {
            // Opening block tag
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(tag);
        }

        last_end = mat.end();
    }

    let remaining = &html[last_end..];
    if !remaining.trim().is_empty() {
        result.push_str(remaining);
    }

    result
}

/// Helper function to parse HTML rich message representation with media resolution.
/// Parses `<img>`, `<audio>`, `<tg-collage>`, `<tg-slideshow>`, `<tg-map>` as well as
/// HTML typography tags into `Vec<RichBlock>` and resolves/extracts `Vec<InputRichMessageMedia>`.
#[allow(dead_code)]
pub fn parse_rich_html(
    html: &str,
) -> Result<(Vec<RichBlock>, Vec<InputRichMessageMedia>), ParserError> {
    let trimmed = html.trim();
    if trimmed.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    if trimmed.chars().count() > 32_768 {
        return Err(ParserError::MalformedHtml(
            "Rich Message HTML text exceeds Telegram limit of 32768 characters".to_string(),
        ));
    }

    // 1. Validate container integrity and geographic map parameters
    validate_rich_html_containers(trimmed)?;

    // 2. Extract and resolve media objects
    let media = extract_rich_html_media(trimmed)?;

    // 3. Normalize HTML blocks and parse to RichBlocks
    let normalized = normalize_html_blocks(trimmed);
    let mut blocks = parse_markdown_to_rich_blocks(&normalized);

    // Normalize voice_note wire discriminators
    for block in &mut blocks {
        if let RichBlock::VoiceNote { voice_note, .. } = block {
            if let Some(object) = voice_note.as_object_mut() {
                object.insert("type".to_string(), Value::String("voice_note".to_string()));
            }
        }
    }

    if blocks.len() > 500 {
        return Err(ParserError::MalformedHtml(format!(
            "Rich Message contains {} blocks; Telegram limit is 500",
            blocks.len()
        )));
    }

    Ok((blocks, media))
}

/// Resolves media references in `blocks` against `media_items`, replacing `tg://` scheme URIs
/// with their actual target media URLs.
#[allow(dead_code)]
pub fn resolve_media_references(blocks: &mut [RichBlock], media_items: &[InputRichMessageMedia]) {
    let map: std::collections::HashMap<&str, &str> = media_items
        .iter()
        .map(|item| (item.id.as_str(), item.media.media_url()))
        .collect();

    for block in blocks {
        block.replace_media_urls(&|url| {
            for prefix in &[
                "tg://photo?id=",
                "tg://audio?id=",
                "tg://video?id=",
                "tg://document?id=",
            ] {
                if let Some(id) = url.strip_prefix(prefix) {
                    if let Some(target_url) = map.get(id) {
                        return Some(target_url.to_string());
                    }
                }
            }
            None
        });
    }
}

#[cfg(test)]
#[path = "markdown/tests.rs"]
mod tests;
