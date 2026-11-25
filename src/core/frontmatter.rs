//! Obsidian-style YAML frontmatter and wikilinks.
//!
//! The frontmatter stays inside the memo text, so editing and export round-trip.
//! Tags, titles and note links are derived from it for search, the graph and rendering.
//!
//! The parser covers the subset Obsidian writes: scalars, inline lists, block lists
//! and folded blocks. It is not a full YAML implementation.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use super::time;

const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "heic", "heif"];

#[derive(Clone, Debug, PartialEq)]
pub struct Frontmatter {
    /// End of the fenced block, including the closing newline. `0` when there is none.
    pub end: usize,
    pub fields: Vec<(String, Vec<String>)>,
}

impl Frontmatter {
    fn empty() -> Self {
        Self { end: 0, fields: Vec::new() }
    }
}

pub fn parse(content: &str) -> Frontmatter {
    let (end, yaml) = match fence(content) {
        Some(pair) => pair,
        None => return Frontmatter::empty(),
    };
    Frontmatter { end, fields: parse_fields(yaml) }
}

pub fn has_frontmatter(content: &str) -> bool {
    parse(content).end > 0
}

pub fn body(content: &str) -> &str {
    let end = parse(content).end;
    &content[end..]
}

pub fn body_start(content: &str) -> usize {
    parse(content).end
}

pub fn title(content: &str) -> Option<String> {
    field(content, &["title"]).into_iter().next()
}

pub fn tags(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tag in field(content, &["tags", "tag"]) {
        if !out.contains(&tag) {
            out.push(tag);
        }
    }
    out
}

pub fn first_heading(content: &str) -> Option<String> {
    let mut fenced = false;
    for line in body(content).lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if let Some(text) = heading_text(trimmed) {
            return Some(text);
        }
    }
    None
}

/// Lookup keys for wikilink resolution. Rank 0 is a title or alias; rank 1 is the first heading.
pub fn link_keys(content: &str) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    let mut push = |value: &str, rank: u8| {
        let key = normalize_link(value);
        if key.is_empty() || out.iter().any(|(k, r)| k == &key && *r == rank) {
            return;
        }
        out.push((key, rank));
    };
    for value in field(content, &["title", "aliases", "alias"]) {
        push(&value, 0);
    }
    if let Some(heading) = first_heading(content) {
        push(&heading, 1);
    }
    out
}

