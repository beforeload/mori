//! Parsing of memo content: tags, memo references, links, flomo inline markup
//! (`**bold**`, `==highlight==`, `__underline__`, `~~strike~~`, `` `code` ``),
//! and a Markdown-compatible block layer (headings, fenced code, tables, rules,
//! task lists). Headings need a space after `#` (`# Title`); tags do not (`#tag`),
//! so the two do not collide.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#[^\s#]+").unwrap());
static REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[\[memo:([A-Za-z0-9_-]+)\]\]|(?:mindflow|mori)://memo/([A-Za-z0-9_-]+)").unwrap()
});
static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s<>"'，。；！？、）】」』《》]+"#).unwrap());
static ORDERED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,3})[.、]\s").unwrap());

const TAG_TRAILING: &[char] = &[
    ',', '.', ';', ':', '!', '?', '，', '。', '；', '：', '！', '？', '、', ')', '）', ']', '】', '}',
    '」', '』', '》', '"', '\'', '”', '’', '/', '*', '=', '~',
];
/// Full-width punctuation ends a tag even mid-word: `#读书（上）` is `#读书`.
const TAG_STOP: &[char] = &['，', '。', '；', '：', '！', '？', '、', '（', '）', '【', '】', '「', '」', '『', '』', '《', '》'];

/// Byte ranges of fenced code blocks and inline `code` spans, where `#` is not a tag.
fn code_ranges(content: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut fence_start: Option<usize> = None;
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            match fence_start.take() {
                Some(start) => ranges.push(start..offset + line.len()),
                None => fence_start = Some(offset),
            }
        } else if fence_start.is_none() {
            let mut open: Option<usize> = None;
            for (i, _) in line.match_indices('`') {
                match open.take() {
                    Some(start) => ranges.push(offset + start..offset + i + 1),
                    None => open = Some(i),
                }
            }
        }
        offset += line.len();
    }
    if let Some(start) = fence_start {
        ranges.push(start..content.len());
    }
    ranges
}

pub fn ref_token(id: &str) -> String {
    format!("[[memo:{id}]]")
}

/// Byte ranges of every `#tag` (including the `#`) together with the tag name.
pub fn wikilink_targets(content: &str) -> Vec<String> {
    super::frontmatter::note_links(content)
}

pub fn tag_spans(content: &str) -> Vec<(Range<usize>, String)> {
    let body_at = super::frontmatter::body_start(content);
    let ref_ranges: Vec<Range<usize>> = REF_RE.find_iter(content).map(|m| m.range()).collect();
    let url_ranges: Vec<Range<usize>> = URL_RE.find_iter(content).map(|m| m.range()).collect();
    let code = code_ranges(content);
    let mut spans = Vec::new();
    for m in TAG_RE.find_iter(content) {
        let start = m.start();
        if start < body_at {
            continue;
        }
        // flomo writes bold tags as `**#tag**`: markup markers may sit between the space and `#`.
        let before = content[..start].trim_end_matches(['*', '_', '=', '~']);
        if before.chars().next_back().is_some_and(|c| !c.is_whitespace()) {
            continue;
        }
        if ref_ranges.iter().chain(url_ranges.iter()).chain(code.iter()).any(|r| r.contains(&start)) {
            continue;
        }
        let raw = &content[start + 1..m.end()];
        let raw = raw.split(TAG_STOP).next().unwrap_or("");
        let name = raw.trim_end_matches(TAG_TRAILING);
        // Like Obsidian, `#2` or `#000` (list numbers, issue ids, colours) is not a tag.
        if name.is_empty() || name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        spans.push((start..start + 1 + name.len(), name.to_string()));
    }
    spans
}

