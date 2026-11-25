//! Images live in the note text as Markdown, `![alt](images/<file>)`, so they
//! render where they were placed. `images/` is the library's image folder.
//! Notes saved before that keep their file list in `Memo::images`; those files
//! are shown after the text until the note is edited.

use std::borrow::Cow;
use std::ops::Range;

pub const FOLDER: &str = "images/";

#[derive(Clone, Debug, PartialEq)]
pub struct InlineImage {
    pub range: Range<usize>,
    pub alt: String,
    pub src: String,
}

pub fn markdown(name: &str, alt: &str) -> String {
    format!("![{alt}]({FOLDER}{name})")
}

/// `images/abc.png` → `abc.png`, for files in the library's image folder only.
pub fn library_name(src: &str) -> Option<&str> {
    let name = src.strip_prefix(FOLDER)?;
    (!name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\'])).then_some(name)
}

/// Markdown images outside fenced and inline code.
pub fn spans(content: &str) -> Vec<InlineImage> {
    let mut out = Vec::new();
    let mut fenced = false;
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced {
            line_spans(line, offset, &mut out);
        }
        offset += line.len();
    }
    out
}

fn line_spans(line: &str, offset: usize, out: &mut Vec<InlineImage>) {
    let bytes = line.as_bytes();
    let mut code = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'`' => code = !code,
            b'!' if !code && bytes.get(i + 1) == Some(&b'[') && bytes.get(i + 2) != Some(&b'[') => {
                if let Some(image) = parse_at(line, i) {
                    i = image.range.end;
                    out.push(InlineImage {
                        range: offset + image.range.start..offset + image.range.end,
                        ..image
                    });
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn parse_at(line: &str, start: usize) -> Option<InlineImage> {
    let alt_start = start + 2;
    let close = alt_start + line[alt_start..].find(']')?;
    if line.as_bytes().get(close + 1) != Some(&b'(') {
        return None;
    }
    let src_start = close + 2;
    let end = src_start + line[src_start..].find(')')?;
    let inner = line[src_start..end].trim();
    let src = match inner.strip_prefix('<') {
        Some(rest) => rest.split('>').next().unwrap_or(""),
        None => inner.split_whitespace().next().unwrap_or(""),
    };
    if src.is_empty() {
        return None;
    }
    Some(InlineImage {
        range: start..end + 1,
        alt: line[alt_start..close].trim().to_string(),
        src: src.to_string(),
    })
}

/// Library files the text places, in order, without repeats.
pub fn library_names(content: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for image in spans(content) {
        if let Some(name) = library_name(&image.src)
            && !names.iter().any(|n| n == name)
        {
            names.push(name.to_string());
        }
    }
    names
}

/// Files placed in the text first, then stored files the text does not place.
pub fn merged(images: Vec<String>, content: &str) -> Vec<String> {
    let mut names = library_names(content);
    for name in images {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The text with stored files it does not place yet appended, one per line.
pub fn with_unplaced<'a>(content: &'a str, images: &[String]) -> Cow<'a, str> {
    let placed = library_names(content);
    let extra: Vec<&String> = images.iter().filter(|n| !placed.contains(n)).collect();
    if extra.is_empty() {
        return Cow::Borrowed(content);
    }
    let mut out = content.trim_end().to_string();
    for (i, name) in extra.into_iter().enumerate() {
        if !out.is_empty() {
            out.push_str(if i == 0 { "\n\n" } else { "\n" });
        }
        out.push_str(&markdown(name, ""));
    }
    Cow::Owned(out)
}

/// The text without image markup (Markdown images and `![[image]]` embeds).
pub fn strip(content: &str) -> Cow<'_, str> {
    if !content.contains("![") {
        return Cow::Borrowed(content);
    }
    let mut ranges: Vec<Range<usize>> = spans(content).into_iter().map(|i| i.range).collect();
    ranges.extend(
        super::frontmatter::image_embeds(content)
            .into_iter()
            .map(|e| e.range)
            .filter(|r| content[r.clone()].starts_with("![[")),
    );
    if ranges.is_empty() {
        return Cow::Borrowed(content);
    }
    ranges.sort_by_key(|r| r.start);
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    for range in ranges {
        if range.start < last {
            continue;
        }
        out.push_str(&content[last..range.start]);
        last = range.end;
    }
    out.push_str(&content[last..]);
    Cow::Owned(out)
}

/// Rewrites where library images point, e.g. at an export folder.
pub fn map_library(content: &str, f: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    for image in spans(content) {
        let Some(name) = library_name(&image.src) else { continue };
        out.push_str(&content[last..image.range.start]);
        out.push_str(&format!("![{}]({})", image.alt, f(name)));
        last = image.range.end;
    }
    out.push_str(&content[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_skip_code_and_read_titles() {
        let text = "前 ![封面](images/a.png) 后\n`![不是](images/x.png)`\n```\n![也不是](images/y.png)\n```\n![](<b c.png> \"标题\") ![[pic.png]]";
        let found = spans(text);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].alt, "封面");
        assert_eq!(library_name(&found[0].src), Some("a.png"));
        assert_eq!(found[1].src, "b c.png");
        assert_eq!(library_names(text), vec!["a.png".to_string()]);
        assert_eq!(library_name("images/../x.png"), None);
        assert_eq!(strip(text).matches("![").count(), 2);
    }

    #[test]
    fn unplaced_files_follow_the_text_and_merge_keeps_order() {
        let text = "正文\n![](images/a.png)";
        let images = vec!["b.png".to_string(), "a.png".to_string()];
        assert_eq!(with_unplaced(text, &images), "正文\n![](images/a.png)\n\n![](images/b.png)");
        assert_eq!(merged(images, text), vec!["a.png".to_string(), "b.png".to_string()]);
        assert_eq!(map_library(text, |n| format!("out/{n}")), "正文\n![](out/a.png)");
    }
}