/// Note targets of `[[wikilinks]]` and of note embeds `![[Note]]`. Image embeds are skipped.
pub fn note_links(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for link in wikilink_hits(content) {
        if !out.contains(&link.target) {
            out.push(link.target);
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct WikilinkHit {
    pub range: Range<usize>,
    pub target: String,
    pub label: String,
}

pub fn wikilink_hits(content: &str) -> Vec<WikilinkHit> {
    scan_outside_fences(content, false)
}

#[derive(Clone, Debug)]
pub struct ImageEmbed {
    pub range: Range<usize>,
    pub target: String,
}

pub fn image_embeds(content: &str) -> Vec<ImageEmbed> {
    scan_outside_fences(content, true)
        .into_iter()
        .map(|hit| ImageEmbed { range: hit.range, target: hit.target })
        .collect()
}

/// Adds filename aliases when the note cannot already be found by that name.
pub fn ensure_aliases(content: &str, aliases: &[String]) -> String {
    let known: HashSet<String> = link_keys(content).into_iter().map(|(k, _)| k).collect();
    let mut adding = Vec::new();
    for alias in aliases {
        let key = normalize_link(alias);
        if key.is_empty() || known.contains(&key) || adding.iter().any(|a: &String| normalize_link(a) == key) {
            continue;
        }
        adding.push(alias.trim().to_string());
    }
    if adding.is_empty() {
        return content.to_string();
    }
    let mut fm = parse(content);
    if let Some((_, values)) = fm.fields.iter_mut().find(|(k, _)| k == "aliases" || k == "alias") {
        values.extend(adding);
    } else {
        fm.fields.push(("aliases".into(), adding));
    }
    render(&fm.fields, body(content))
}

/// Rewrites tags in `tags` / `tag`. `None` drops the tag. Other fields stay.
pub fn map_tags(content: &str, mut f: impl FnMut(&str) -> Option<String>) -> String {
    let fm = parse(content);
    if fm.end == 0 {
        return content.to_string();
    }
    let mut fields = Vec::new();
    let mut changed = false;
    for (key, values) in fm.fields {
        if key == "tags" || key == "tag" {
            let mut next = Vec::new();
            for value in &values {
                match f(value) {
                    Some(tag) if tag == *value => next.push(value.clone()),
                    Some(tag) if !tag.is_empty() => {
                        changed = true;
                        if !next.contains(&tag) {
                            next.push(tag);
                        }
                    }
                    _ => changed = true,
                }
            }
            if next != values {
                changed = true;
            }
            if !next.is_empty() {
                fields.push((key, next));
            }
        } else {
            fields.push((key, values));
        }
    }
    if !changed {
        return content.to_string();
    }
    render(&fields, body(content))
}

pub fn created_timestamp(content: &str) -> Option<i64> {
    for key in ["created", "created_at", "date"] {
        if let Some(value) = field(content, &[key]).into_iter().next()
            && let Some(ts) = time::parse_datetime(&value)
        {
            return Some(ts);
        }
    }
    None
}

pub fn normalize_link(target: &str) -> String {
    let trimmed = target.trim().trim_matches(|c| c == '<' || c == '>');
    let without_ext = trimmed.strip_suffix(".md").unwrap_or(trimmed);
    without_ext.trim().trim_matches('/').to_lowercase()
}

pub fn is_image_name(name: &str) -> bool {
    let stem = name.split(['?', '#']).next().unwrap_or(name);
    let ext = stem.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    IMAGE_EXT.contains(&ext.as_str())
}

/// Resolves wikilink targets against note titles, aliases and headings.
/// A title or alias wins over a heading when several notes share the name.
pub struct LinkIndex {
    ids: Vec<String>,
    by_key: HashMap<String, Vec<(usize, u8)>>,
}

impl LinkIndex {
    pub fn build<'a>(notes: impl Iterator<Item = (&'a str, &'a str)>) -> Self {
        let mut ids = Vec::new();
        let mut by_key: HashMap<String, Vec<(usize, u8)>> = HashMap::new();
        for (id, content) in notes {
            let index = ids.len();
            ids.push(id.to_string());
            let mut best: HashMap<String, u8> = HashMap::new();
            for (key, rank) in link_keys(content) {
                best.entry(key).and_modify(|r| *r = (*r).min(rank)).or_insert(rank);
            }
            for (key, rank) in best {
                by_key.entry(key).or_default().push((index, rank));
            }
        }
        Self { ids, by_key }
    }

    pub fn resolve(&self, target: &str) -> Vec<String> {
        let key = normalize_link(target);
        if key.is_empty() {
            return Vec::new();
        }
        let mut hits = self.lookup(&key);
        if hits.is_empty()
            && let Some(stem) = key.rsplit_once('/').map(|(_, stem)| stem)
        {
            hits = self.lookup(stem);
        }
        let Some(best) = hits.iter().map(|(_, rank)| *rank).min() else {
            return Vec::new();
        };
        hits.into_iter()
            .filter(|(_, rank)| *rank == best)
            .map(|(index, _)| self.ids[index].clone())
            .collect()
    }

    fn lookup(&self, key: &str) -> Vec<(usize, u8)> {
        self.by_key.get(key).cloned().unwrap_or_default()
    }
}

fn field(content: &str, keys: &[&str]) -> Vec<String> {
    let fm = parse(content);
    let mut out = Vec::new();
    for (key, values) in fm.fields {
        if keys.iter().any(|k| *k == key) {
            out.extend(values);
        }
    }
    out
}

fn fence(content: &str) -> Option<(usize, &str)> {
    let text = content.strip_prefix('\u{feff}').unwrap_or(content);
    let bom = content.len() - text.len();
    let (first, mut offset) = next_line(text, 0)?;
    if first.trim() != "---" {
        return None;
    }
    let yaml_start = offset;
    loop {
        let (line, next) = next_line(text, offset)?;
        let marker = line.trim();
        if marker == "---" || marker == "..." {
            return Some((bom + next, &text[yaml_start..offset]));
        }
        offset = next;
        if next == text.len() {
            return None;
        }
    }
}

fn next_line(text: &str, start: usize) -> Option<(&str, usize)> {
    if start > text.len() {
        return None;
    }
    if start == text.len() {
        return None;
    }
    let rest = &text[start..];
    match rest.find('\n') {
        Some(i) => {
            let line = &rest[..i];
            let line = line.strip_suffix('\r').unwrap_or(line);
            Some((line, start + i + 1))
        }
        None => Some((rest.strip_suffix('\r').unwrap_or(rest), text.len())),
    }
}

fn parse_fields(yaml: &str) -> Vec<(String, Vec<String>)> {
    let lines: Vec<&str> = yaml.lines().collect();
    let mut i = 0;
    let mut fields: Vec<(String, Vec<String>)> = Vec::new();
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || (trimmed.starts_with('#') && !trimmed.contains(':')) {
            i += 1;
            continue;
        }
        let Some((key, rest)) = split_key(line) else {
            i += 1;
            continue;
        };
        i += 1;
        let values = if rest.is_empty() || rest == "|" || rest == ">" || rest == "|-" || rest == "|+" || rest == ">-" || rest == ">+" {
            if rest.is_empty() {
                read_block(&lines, &mut i, &key)
            } else {
                read_literal(&lines, &mut i)
            }
        } else if rest.starts_with('[') {
            parse_flow_list(&rest).into_iter().flat_map(|item| values_of(&key, &item, true)).collect()
        } else {
            values_of(&key, &rest, false)
        };
        if let Some((_, existing)) = fields.iter_mut().find(|(k, _)| k == &key) {
            existing.extend(values);
        } else {
            fields.push((key, values));
        }
    }
    fields
}

fn read_block(lines: &[&str], i: &mut usize, key: &str) -> Vec<String> {
    let mut values = Vec::new();
    while *i < lines.len() {
        let line = lines[*i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            *i += 1;
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 && !trimmed.starts_with("- ") && trimmed != "-" {
            break;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            values.extend(values_of(key, item, true));
        } else if trimmed == "-" {
            // A nested value on the following line is ignored; the item itself is empty.
        } else if let Some(last) = values.last_mut() {
            if !last.is_empty() {
                last.push(' ');
            }
            last.push_str(trimmed);
        } else {
            values.extend(values_of(key, trimmed, false));
        }
        *i += 1;
    }
    values
}

fn read_literal(lines: &[&str], i: &mut usize) -> Vec<String> {
    let mut buf = String::new();
    while *i < lines.len() {
        let line = lines[*i];
        if !line.is_empty() && line.len() == line.trim_start().len() {
            break;
        }
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(line.trim());
        *i += 1;
    }
    if buf.is_empty() { Vec::new() } else { vec![buf] }
}

fn values_of(key: &str, raw: &str, _from_list: bool) -> Vec<String> {
    let value = unquote(raw);
    if value.is_empty() {
        return Vec::new();
    }
    if key == "tags" || key == "tag" { normalize_tags(&value) } else { vec![value] }
}

fn normalize_tags(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .map(|part| part.trim().trim_matches(|c| c == '"' || c == '\'').trim_start_matches('#').trim_matches('/'))
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
}

fn split_key(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.starts_with('-') {
        return None;
    }
    let (key, rest) = trimmed.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || key.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    Some((key.to_string(), rest.trim().to_string()))
}

fn parse_flow_list(raw: &str) -> Vec<String> {
    let text = raw.trim();
    let text = text.strip_prefix('[').unwrap_or(text);
    let text = text.trim_end_matches(']').trim();
    let mut items = Vec::new();
    let mut buf = String::new();
    let mut quote: Option<char> = None;
    for ch in text.chars() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            buf.push(ch);
            continue;
        }
        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                buf.push(ch);
            }
            ',' => {
                items.push(std::mem::take(&mut buf));
            }
            _ => buf.push(ch),
        }
    }
    if !buf.trim().is_empty() {
        items.push(buf);
    }
    items
}