pub fn extract_tags(content: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for (_, tag) in tag_spans(content) {
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    for tag in super::frontmatter::tags(content) {
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

pub fn extract_refs(content: &str) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    for caps in REF_RE.captures_iter(content) {
        if let Some(id) = caps.get(1).or_else(|| caps.get(2)) {
            let id = id.as_str().to_string();
            if !refs.contains(&id) {
                refs.push(id);
            }
        }
    }
    refs
}

pub fn ref_spans(content: &str) -> Vec<(Range<usize>, String)> {
    REF_RE
        .captures_iter(content)
        .filter_map(|caps| {
            let whole = caps.get(0)?;
            let id = caps.get(1).or_else(|| caps.get(2))?;
            Some((whole.range(), id.as_str().to_string()))
        })
        .collect()
}

pub fn has_link(content: &str) -> bool {
    URL_RE.is_match(content)
}

/// Content with markup removed, used for counting, snippets and vectors.
pub fn plain_text(content: &str) -> String {
    let source = super::images::strip(super::frontmatter::body(content));
    let without_refs = REF_RE.replace_all(&source, "");
    let mut out = String::with_capacity(without_refs.len());
    for line in without_refs.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || is_table_separator(trimmed) {
            continue;
        }
        let (kind, rest) = split_line_kind(line);
        if kind == LineKind::Rule {
            continue;
        }
        if trimmed.starts_with('|') {
            out.push_str(&strip_markers(&rest.replace('|', " ")));
        } else {
            out.push_str(&strip_markers(rest));
        }
        out.push('\n');
    }
    let mut text = out.trim().to_string();
    if let Some(title) = super::frontmatter::title(content)
        && !text.contains(&title)
    {
        text = if text.is_empty() { title } else { format!("{title}\n{text}") };
    }
    text
}

pub fn plain_char_count(content: &str) -> usize {
    plain_text(content).chars().filter(|c| !c.is_whitespace()).count()
}

pub fn snippet(content: &str, max_chars: usize) -> String {
    let plain = plain_text(content);
    let one_line: String = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&one_line, max_chars)
}

pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max_chars).collect();
        s.push('…');
        s
    }
}

fn strip_markers(text: &str) -> String {
    text.replace("**", "").replace("==", "").replace("~~", "").replace("__", "").replace('`', "")
}

/// Rewrites every tag equal to `old` or nested below it so that it starts with `new`.
pub fn rename_tag_in(content: &str, old: &str, new: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    for (range, tag) in tag_spans(content) {
        if super::model::tag_matches(&tag, old) {
            out.push_str(&content[last..range.start]);
            out.push('#');
            out.push_str(new);
            out.push_str(&tag[old.len()..]);
            last = range.end;
        }
    }
    out.push_str(&content[last..]);
    super::frontmatter::map_tags(&out, |tag| {
        if super::model::tag_matches(tag, old) {
            Some(format!("{new}{}", &tag[old.len()..]))
        } else {
            Some(tag.to_string())
        }
    })
}

/// Removes every tag equal to `tag` or nested below it, keeping the rest of the text.
pub fn remove_tag_in(content: &str, tag: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    for (range, t) in tag_spans(content) {
        if super::model::tag_matches(&t, tag) {
            out.push_str(&content[last..range.start]);
            let mut end = range.end;
            if content[end..].starts_with(' ') {
                end += 1;
            }
            last = end;
        }
    }
    out.push_str(&content[last..]);
    let cleaned: Vec<&str> = out.lines().map(|l| l.trim_end()).collect();
    let cleaned = cleaned.join("\n").trim().to_string();
    super::frontmatter::map_tags(&cleaned, |found| {
        if super::model::tag_matches(found, tag) {
            None
        } else {
            Some(found.to_string())
        }
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum LineKind {
    Paragraph,
    Bullet,
    Ordered(String),
    Quote,
    Blank,
    /// One frontmatter field, shown above the body.
    Property,
    Heading(u8),
    /// Fenced code block; `text` on the [`RichLine`] is the raw code.
    CodeBlock { lang: Option<String> },
    /// One table row. `is_header` marks the first row after the separator.
    TableRow { is_header: bool, cells: Vec<String> },
    Rule,
    Task { checked: bool },
    /// A Markdown image shown in place; `text` on the [`RichLine`] is the alt text.
    Image { src: String },
}

pub fn split_line_kind(line: &str) -> (LineKind, &str) {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return (LineKind::Blank, "");
    }
    if let Some((level, rest)) = heading_line(trimmed) {
        return (LineKind::Heading(level), rest);
    }
    if is_rule_line(trimmed) {
        return (LineKind::Rule, "");
    }
    if let Some((checked, rest)) = task_line(trimmed) {
        return (LineKind::Task { checked }, rest);
    }
    for marker in ["- ", "* ", "• "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return (LineKind::Bullet, rest);
        }
    }
    if let Some(rest) = trimmed.strip_prefix("> ") {
        return (LineKind::Quote, rest);
    }
    if let Some(caps) = ORDERED_RE.captures(trimmed) {
        let whole = caps.get(0).unwrap();
        return (
            LineKind::Ordered(caps[1].to_string()),
            &trimmed[whole.end()..],
        );
    }
    (LineKind::Paragraph, line)
}

fn heading_line(trimmed: &str) -> Option<(u8, &str)> {
    let bytes = trimmed.as_bytes();
    let mut n = 0usize;
    while n < bytes.len() && bytes[n] == b'#' && n < 6 {
        n += 1;
    }
    if n == 0 || n >= bytes.len() || bytes[n] != b' ' {
        return None;
    }
    Some((n as u8, trimmed[n + 1..].trim_start()))
}

fn is_rule_line(trimmed: &str) -> bool {
    let t = trimmed.trim();
    t.len() >= 3
        && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

fn task_line(trimmed: &str) -> Option<(bool, &str)> {
    for (prefix, checked) in [("- [ ] ", false), ("- [x] ", true), ("- [X] ", true), ("* [ ] ", false), ("* [x] ", true), ("* [X] ", true)] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some((checked, rest));
        }
    }
    None
}

