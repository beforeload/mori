//! Share images: memo cards and cognitive-map snapshots rendered as SVG, rasterized to PNG.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::{Context as _, Result};
use resvg::{tiny_skia, usvg};

use super::model::Memo;
use super::parse::{self, LineKind};
use super::time;

const FONT_STACK: &str = "Hiragino Sans GB, STHeiti, Songti SC, Heiti SC, sans-serif";

fn fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
    .clone()
}

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Rasterizes an SVG document at `scale` and returns PNG bytes.
pub fn render_png(svg: &str, scale: f32) -> Result<Vec<u8>> {
    let options = usvg::Options {
        fontdb: fontdb(),
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(svg, &options).context("无法解析分享图")?;
    let size = tree.size();
    let width = (size.width() * scale).ceil() as u32;
    let height = (size.height() * scale).ceil() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width.max(1), height.max(1)).context("分享图尺寸无效")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().context("无法编码 PNG")
}

/// A timestamped file path in ~/Downloads/Mori, creating the folder if needed.
pub fn output_path(stem: &str, extension: &str) -> Result<PathBuf> {
    let dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Mori");
    std::fs::create_dir_all(&dir)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let stem: String = stem.chars().map(|c| if matches!(c, '/' | '\\' | ':') { '-' } else { c }).collect();
    Ok(dir.join(format!("{stem}-{stamp}.{extension}")))
}

/// Saves into ~/Downloads/Mori and returns the path.
pub fn save_png(bytes: &[u8], stem: &str) -> Result<PathBuf> {
    let path = output_path(stem, "png")?;
    std::fs::write(&path, bytes)?;
    Ok(path)
}