fn is_quoted(text: &str) -> bool {
    let quote = text.chars().next();
    matches!(quote, Some('"' | '\'')) && text.ends_with(quote.unwrap()) && text.len() >= 2
}

fn unquote(raw: &str) -> String {
    let text = raw.trim();
    if is_quoted(text) {
        let inner = &text[1..text.len() - 1];
        return inner.replace("\\\"", "\"").replace("\\'", "'").replace("\\\\", "\\");
    }
    let text = match text.find(" #") {
        Some(i) => text[..i].trim(),
        None => text,
    };
    text.to_string()
}

fn render(fields: &[(String, Vec<String>)], body: &str) -> String {
    if fields.iter().all(|(_, values)| values.is_empty()) {
        return body.trim_start_matches('\n').to_string();
    }
    let mut out = String::from("---\n");
    for (key, values) in fields {
        if values.is_empty() {
            continue;
        }
        if values.len() == 1 && !values[0].contains('\n') {
            out.push_str(key);
            out.push_str(": ");
            out.push_str(&quote_if_needed(&values[0]));
            out.push('\n');
        } else {
            out.push_str(key);
            out.push_str(":\n");
            for value in values {
                for (n, line) in value.split('\n').enumerate() {
                    if n == 0 {
                        out.push_str("  - ");
                    } else {
                        out.push_str("    ");
                    }
                    out.push_str(&quote_if_needed(line));
                    out.push('\n');
                }
            }
        }
    }
    out.push_str("---\n");
    if body.starts_with('\n') {
        out.push_str(body);
    } else if !body.is_empty() {
        out.push('\n');
        out.push_str(body);
    }
    out
}