fn fence_open(line: &str) -> Option<Option<String>> {
    let trimmed = line.trim_end();
    let rest = trimmed.strip_prefix("```")?;
    let lang = rest.trim();
    if lang.is_empty() {
        Some(None)
    } else {
        Some(Some(lang.to_string()))
    }
}

fn split_table_row(line: &str) -> Option<Vec<String>> {
    let t = line.trim();
    if !t.contains('|') {
        return None;
    }
    let body = t.strip_prefix('|').unwrap_or(t);
    let body = if body.ends_with('|') && !body.ends_with("\\|") { &body[..body.len() - 1] } else { body };
    if body.chars().all(|c| c == '-' || c == ':' || c == '|' || c.is_whitespace()) {
        return None;
    }
    let cells: Vec<String> = split_cells(body).into_iter().map(|c| c.trim().to_string()).collect();
    if cells.is_empty() || cells.iter().all(|c| c.is_empty()) {
        return None;
    }
    Some(cells)
}

/// Splits a table row on `|`, keeping `\|` (unescaped) and pipes inside `[[link|label]]`
/// or inside `` `code` ``.
fn split_cells(body: &str) -> Vec<String> {
    let mut cells = vec![String::new()];
    let mut link_depth = 0usize;
    let mut in_code = false;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        let cell = cells.last_mut().unwrap();
        if c == '`' {
            in_code = !in_code;
            cell.push(c);
            continue;
        }
        if in_code && c == '|' {
            cell.push(c);
            continue;
        }
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                chars.next();
                cell.push('|');
            }
            '[' if chars.peek() == Some(&'[') => {
                chars.next();
                link_depth += 1;
                cell.push_str("[[");
            }
            ']' if link_depth > 0 && chars.peek() == Some(&']') => {
                chars.next();
                link_depth -= 1;
                cell.push_str("]]");
            }
            '|' if link_depth == 0 => cells.push(String::new()),
            _ => cell.push(c),
        }
    }
    cells
}

/// One table cell with the same inline markup, tags and links as a line of text.
pub fn parse_cell(cell: &str, keywords: &[String], ref_label: &dyn Fn(&str) -> String) -> RichLine {
    parse_inline(LineKind::Paragraph, cell, keywords, ref_label)
}

fn is_table_separator(line: &str) -> bool {
    let t = line.trim().trim_matches('|');
    !t.is_empty()
        && t.split('|').all(|cell| {
            let c = cell.trim();
            !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')
        })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub bold: bool,
    pub highlight: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub tag: bool,
    pub link: bool,
    pub memo_ref: bool,
    pub search_hit: bool,
}