fn char_width(c: char, font_size: f32) -> f32 {
    let wide = matches!(c as u32, 0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x3FFFD)
        || matches!(c, '，' | '。' | '：' | '；' | '！' | '？' | '“' | '”' | '‘' | '’' | '、' | '（' | '）' | '《' | '》' | '—' | '…');
    if wide {
        font_size
    } else if c == ' ' {
        font_size * 0.3
    } else if c.is_ascii_uppercase() || c == 'm' || c == 'w' {
        font_size * 0.68
    } else {
        font_size * 0.56
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
struct Run {
    bold: bool,
    tag: bool,
    accent: bool,
    highlight: bool,
    underline: bool,
    strike: bool,
}

/// Splits styled text into lines no wider than `max_width`.
fn wrap(text: &str, styles: &[Run], font_size: f32, max_width: f32) -> Vec<Vec<(char, Run)>> {
    let mut lines = vec![Vec::new()];
    let mut width = 0.0;
    for (i, c) in text.chars().enumerate() {
        let w = char_width(c, font_size);
        if width + w > max_width && !lines.last().unwrap().is_empty() {
            lines.push(Vec::new());
            width = 0.0;
        }
        lines.last_mut().unwrap().push((c, styles.get(i).copied().unwrap_or_default()));
        width += w;
    }
    lines
}

fn tspans(line: &[(char, Run)], colors: &ShareColors) -> String {
    let mut out = String::new();
    let mut current: Option<(Run, String)> = None;
    let flush = |out: &mut String, run: Run, text: &str| {
        let fill = if run.tag || run.accent { colors.tag } else { colors.text };
        let weight = if run.bold { "700" } else { "400" };
        let decoration = match (run.underline, run.strike) {
            (true, true) => " text-decoration=\"underline line-through\"",
            (true, false) => " text-decoration=\"underline\"",
            (false, true) => " text-decoration=\"line-through\"",
            (false, false) => "",
        };
        out.push_str(&format!(
            "<tspan fill=\"{fill}\" font-weight=\"{weight}\"{decoration}>{}</tspan>",
            escape(text)
        ));
    };
    for &(c, run) in line {
        match current.as_mut() {
            Some((r, text)) if *r == run => text.push(c),
            _ => {
                if let Some((r, text)) = current.take() {
                    flush(&mut out, r, &text);
                }
                current = Some((run, c.to_string()));
            }
        }
    }
    if let Some((r, text)) = current {
        flush(&mut out, r, &text);
    }
    out
}

struct ShareColors {
    page: &'static str,
    card: &'static str,
    text: &'static str,
    muted: &'static str,
    tag: &'static str,
    accent: &'static str,
    highlight: &'static str,
}

const LIGHT: ShareColors = ShareColors {
    page: "#eef1ef",
    card: "#ffffff",
    text: "#1f2328",
    muted: "#8a8f98",
    tag: "#4f7cf0",
    accent: "#30a46c",
    highlight: "#fbe3a0",
};

const DARK: ShareColors = ShareColors {
    page: "#141615",
    card: "#262727",
    text: "#e8e8e8",
    muted: "#8b8f94",
    tag: "#7d9cf7",
    accent: "#3f9a6a",
    highlight: "#5e4f1f",
};

/// A shareable card image for one memo, flomo style.
pub fn memo_share_svg(memo: &Memo, nickname: &str, dark: bool, ref_label: &dyn Fn(&str) -> String) -> String {
    let colors = if dark { &DARK } else { &LIGHT };
    let width = 750.0f32;
    let card_x = 40.0;
    let inner_x = card_x + 48.0;
    let inner_w = width - 2.0 * card_x - 96.0;
    let font_size = 30.0;
    let line_h = font_size * 1.72;

    let mut body = String::new();
    let mut y = 40.0 + 120.0;
    for line in parse::parse_rich(&memo.content, &[], ref_label) {
        if line.kind == LineKind::Blank {
            y += line_h * 0.5;
            continue;
        }
        let (prefix, indent) = match &line.kind {
            LineKind::Bullet => ("•".to_string(), 34.0),
            LineKind::Ordered(n) => (format!("{n}."), 40.0),
            LineKind::Quote => ("│".to_string(), 30.0),
            _ => (String::new(), 0.0),
        };
        let styles: Vec<Run> = line
            .text
            .char_indices()
            .map(|(byte, _)| {
                let style = line
                    .styles
                    .iter()
                    .find(|(r, _)| r.contains(&byte))
                    .map(|(_, s)| *s)
                    .unwrap_or_default();
                Run {
                    bold: style.bold,
                    tag: style.tag,
                    accent: style.link || style.memo_ref,
                    highlight: style.highlight,
                    underline: style.underline,
                    strike: style.strike,
                }
            })
            .collect();
        let wrapped = wrap(&line.text, &styles, font_size, inner_w - indent);
        for (i, visual) in wrapped.iter().enumerate() {
            if i == 0 && !prefix.is_empty() {
                body.push_str(&format!(
                    "<text x=\"{inner_x}\" y=\"{y}\" font-size=\"{font_size}\" fill=\"{}\">{}</text>",
                    colors.muted,
                    escape(&prefix)
                ));
            }
            let mut x = inner_x + indent;
            for &(c, run) in visual {
                let w = char_width(c, font_size);
                if run.highlight {
                    body.push_str(&format!(
                        "<rect x=\"{x}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
                        y - font_size * 0.92,
                        w + 0.5,
                        font_size * 1.2,
                        colors.highlight
                    ));
                }
                x += w;
            }
            body.push_str(&format!(
                "<text x=\"{}\" y=\"{y}\" font-size=\"{font_size}\">{}</text>",
                inner_x + indent,
                tspans(visual, colors)
            ));
            y += line_h;
        }
    }
    let last_baseline = y - line_h;
    let card_h = (last_baseline + 60.0 - 40.0).max(170.0);
    let height = card_h + 40.0 + 90.0;
    let footer_y = 40.0 + card_h + 58.0;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" font-family="{FONT_STACK}">
<rect width="{width}" height="{height}" fill="{page}"/>
<rect x="{card_x}" y="40" width="{cw}" height="{card_h}" rx="24" fill="{card}"/>
<text x="{inner_x}" y="104" font-size="22" fill="{muted}">{date}</text>
{body}
<circle cx="{dot_x}" cy="{dot_y}" r="9" fill="{accent}"/>
<text x="{text_x}" y="{footer_y}" font-size="22" fill="{muted}">{name} · {count} {unit} · Mori</text>
</svg>"##,
        page = colors.page,
        card = colors.card,
        muted = colors.muted,
        accent = colors.accent,
        cw = width - 2.0 * card_x,
        date = time::fmt_minute(memo.created_at),
        dot_x = card_x + 10.0,
        dot_y = footer_y - 8.0,
        text_x = card_x + 30.0,
        name = escape(nickname),
        count = memo.char_count,
        unit = t!("字"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_card_renders_png() {
        let mut memo = Memo {
            id: "a".into(),
            content: "**读书**笔记：持续不断记录，==意义自然浮现==。This line mixes English words to test wrapping width.\n- 第一点\n- 第二点 #读书/方法".into(),
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            deleted_at: None,
            pinned: false,
            source: "app".into(),
            images: vec![],
            tags: vec![],
            refs: vec![],
            char_count: 0,
            has_link: false,
        };
        memo.refresh_derived();
        let svg = memo_share_svg(&memo, "Mori", false, &|_| String::new());
        let png = render_png(&svg, 1.0).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(png.len() > 5000);
    }
}