fn quote_if_needed(value: &str) -> String {
    let special = value.is_empty()
        || value.chars().any(|c| {
            c.is_whitespace() || matches!(c, ':' | '#' | '[' | ']' | '{' | '}' | ',' | '"' | '\'' | '&' | '*' | '!' | '|' | '>' | '%' | '@' | '`')
        });
    if special {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn heading_text(trimmed: &str) -> Option<String> {
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = trimmed[hashes..].trim();
    if rest.is_empty() || trimmed.as_bytes().get(hashes) != Some(&b' ') {
        return None;
    }
    let rest = rest.trim_end_matches('#').trim();
    if rest.is_empty() { None } else { Some(rest.to_string()) }
}

fn scan_outside_fences(content: &str, images_only: bool) -> Vec<WikilinkHit> {
    let start = body_start(content);
    let body = &content[start..];
    let mut hits = Vec::new();
    let mut fenced = false;
    let mut offset = start;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            offset += line.len();
            continue;
        }
        if !fenced {
            scan_line(offset, line, images_only, &mut hits);
        }
        offset += line.len();
    }
    hits
}

fn scan_line(line_start: usize, line: &str, images_only: bool, hits: &mut Vec<WikilinkHit>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 3 < bytes.len() {
        if bytes[i] == b'[' && bytes.get(i + 1) == Some(&b'[') {
            let bang = i > 0 && bytes[i - 1] == b'!';
            if let Some(rel) = line[i + 2..].find("]]") {
                let inner = &line[i + 2..i + 2 + rel];
                let end = line_start + i + 2 + rel + 2;
                if let Some((target, label)) = split_target(inner) {
                    let image = is_image_name(&target);
                    let start = line_start + if bang { i - 1 } else { i };
                    if images_only {
                        if bang && image {
                            hits.push(WikilinkHit { range: start..end, target, label });
                        }
                    } else if !image && !target.starts_with("memo:") {
                        hits.push(WikilinkHit { range: start..end, target, label });
                    }
                }
                i = i + 2 + rel + 2;
                continue;
            }
        }
        if !images_only {
            i += 1;
            continue;
        }
        if bytes[i] == b'!'
            && bytes.get(i + 1) == Some(&b'[')
            && let Some(close) = line[i..].find("](")
        {
            let paren = i + close + 2;
            if let Some(end_rel) = line[paren..].find(')') {
                let url = line[paren..paren + end_rel].trim();
                let end = line_start + paren + end_rel + 1;
                if !url.contains("://") && is_image_name(url) {
                    hits.push(WikilinkHit {
                        range: line_start + i..end,
                        target: url.to_string(),
                        label: String::new(),
                    });
                }
                i = paren + end_rel + 1;
                continue;
            }
        }
        i += 1;
    }
}