impl InlineStyle {
    pub fn is_plain(&self) -> bool {
        *self == InlineStyle::default()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Tag(String),
    Memo(String),
    /// A `[[wikilink]]` whose note id is resolved by the caller.
    Wiki(String),
    Url(String),
}

#[derive(Clone, Debug)]
pub struct RichLine {
    pub kind: LineKind,
    pub text: String,
    pub styles: Vec<(Range<usize>, InlineStyle)>,
    pub targets: Vec<(Range<usize>, Target)>,
}

enum Special {
    Ref(String),
    Wiki(String, String),
    Url(String),
    Tag(String),
}

fn display_url(url: &str) -> String {
    let without_scheme = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    truncate_chars(without_scheme, 28)
}

/// Converts memo content into display lines with styled and clickable ranges.
pub fn parse_rich(
    content: &str,
    keywords: &[String],
    ref_label: &dyn Fn(&str) -> String,
) -> Vec<RichLine> {
    let mut lines = Vec::new();
    for (key, values) in super::frontmatter::parse(content).fields {
        if values.is_empty() {
            continue;
        }
        if key == "tags" || key == "tag" {
            let synthetic = values.iter().map(|tag| format!("#{tag}")).collect::<Vec<_>>().join(" ");
            lines.push(parse_inline(LineKind::Property, &synthetic, keywords, ref_label));
        } else {
            let shown = truncate_chars(&values.join(", "), 80);
            lines.push(RichLine {
                kind: LineKind::Property,
                text: format!("{key}: {shown}"),
                styles: Vec::new(),
                targets: Vec::new(),
            });
        }
    }
    let raw: Vec<&str> = super::frontmatter::body(content).split('\n').collect();
    let mut i = 0;
    while i < raw.len() {
        let raw_line = raw[i];
        if let Some(lang) = fence_open(raw_line) {
            i += 1;
            let mut code = String::new();
            while i < raw.len() && fence_open(raw[i]).is_none() {
                if !code.is_empty() {
                    code.push('\n');
                }
                code.push_str(raw[i]);
                i += 1;
            }
            if i < raw.len() {
                i += 1; // closing fence
            }
            lines.push(RichLine {
                kind: LineKind::CodeBlock { lang },
                text: code,
                styles: Vec::new(),
                targets: Vec::new(),
            });
            continue;
        }
        if let Some(cells) = split_table_row(raw_line)
            && i + 1 < raw.len()
            && is_table_separator(raw[i + 1])
        {
            lines.push(RichLine {
                kind: LineKind::TableRow {
                    is_header: true,
                    cells: cells.clone(),
                },
                text: cells.join(" · "),
                styles: Vec::new(),
                targets: Vec::new(),
            });
            i += 2; // header + separator
            while i < raw.len() {
                let Some(row) = split_table_row(raw[i]) else { break };
                lines.push(RichLine {
                    kind: LineKind::TableRow {
                        is_header: false,
                        cells: row.clone(),
                    },
                    text: row.join(" · "),
                    styles: Vec::new(),
                    targets: Vec::new(),
                });
                i += 1;
            }
            continue;
        }
        let found = super::images::spans(raw_line);
        if !found.is_empty() {
            let mut pos = 0;
            for image in found {
                let before = &raw_line[pos..image.range.start];
                if !before.trim().is_empty() {
                    let (kind, rest) = if pos == 0 { split_line_kind(before) } else { (LineKind::Paragraph, before.trim()) };
                    lines.push(parse_inline(kind, rest, keywords, ref_label));
                }
                lines.push(RichLine {
                    kind: LineKind::Image { src: image.src },
                    text: image.alt,
                    styles: Vec::new(),
                    targets: Vec::new(),
                });
                pos = image.range.end;
            }
            let after = raw_line[pos..].trim();
            if !after.is_empty() {
                lines.push(parse_inline(LineKind::Paragraph, after, keywords, ref_label));
            }
            i += 1;
            continue;
        }
        let (kind, rest) = split_line_kind(raw_line);
        if kind == LineKind::Blank || kind == LineKind::Rule {
            lines.push(RichLine {
                kind,
                text: String::new(),
                styles: Vec::new(),
                targets: Vec::new(),
            });
            i += 1;
            continue;
        }
        lines.push(parse_inline(kind, rest, keywords, ref_label));
        i += 1;
    }
    while lines.last().is_some_and(|l| l.kind == LineKind::Blank) {
        lines.pop();
    }
    lines
}

fn parse_inline(
    kind: LineKind,
    line: &str,
    keywords: &[String],
    ref_label: &dyn Fn(&str) -> String,
) -> RichLine {
    let mut specials: Vec<(Range<usize>, Special)> = Vec::new();
    for (range, id) in ref_spans(line) {
        specials.push((range, Special::Ref(id)));
    }
    for hit in super::frontmatter::wikilink_hits(line) {
        specials.push((hit.range, Special::Wiki(hit.target, hit.label)));
    }
    for m in URL_RE.find_iter(line) {
        specials.push((m.range(), Special::Url(m.as_str().to_string())));
    }
    for (range, tag) in tag_spans(line) {
        specials.push((range, Special::Tag(tag)));
    }
    specials.sort_by_key(|(r, _)| r.start);
    let mut filtered: Vec<(Range<usize>, Special)> = Vec::new();
    for (range, special) in specials {
        if filtered.last().is_some_and(|(last, _)| range.start < last.end) {
            continue;
        }
        filtered.push((range, special));
    }

    let mut text = String::new();
    let mut pieces: Vec<(Range<usize>, InlineStyle)> = Vec::new();
    let mut targets: Vec<(Range<usize>, Target)> = Vec::new();
    let mut style = InlineStyle::default();
    let mut pos = 0;

    for (range, special) in filtered {
        push_plain(line, pos..range.start, &mut style, &mut text, &mut pieces);
        let start = text.len();
        let mut special_style = style;
        let target = match special {
            Special::Ref(id) => {
                text.push_str(&format!("↗{}", ref_label(&id)));
                special_style.memo_ref = true;
                Target::Memo(id)
            }
            Special::Wiki(target, label) => {
                text.push_str(&label);
                special_style.memo_ref = true;
                Target::Wiki(target)
            }
            Special::Url(url) => {
                text.push_str(&display_url(&url));
                special_style.link = true;
                Target::Url(url)
            }
            Special::Tag(tag) => {
                text.push('#');
                text.push_str(&tag);
                special_style.tag = true;
                Target::Tag(tag)
            }
        };
        pieces.push((start..text.len(), special_style));
        targets.push((start..text.len(), target));
        pos = range.end;
    }
    push_plain(line, pos..line.len(), &mut style, &mut text, &mut pieces);

    if !keywords.is_empty() {
        let lower = text.to_ascii_lowercase();
        let mut hits: Vec<Range<usize>> = Vec::new();
        for kw in keywords {
            let kw = kw.to_ascii_lowercase();
            if kw.is_empty() {
                continue;
            }
            let mut from = 0;
            while let Some(found) = lower[from..].find(&kw) {
                let s = from + found;
                hits.push(s..s + kw.len());
                from = s + kw.len();
            }
        }
        if !hits.is_empty() {
            for (range, st) in pieces.iter_mut() {
                if hits.iter().any(|h| h.start < range.end && range.start < h.end) {
                    st.search_hit = true;
                }
            }
        }
    }

    let mut styles: Vec<(Range<usize>, InlineStyle)> = Vec::new();
    for (range, st) in pieces {
        if let Some((last_range, last_style)) = styles.last_mut()
            && *last_style == st
            && last_range.end == range.start
        {
            last_range.end = range.end;
            continue;
        }
        styles.push((range, st));
    }
    styles.retain(|(_, st)| !st.is_plain());

    RichLine {
        kind,
        text,
        styles,
        targets,
    }
}

const MARKERS: [&str; 4] = ["**", "==", "__", "~~"];

fn marker_flag<'a>(style: &'a mut InlineStyle, marker: &str) -> &'a mut bool {
    match marker {
        "**" => &mut style.bold,
        "==" => &mut style.highlight,
        "__" => &mut style.underline,
        _ => &mut style.strike,
    }
}

