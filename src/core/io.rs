//! Import and export.
//!
//! Export: JSON (complete, re-importable), Markdown and HTML (images copied beside the file).
//! Import: Mori JSON, flomo HTML export, Markdown/TXT, CSV (Day One and generic),
//! and text copied from WeChat Read (微信读书).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context as _, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::model::Memo;
use super::store::{NewMemo, Store};
use super::time;

#[derive(Clone, Debug, Default)]
pub struct ImportReport {
    pub imported: usize,
    pub skipped: usize,
    pub files: usize,
}

impl ImportReport {
    pub fn summary(&self) -> String {
        if self.skipped > 0 {
            t!("导入 {} 条笔记，跳过 {} 条重复内容", self.imported, self.skipped)
        } else {
            t!("导入 {} 条笔记", self.imported)
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ExportFile {
    app: String,
    version: u32,
    exported_at: String,
    memos: Vec<Memo>,
}

fn active_sorted(store: &Store) -> Vec<&Memo> {
    let mut memos: Vec<&Memo> = store.active_memos().collect();
    memos.sort_by_key(|m| std::cmp::Reverse(m.created_at));
    memos
}

pub fn export_json(store: &Store, path: &Path) -> Result<usize> {
    let memos: Vec<Memo> = active_sorted(store).into_iter().cloned().collect();
    let count = memos.len();
    let file = ExportFile {
        app: "Mori".into(),
        version: 1,
        exported_at: time::fmt_datetime(time::now()),
        memos,
    };
    std::fs::write(path, serde_json::to_string_pretty(&file)?)?;
    Ok(count)
}

fn copy_images(store: &Store, memo: &Memo, target_dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if memo.images.is_empty() {
        return names;
    }
    let _ = std::fs::create_dir_all(target_dir);
    for image in &memo.images {
        let source = store.image_path(image);
        if std::fs::copy(&source, target_dir.join(image)).is_ok() {
            names.push(image.clone());
        }
    }
    names
}

fn images_dir_for(path: &Path) -> (PathBuf, String) {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("mori")
        .to_string();
    let folder = format!("{stem}_images");
    (path.with_file_name(&folder), folder)
}

pub fn export_markdown(store: &Store, path: &Path) -> Result<usize> {
    export_markdown_memos(store, &active_sorted(store), t!("Mori 笔记导出"), path)
}

/// Writes `memos` in the given order; images are copied next to the file.
pub fn export_markdown_memos(store: &Store, memos: &[&Memo], heading: &str, path: &Path) -> Result<usize> {
    let (image_dir, folder) = images_dir_for(path);
    let mut out = format!("# {heading}\n\n");
    for memo in memos {
        out.push_str(&format!("## {}\n\n{}\n", time::fmt_datetime(memo.created_at), memo.content.trim()));
        for image in copy_images(store, memo, &image_dir) {
            out.push_str(&format!("\n![]({folder}/{image})\n"));
        }
        out.push_str("\n---\n\n");
    }
    std::fs::write(path, out)?;
    Ok(memos.len())
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn export_html(store: &Store, path: &Path) -> Result<usize> {
    let (image_dir, folder) = images_dir_for(path);
    let memos = active_sorted(store);
    let title = t!("Mori 笔记导出");
    let mut out = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
<style>body{{font-family:-apple-system,'PingFang SC',sans-serif;max-width:720px;margin:40px auto;color:#222;background:#f6f6f4}}\
.memo{{background:#fff;border-radius:10px;padding:16px 20px;margin:12px 0}}.time{{color:#999;font-size:12px;margin-bottom:8px}}\
.content p{{margin:4px 0;line-height:1.7}}.files img{{max-width:200px;border-radius:6px;margin:6px 6px 0 0}}</style></head><body>\n",
    );
    out.push_str(&format!("<h2>{}</h2>\n", t!("Mori 笔记导出（{} 条）", memos.len())));
    for memo in &memos {
        out.push_str("<div class=\"memo\">");
        out.push_str(&format!("<div class=\"time\">{}</div><div class=\"content\">", time::fmt_datetime(memo.created_at)));
        for line in memo.content.lines() {
            out.push_str(&format!("<p>{}</p>", escape_html(line)));
        }
        out.push_str("</div><div class=\"files\">");
        for image in copy_images(store, memo, &image_dir) {
            out.push_str(&format!("<img src=\"{folder}/{image}\" />"));
        }
        out.push_str("</div></div>\n");
    }
    out.push_str("</body></html>\n");
    std::fs::write(path, out)?;
    Ok(memos.len())
}

// -------------------------------------------------------------- import

struct Importer<'a> {
    store: &'a mut Store,
    existing: HashSet<String>,
    skip_duplicates: bool,
    pending: Vec<NewMemo>,
    report: ImportReport,
}

impl<'a> Importer<'a> {
    fn new(store: &'a mut Store, skip_duplicates: bool) -> Self {
        let existing = store
            .active_memos()
            .map(|m| normalize(&m.content))
            .collect();
        Self {
            store,
            existing,
            skip_duplicates,
            pending: Vec::new(),
            report: ImportReport::default(),
        }
    }

    fn add(&mut self, content: String, created_at: Option<i64>, images: Vec<String>) {
        let content = content.trim().to_string();
        if content.is_empty() && images.is_empty() {
            return;
        }
        let key = normalize(&content);
        if self.skip_duplicates && !key.is_empty() && self.existing.contains(&key) {
            self.report.skipped += 1;
            return;
        }
        self.existing.insert(key);
        self.pending.push(NewMemo {
            content,
            created_at,
            images,
            source: "import".into(),
        });
    }

    fn finish(mut self) -> Result<ImportReport> {
        self.report.imported = self.pending.len();
        let pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() {
            self.store.create_many(pending)?;
        }
        Ok(self.report)
    }
}

fn normalize(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn import_files(store: &mut Store, paths: &[PathBuf], skip_duplicates: bool) -> Result<ImportReport> {
    let mut importer = Importer::new(store, skip_duplicates);
    for path in paths {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let text = std::fs::read_to_string(path)
            .with_context(|| t!("无法读取 {}", path.display()))?;
        match ext.as_str() {
            "json" => import_json(&mut importer, &text)?,
            "html" | "htm" => import_flomo_html(&mut importer, &text, path.parent()),
            "csv" => import_csv(&mut importer, &text),
            "md" | "markdown" | "txt" => {
                let modified = std::fs::metadata(path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64);
                import_markdown(&mut importer, &text, modified)
            }
            _ => bail!("不支持的文件类型：{}", path.display()),
        }
        importer.report.files += 1;
    }
    importer.finish()
}

fn import_json(importer: &mut Importer, text: &str) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(text).context("JSON 格式不正确")?;
    let list = value
        .get("memos")
        .cloned()
        .unwrap_or(value)
        .as_array()
        .cloned()
        .context("JSON 中没有找到笔记列表")?;
    for item in list {
        let Some(content) = item.get("content").and_then(|c| c.as_str()) else {
            continue;
        };
        let created_at = item.get("created_at").and_then(|c| {
            c.as_i64()
                .or_else(|| c.as_str().and_then(time::parse_datetime))
        });
        importer.add(content.to_string(), created_at, Vec::new());
    }
    Ok(())
}

static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static IMG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<img[^>]*?src\s*=\s*["']([^"']+)["']"#).unwrap());
static TIME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<div class="time">(.*?)</div>"#).unwrap());
static CONTENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<div class="content">(.*?)</div>\s*(?:<div class="files">|$)"#).unwrap());

pub fn html_to_text(html: &str) -> String {
    let mut s = html.replace("\r", "");
    for (from, to) in [
        ("<br>", "\n"),
        ("<br/>", "\n"),
        ("<br />", "\n"),
        ("</p>", "\n"),
        ("</li>", "\n"),
        ("<li>", "- "),
        ("<strong>", "**"),
        ("</strong>", "**"),
        ("<b>", "**"),
        ("</b>", "**"),
        ("<mark>", "=="),
        ("</mark>", "=="),
        ("<u>", "__"),
        ("</u>", "__"),
        ("</div>", "\n"),
    ] {
        s = s.replace(from, to);
    }
    let stripped = TAG_RE.replace_all(&s, "");
    let decoded = decode_entities(&stripped);
    let mut out = String::new();
    let mut blank = 0;
    for line in decoded.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

fn decode_entities(text: &str) -> String {
    static NUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&#(x?[0-9a-fA-F]+);").unwrap());
    let replaced = NUM_RE.replace_all(text, |caps: &regex::Captures| {
        let raw = &caps[1];
        let code = if let Some(hex) = raw.strip_prefix('x') {
            u32::from_str_radix(hex, 16).ok()
        } else {
            raw.parse().ok()
        };
        code.and_then(char::from_u32).map(|c| c.to_string()).unwrap_or_default()
    });
    replaced
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn import_flomo_html(importer: &mut Importer, html: &str, base: Option<&Path>) {
    let blocks: Vec<&str> = html.split("<div class=\"memo\">").skip(1).collect();
    if blocks.is_empty() {
        importer.add(html_to_text(html), None, Vec::new());
        return;
    }
    for block in blocks {
        let created_at = TIME_RE
            .captures(block)
            .and_then(|c| time::parse_datetime(&html_to_text(&c[1])));
        let content_html = CONTENT_RE
            .captures(block)
            .map(|c| c[1].to_string())
            .unwrap_or_else(|| block.to_string());
        let content = html_to_text(&content_html);
        let mut images = Vec::new();
        if let Some(files_start) = block.find("<div class=\"files\">") {
            for caps in IMG_RE.captures_iter(&block[files_start..]) {
                let src = &caps[1];
                let candidate = base.map(|b| b.join(src)).unwrap_or_else(|| PathBuf::from(src));
                if candidate.exists()
                    && let Ok(name) = importer.store.import_image_file(&candidate)
                {
                    images.push(name);
                }
            }
        }
        importer.add(content, created_at, images);
    }
}

static MD_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##\s+(\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}(?::\d{2})?)\s*$").unwrap());

fn import_markdown(importer: &mut Importer, text: &str, file_time: Option<i64>) {
    let text = text.replace("\r\n", "\n");
    let has_headers = text.lines().any(|l| MD_HEADER_RE.is_match(l));
    if has_headers {
        let mut current: Option<(Option<i64>, String)> = None;
        for line in text.lines() {
            if let Some(caps) = MD_HEADER_RE.captures(line) {
                if let Some((ts, body)) = current.take() {
                    importer.add(clean_md_body(&body), ts, Vec::new());
                }
                current = Some((time::parse_datetime(&caps[1]), String::new()));
            } else if let Some((_, body)) = current.as_mut() {
                body.push_str(line);
                body.push('\n');
            }
        }
        if let Some((ts, body)) = current {
            importer.add(clean_md_body(&body), ts, Vec::new());
        }
        return;
    }
    let parts: Vec<&str> = text.split("\n---\n").collect();
    if parts.len() > 1 {
        for part in parts {
            importer.add(part.to_string(), file_time, Vec::new());
        }
    } else {
        importer.add(text, file_time, Vec::new());
    }
}

fn clean_md_body(body: &str) -> String {
    let trimmed = body.trim();
    let trimmed = trimmed.strip_suffix("---").unwrap_or(trimmed);
    trimmed
        .lines()
        .filter(|l| !l.trim_start().starts_with("![]("))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => in_quotes = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    rows
}

fn import_csv(importer: &mut Importer, text: &str) {
    let rows = parse_csv(text);
    let Some(header) = rows.first() else {
        return;
    };
    let lower: Vec<String> = header.iter().map(|h| h.trim().to_lowercase()).collect();
    let find = |names: &[&str]| lower.iter().position(|h| names.iter().any(|n| h == n));
    let content_col = find(&["content", "text", "内容", "正文", "memo", "note", "body", "笔记"]);
    let date_col = find(&[
        "date", "created", "created_at", "creationdate", "time", "日期", "时间", "创建时间",
    ]);
    let (data, content_col, date_col) = match content_col {
        Some(c) => (&rows[1..], c, date_col),
        None => (&rows[..], header.len().saturating_sub(1), None),
    };
    for row in data {
        let content = row.get(content_col).cloned().unwrap_or_default();
        let created_at = date_col
            .and_then(|d| row.get(d))
            .and_then(|d| time::parse_datetime(d));
        importer.add(content, created_at, Vec::new());
    }
}

/// Text copied from WeChat Read's "复制全部笔记".
pub fn import_wechat_read(store: &mut Store, text: &str, skip_duplicates: bool) -> Result<ImportReport> {
    let mut importer = Importer::new(store, skip_duplicates);
    let mut book = String::new();
    let mut chapter = String::new();
    let mut highlight: Option<String> = None;
    let mut thought = String::new();
    let mut entries: Vec<(String, String, String)> = Vec::new();

    let mut flush = |highlight: &mut Option<String>, thought: &mut String, chapter: &str| {
        if let Some(h) = highlight.take() {
            entries.push((h, thought.trim().to_string(), chapter.to_string()));
        }
        thought.clear();
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if book.is_empty() && line.starts_with('《') {
            book = line
                .trim_start_matches('《')
                .split('》')
                .next()
                .unwrap_or("")
                .to_string();
            continue;
        }
        if let Some(rest) = line.strip_prefix('◆') {
            flush(&mut highlight, &mut thought, &chapter);
            chapter = rest.trim().to_string();
            continue;
        }
        if let Some(rest) = line.strip_prefix(">>") {
            flush(&mut highlight, &mut thought, &chapter);
            highlight = Some(rest.trim().to_string());
            continue;
        }
        if highlight.is_some() {
            if line.ends_with("发表想法") || time::parse_datetime(line).is_some() {
                continue;
            }
            thought.push_str(line);
            thought.push('\n');
        }
    }
    flush(&mut highlight, &mut thought, &chapter);

    if entries.is_empty() {
        bail!("没有识别到微信读书笔记，请粘贴「复制全部笔记」得到的内容");
    }
    let tag = if book.is_empty() {
        "读书".to_string()
    } else {
        format!("读书/{}", book.replace(char::is_whitespace, "_"))
    };
    for (highlight, thought, chapter) in entries {
        let mut content = String::new();
        if !thought.is_empty() {
            content.push_str(&thought);
            content.push_str("\n\n");
        }
        content.push_str(&format!("> {highlight}\n"));
        if !chapter.is_empty() {
            content.push_str(&format!("\n—— {chapter}"));
        }
        content.push_str(&format!("\n#{tag}"));
        importer.add(content, None, Vec::new());
    }
    importer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_conversion() {
        let t = html_to_text("<p>你好 <strong>世界</strong></p><ul><li>一</li><li>二</li></ul><p>&amp;&#x4F60;</p>");
        assert_eq!(t, "你好 **世界**\n- 一\n- 二\n&你");
    }

    fn temp_store() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("mori-io-{}", super::super::store::new_id()));
        (Store::open(&dir).unwrap(), dir)
    }

    #[test]
    fn json_roundtrip_and_markdown_export() {
        let (mut store, dir) = temp_store();
        store
            .create_many(vec![
                NewMemo { content: "第一条 #甲".into(), created_at: Some(1_700_000_000), images: vec![], source: "app".into() },
                NewMemo { content: "第二条 **粗**".into(), created_at: Some(1_700_100_000), images: vec![], source: "app".into() },
            ])
            .unwrap();
        let json = dir.join("out.json");
        assert_eq!(export_json(&store, &json).unwrap(), 2);
        let md = dir.join("out.md");
        assert_eq!(export_markdown(&store, &md).unwrap(), 2);
        assert_eq!(export_html(&store, &dir.join("out.html")).unwrap(), 2);

        let (mut other, _) = temp_store();
        let report = import_files(&mut other, &[json.clone()], true).unwrap();
        assert_eq!(report.imported, 2);
        let again = import_files(&mut other, &[json, md], true).unwrap();
        assert_eq!(again.imported, 0);
        assert_eq!(again.skipped, 4);
        assert!(other.active_memos().any(|m| m.created_at == 1_700_000_000 && m.tags == vec!["甲"]));
    }

    #[test]
    fn flomo_html_import_with_image() {
        let (mut store, dir) = temp_store();
        std::fs::create_dir_all(dir.join("file")).unwrap();
        std::fs::write(dir.join("file/a.png"), b"png").unwrap();
        let html = r#"<div class="memo"><div class="time">2023-03-01 10:20:30</div><div class="content"><p>读书 <strong>笔记</strong> #读书</p><ul><li>要点</li></ul></div><div class="files"><img src="file/a.png" /></div></div>
<div class="memo"><div class="time">2023-03-02 08:00:00</div><div class="content"><p>第二条</p></div></div>"#;
        let path = dir.join("index.html");
        std::fs::write(&path, html).unwrap();
        let report = import_files(&mut store, &[path], true).unwrap();
        assert_eq!(report.imported, 2);
        let memo = store.active_memos().find(|m| m.content.contains("笔记")).unwrap();
        assert_eq!(memo.content, "读书 **笔记** #读书\n- 要点");
        assert_eq!(memo.images.len(), 1);
        assert_eq!(super::time::fmt_datetime(memo.created_at), "2023-03-01 10:20:30");
    }

    #[test]
    fn wechat_read_import() {
        let (mut store, _) = temp_store();
        let text = "《原则》\n瑞·达利欧\n\n◆ 第一部分\n\n>> 痛苦+反思=进步\n\n我的想法：值得每天复盘\n\n>> 极度透明\n";
        let report = import_wechat_read(&mut store, text, true).unwrap();
        assert_eq!(report.imported, 2);
        assert!(store.active_memos().all(|m| m.tags == vec!["读书/原则"]));
        assert!(store.active_memos().any(|m| m.content.starts_with("我的想法：值得每天复盘")));
    }

    #[test]
    fn csv_parsing() {
        let rows = parse_csv("date,text\n2024-01-01,\"a, \"\"quoted\"\"\nline\"\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][1], "a, \"quoted\"\nline");
    }
}