fn split_target(inner: &str) -> Option<(String, String)> {
    let inner = inner.trim();
    if inner.is_empty() {
        return None;
    }
    // Inside a Markdown table the label pipe is written `\|`.
    let (path, label) = match inner.rsplit_once('|') {
        Some((path, label)) => (path.trim().trim_end_matches('\\').trim(), label.trim().to_string()),
        None => (inner, String::new()),
    };
    let path = path.split_once('#').map(|(path, _)| path.trim()).unwrap_or(path);
    if path.is_empty() {
        return None;
    }
    let label = if label.is_empty() { path.to_string() } else { label };
    Some((path.to_string(), label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_tags_aliases_and_dates() {
        let note = "---\ntitle: 阅读法\ntags:\n  - 读书/方法\n  - 想法\naliases: [卡片笔记, \"Zettel\"]\ncreated: 2024-05-06\n---\n\n正文 #额外\n";
        assert_eq!(title(note).as_deref(), Some("阅读法"));
        assert_eq!(tags(note), vec!["读书/方法", "想法"]);
        assert!(created_timestamp(note).is_some());
        assert!(body(note).trim_start().starts_with("正文"));
        assert!(!link_keys(note).iter().any(|(k, _)| k == "---"));
        assert!(link_keys(note).iter().any(|(k, r)| k == "卡片笔记" && *r == 0));
    }

    #[test]
    fn wikilinks_skip_images_and_fences() {
        let note = "见 [[阅读法|那篇]] 和 ![[封面.png]]\n\n```\n[[不是链接]]\n```\n\n![[另一篇]]\n";
        assert_eq!(note_links(note), vec!["阅读法".to_string(), "另一篇".to_string()]);
        assert_eq!(image_embeds(note).len(), 1);
        assert_eq!(image_embeds(note)[0].target, "封面.png");
    }

    #[test]
    fn tag_rewrite_and_alias_insert() {
        let note = "---\ntags:\n  - 读书/方法\n  - 其他\n---\n\n#读书/方法 保留\n";
        let renamed = map_tags(note, |tag| {
            tag.strip_prefix("读书").map(|rest| format!("阅读{rest}")).or_else(|| Some(tag.to_string()))
        });
        assert!(renamed.contains("阅读/方法"));
        assert!(!parse(&renamed).fields.iter().any(|(_, vs)| vs.iter().any(|v| v == "读书/方法")));
        let with_name = ensure_aliases("只有正文\n", &["文件名".into()]);
        assert!(has_frontmatter(&with_name));
        assert!(link_keys(&with_name).iter().any(|(k, _)| k == "文件名"));
        assert!(ensure_aliases(&with_name, &["文件名".into()]) == with_name);    }

    #[test]
    fn link_index_prefers_title_over_heading() {
        let index = LinkIndex::build([("a", "---\ntitle: 同名\n---\n"), ("b", "# 同名\n\n正文")].into_iter());
        assert_eq!(index.resolve("同名"), vec!["a".to_string()]);
        assert_eq!(index.resolve("notes/阅读法.md"), Vec::<String>::new());
        let index = LinkIndex::build([("a", "---\naliases:\n  - notes/阅读法\n---\n")].into_iter());
        assert_eq!(index.resolve("[[notes/阅读法]]".trim_start_matches("[[").trim_end_matches("]]")), vec!["a".to_string()]);
    }
}