/// Appends `line[segment]` to `text`, toggling inline styles on paired markers.
/// A marker only opens a style when a closing marker follows on the same line.
/// Inline `` `code` `` is also recognized.
fn push_plain(
    line: &str,
    segment: Range<usize>,
    style: &mut InlineStyle,
    text: &mut String,
    pieces: &mut Vec<(Range<usize>, InlineStyle)>,
) {
    let mut i = segment.start;
    while i < segment.end {
        if !style.code
            && line.as_bytes().get(i) == Some(&b'`')
            && let Some(rel) = line[i + 1..segment.end].find('`')
            && rel > 0
        {
            let code = &line[i + 1..i + 1 + rel];
            let mut code_style = *style;
            code_style.code = true;
            let start = text.len();
            text.push_str(code);
            pieces.push((start..text.len(), code_style));
            i += 2 + rel;
            continue;
        }
        if let Some(marker) = MARKERS.iter().find(|m| line[i..segment.end].starts_with(**m)) {
            let flag = marker_flag(style, marker);
            if *flag || line[i + 2..].contains(*marker) {
                *flag = !*flag;
                i += 2;
                continue;
            }
        }
        let ch = line[i..].chars().next().unwrap();
        let start = text.len();
        text.push(ch);
        pieces.push((start..text.len(), *style));
        i += ch.len_utf8();
    }
}

/// The partial `#tag` or `@query` immediately before `cursor`, if any.
pub fn token_before_cursor(text: &str, cursor: usize, trigger: char) -> Option<(usize, String)> {
    let cursor = cursor.min(text.len());
    let before = &text[..cursor];
    let start = before.rfind(trigger)?;
    let prefix_ok = start == 0
        || before[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_whitespace());
    if !prefix_ok {
        return None;
    }
    let query = &before[start + trigger.len_utf8()..];
    if query.chars().any(|c| c.is_whitespace() || c == trigger) {
        return None;
    }
    Some((start, query.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_tags_join_inline_tags_and_survive_rename() {
        let note = "---\ntags:\n  - 读书/方法\n---\n\n正文 #想法\n见 [[项目]]\n";
        assert_eq!(extract_tags(note), vec!["想法", "读书/方法"]);
        assert_eq!(wikilink_targets(note), vec!["项目".to_string()]);
        let renamed = rename_tag_in(note, "读书", "阅读");
        assert!(renamed.contains("阅读/方法"));
        assert!(!extract_tags(&renamed).iter().any(|t| t.starts_with("读书")));
        assert!(plain_text(note).contains("正文"));
        assert!(!plain_text(note).contains("tags:"));
        let lines = parse_rich(note, &[], &|_| String::new());
        assert!(lines.iter().any(|l| l.kind == LineKind::Property && l.text.contains("#读书/方法")));
        assert!(lines.iter().any(|l| l.targets.iter().any(|(_, t)| matches!(t, Target::Wiki(name) if name == "项目"))));
    }

    #[test]
    fn images_render_in_place_and_leave_plain_text() {
        let note = "开头 #图\n看图 ![封面](images/a.png) 之后\n\n![](images/b.png)\n结尾";
        let lines = parse_rich(note, &[], &|_| String::new());
        let kinds: Vec<_> = lines.iter().map(|l| (&l.kind, l.text.as_str())).collect();
        assert_eq!(kinds[1], (&LineKind::Paragraph, "看图 "));
        assert_eq!(kinds[2], (&LineKind::Image { src: "images/a.png".into() }, "封面"));
        assert_eq!(kinds[3], (&LineKind::Paragraph, "之后"));
        assert_eq!(kinds[5], (&LineKind::Image { src: "images/b.png".into() }, ""));
        assert_eq!(plain_text(note).split_whitespace().collect::<Vec<_>>(), vec!["开头", "#图", "看图", "之后", "结尾"]);
    }

    #[test]
    fn tags_and_hierarchy() {
        let tags = extract_tags("读书笔记 #读书/Different/营销 和 #想法， 以及 url https://a.com/#x");
        assert_eq!(tags, vec!["读书/Different/营销", "想法"]);
        assert!(extract_tags("# 标题").is_empty());
        assert!(extract_tags("a#b").is_empty());
        assert_eq!(extract_tags("**#生活/回忆录 ** 正文"), vec!["生活/回忆录"]);
        assert!(extract_tags("a**#b**").is_empty());
        let technical = "缺陷 #1234567890123，修复\n- #2 完成（#6）：上线\n颜色 #000 和 `#内联` 以及 #真标签（上）\n```bash\n#!/bin/bash\n#代码\n```\n#结尾";
        assert_eq!(extract_tags(technical), vec!["真标签", "结尾"]);
    }

    #[test]
    fn refs_and_rename() {
        let c = "批注 [[memo:abc123]] #a/b #a #ab";
        assert_eq!(extract_refs(c), vec!["abc123"]);
        assert_eq!(rename_tag_in(c, "a", "x"), "批注 [[memo:abc123]] #x/b #x #ab");
        assert_eq!(remove_tag_in(c, "a"), "批注 [[memo:abc123]] #ab");
    }

    #[test]
    fn rich_markup() {
        let lines = parse_rich("- **加粗** ==高亮== #标签", &[], &|_| String::new());
        assert_eq!(lines[0].kind, LineKind::Bullet);
        assert_eq!(lines[0].text, "加粗 高亮 #标签");
        assert!(lines[0].styles.iter().any(|(_, s)| s.bold));
        assert!(lines[0].styles.iter().any(|(_, s)| s.highlight));
        assert_eq!(lines[0].targets.len(), 1);
        let lines = parse_rich("a == b", &[], &|_| String::new());
        assert_eq!(lines[0].text, "a == b");
    }

    #[test]
    fn cursor_token() {
        assert_eq!(
            token_before_cursor("hello #读", "hello #读".len(), '#'),
            Some((6, "读".into()))
        );
        assert_eq!(token_before_cursor("hello# x", 8, '#'), None);
        // Markdown heading `# Title` has a space, so it is not a tag token.
        assert_eq!(token_before_cursor("# ", 2, '#'), None);
    }

    #[test]
    fn markdown_blocks_and_inline_code() {
        let md = "# 标题\n\n```rust\nfn main() {}\n```\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\n用 `code` 标记\n\n- [x] 完成\n\n---\n";
        let lines = parse_rich(md, &[], &|_| String::new());
        assert!(lines.iter().any(|l| matches!(l.kind, LineKind::Heading(1))));
        assert!(lines.iter().any(|l| matches!(l.kind, LineKind::CodeBlock { .. })));
        assert!(lines.iter().any(|l| matches!(l.kind, LineKind::TableRow { is_header: true, .. })));
        let table = parse_rich("| 目录 | 用途 |\n| --- | --- |\n| [[00-index/README\\|索引 ★]] | 入口 \\| 更新 |", &[], &|_| String::new());
        let LineKind::TableRow { cells, .. } = &table[1].kind else { panic!("table row") };
        assert_eq!(cells, &vec!["[[00-index/README|索引 ★]]".to_string(), "入口 | 更新".to_string()]);
        let coded = parse_rich("| 表 | 内容 | 写者 |\n| --- | --- | --- |\n| `t` | 索引（`at|id` 分页） | Store |", &[], &|_| String::new());
        assert!(matches!(&coded[1].kind, LineKind::TableRow { cells, .. } if cells.len() == 3 && cells[1].contains("`at|id`")));
        let cell = parse_cell(&cells[0], &[], &|_| String::new());
        assert_eq!(cell.text, "索引 ★");
        assert!(matches!(&cell.targets[0].1, Target::Wiki(t) if t == "00-index/README"));
        assert_eq!(super::super::frontmatter::note_links("| [[a/b\\|标签]] |"), vec!["a/b".to_string()]);
        assert!(lines.iter().any(|l| matches!(l.kind, LineKind::Task { checked: true })));
        assert!(lines.iter().any(|l| l.kind == LineKind::Rule));
        let inline = lines.iter().find(|l| l.text.contains("code")).unwrap();
        assert!(inline.styles.iter().any(|(_, s)| s.code));
        // Heading with space is not a tag; tag without space still works.
        assert!(extract_tags("# 标题").is_empty());
        assert_eq!(extract_tags("见 #产品").first().map(String::as_str), Some("产品"));
    }
}
