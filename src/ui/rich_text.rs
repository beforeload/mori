//! Renders memo content: tags, memo references and links are clickable; inline
//! markup becomes bold, highlight, underline, strikethrough and code runs.
//! Block Markdown (headings, fenced code, tables, rules, task lists, images) is
//! rendered alongside flomo-style lines. `# Title` (space after `#`) is a heading;
//! `#tag` (no space) stays a clickable tag.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, FontWeight, HighlightStyle, InteractiveElement as _, InteractiveText, IntoElement,
    ObjectFit, ParentElement as _, SharedString, StatefulInteractiveElement as _, StrikethroughStyle,
    Styled as _, StyledImage as _, StyledText, UnderlineStyle, div, img, px, relative,
};
use gpui_kit::prelude::FluentBuilder as _;

use crate::core::images;
use crate::core::parse::{self, InlineStyle, LineKind, RichLine, Target};

use super::library::Library;
use super::theme::Typography;
use super::{Command, Lucide, dispatch};

const IMAGE_MAX_WIDTH: f32 = 640.;
const IMAGE_MAX_HEIGHT: f32 = 400.;
/// A shown image takes about as much room as this many text lines when folding.
const IMAGE_LINES: usize = 4;

/// Narrow no-break space: pads chip backgrounds without letting a line wrap inside them.
const CHIP_PAD: char = '\u{202F}';

fn highlight_for(style: &InlineStyle, cx: &App) -> HighlightStyle {
    let theme = cx.theme();
    let mut h = HighlightStyle::default();
    if style.bold {
        h.font_weight = Some(FontWeight::SEMIBOLD);
    }
    if style.highlight {
        h.background_color = Some(theme.yellow.opacity(0.35));
    }
    if style.underline {
        h.underline = Some(UnderlineStyle {
            thickness: px(1.),
            color: None,
            wavy: false,
        });
    }
    if style.strike {
        h.strikethrough = Some(StrikethroughStyle {
            thickness: px(1.),
            color: None,
        });
    }
    if style.code {
        h.color = Some(theme.red.opacity(0.85));
        h.background_color = Some(theme.red.opacity(0.07));
    }
    if style.tag {
        h.color = Some(theme.link);
        h.background_color = Some(theme.link.opacity(0.1));
    }
    if style.link {
        h.color = Some(theme.blue);
        h.underline = Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(theme.blue.opacity(0.4)),
            wavy: false,
        });
    }
    if style.memo_ref {
        h.color = Some(theme.link);
        h.background_color = Some(theme.link.opacity(0.07));
    }
    if style.search_hit {
        h.background_color = Some(theme.yellow.opacity(0.6));
    }
    h
}

/// Pads tags, note links and inline code so their background reads as a chip.
fn padded(line: &RichLine) -> RichLine {
    let mut chips: Vec<Range<usize>> = Vec::new();
    for (range, style) in &line.styles {
        if !(style.tag || style.memo_ref || style.code) {
            continue;
        }
        match chips.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => chips.push(range.clone()),
        }
    }
    if chips.is_empty() {
        return line.clone();
    }
    let pad = CHIP_PAD.len_utf8();
    let map = |offset: usize| {
        offset
            + 2 * pad * chips.iter().filter(|c| c.end <= offset).count()
            + pad * chips.iter().filter(|c| c.start < offset && offset < c.end).count()
    };
    let mut text = String::with_capacity(line.text.len() + chips.len() * 2 * pad);
    let mut last = 0;
    for chip in &chips {
        text.push_str(&line.text[last..chip.start]);
        text.push(CHIP_PAD);
        text.push_str(&line.text[chip.clone()]);
        text.push(CHIP_PAD);
        last = chip.end;
    }
    text.push_str(&line.text[last..]);
    RichLine {
        kind: line.kind.clone(),
        text,
        styles: line.styles.iter().map(|(r, s)| (map(r.start)..map(r.end), *s)).collect(),
        targets: line.targets.iter().map(|(r, t)| (map(r.start)..map(r.end), t.clone())).collect(),
    }
}

fn line_text(scope: &SharedString, index: usize, line: &RichLine, cx: &App) -> AnyElement {
    let line = &padded(line);
    let highlights: Vec<_> = line
        .styles
        .iter()
        .map(|(range, style)| (range.clone(), highlight_for(style, cx)))
        .collect();
    let styled = StyledText::new(line.text.clone()).with_highlights(highlights);
    if line.targets.is_empty() {
        return styled.into_any_element();
    }
    let ranges: Vec<_> = line.targets.iter().map(|(r, _)| r.clone()).collect();
    let targets: Vec<Target> = line.targets.iter().map(|(_, t)| t.clone()).collect();
    InteractiveText::new((scope.clone(), index), styled)
        .on_click(ranges, move |ix, window, cx| {
            let command = match &targets[ix] {
                Target::Tag(tag) => Some(Command::OpenTag(tag.clone())),
                Target::Memo(id) => Some(Command::OpenMemo(id.clone())),
                Target::Wiki(name) => crate::ui::library::Library::global(cx)
                    .read(cx)
                    .store()
                    .resolve_wiki(name)
                    .into_iter()
                    .next()
                    .map(Command::OpenMemo),
                Target::Url(url) => Some(Command::OpenUrl(url.clone())),
            };
            if let Some(command) = command {
                dispatch(command, window, cx);
            }
        })
        .into_any_element()
}

fn render_table_block(
    scope: &SharedString,
    rows: &[RichLine],
    keywords: &[String],
    ref_label: &dyn Fn(&str) -> String,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let cols = rows
        .iter()
        .filter_map(|r| match &r.kind {
            LineKind::TableRow { cells, .. } => Some(cells.len()),
            _ => None,
        })
        .max()
        .unwrap_or(1)
        .max(1);
    v_flex()
        .w_full()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .children(rows.iter().enumerate().map(|(r, row)| {
            let (is_header, cells) = match &row.kind {
                LineKind::TableRow { is_header, cells } => (*is_header, cells.clone()),
                _ => (false, vec![row.text.clone()]),
            };
            let mut cells = cells;
            while cells.len() < cols {
                cells.push(String::new());
            }
            h_flex()
                .w_full()
                .border_b_1()
                .border_color(theme.border)
                .when(is_header, |d| d.bg(theme.secondary))
                .children(cells.into_iter().enumerate().map(|(c, cell)| {
                    let rich = parse::parse_cell(&cell, keywords, ref_label);
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .py_1()
                        .border_r_1()
                        .border_color(theme.border)
                        .text_sm()
                        .when(is_header, |d| d.font_semibold())
                        .child(line_text(scope, r * cols + c, &rich, cx))
                }))
                .into_any_element()
        }))
        .into_any_element()
}

type PixelSize = (f32, f32);

/// Pixel size from the file header, cached; library image files never change.
pub fn image_size(path: &Path) -> Option<PixelSize> {
    static SIZES: LazyLock<Mutex<HashMap<PathBuf, Option<PixelSize>>>> = LazyLock::new(Default::default);
    let mut sizes = SIZES.lock().ok()?;
    *sizes.entry(path.to_path_buf()).or_insert_with(|| {
        imagesize::size(path)
            .ok()
            .filter(|s| s.width > 0 && s.height > 0)
            .map(|s| (s.width as f32, s.height as f32))
    })
}

fn render_image(scope: &SharedString, index: usize, src: &str, alt: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let id = SharedString::from(format!("{scope}-img-{index}"));
    let local = match images::library_name(src) {
        Some(name) => Some(Library::global(cx).read(cx).store().image_path(name)),
        None => {
            let path = Path::new(src.strip_prefix("file://").unwrap_or(src));
            path.is_absolute().then(|| path.to_path_buf())
        }
    };
    if let Some(path) = local.filter(|p| p.is_file()) {
        let (w, h) = image_size(&path).unwrap_or((IMAGE_MAX_WIDTH, IMAGE_MAX_HEIGHT));
        let scale = (IMAGE_MAX_WIDTH / w).min(IMAGE_MAX_HEIGHT / h).min(1.);
        let fallback_text: SharedString = if alt.is_empty() { t!("图片无法显示").into() } else { alt.to_string().into() };
        let open = path.clone();
        // Top padding in percent resolves against the box's width, so the box keeps the
        // image's proportions in narrow panels too. (`aspect_ratio` on a percentage-width
        // box inflates the measured height of the card around it.)
        return div()
            .id(id)
            .w_full()
            .max_w(px(w * scale))
            .my_1()
            .rounded_md()
            .overflow_hidden()
            .cursor_pointer()
            .child(
                div().relative().w_full().pt(relative(h / w)).child(
                    img(path)
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .object_fit(ObjectFit::Contain)
                        .with_fallback(move || {
                            div().size_full().flex().items_center().justify_center().text_xs().text_color(muted).child(fallback_text.clone()).into_any_element()
                        }),
                ),
            )
            .on_click(move |_, _, cx| cx.open_with_system(&open))
            .into_any_element();
    }
    let remote = src.starts_with("http://") || src.starts_with("https://");
    let label = match (remote, alt.is_empty()) {
        (true, true) => src.to_string(),
        (true, false) => alt.to_string(),
        (false, _) => t!("找不到图片：{}", src),
    };
    let url = src.to_string();
    h_flex()
        .id(id)
        .gap_1p5()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(theme.secondary)
        .text_xs()
        .text_color(muted)
        .child(Icon::new(Lucide::Image).xsmall())
        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label))
        .when(remote, |chip| {
            chip.cursor_pointer()
                .text_color(theme.link)
                .on_click(move |_, window, cx| dispatch(Command::OpenUrl(url.clone()), window, cx))
        })
        .into_any_element()
}

/// Renders memo content. With `max_lines`, returns whether content was cut.
pub fn render_content(
    scope: impl Into<SharedString>,
    content: &str,
    keywords: &[String],
    max_lines: Option<usize>,
    ref_label: &dyn Fn(&str) -> String,
    typography: Typography,
    cx: &App,
) -> (AnyElement, bool) {
    let scope: SharedString = scope.into();
    let lines = parse::parse_rich(content, keywords, ref_label);
    let weight = |line: &RichLine| if matches!(line.kind, LineKind::Image { .. }) { IMAGE_LINES } else { 1 };
    let total_chars: usize = lines.iter().map(|l| l.text.chars().count()).sum();
    let total_weight: usize = lines.iter().map(weight).sum();
    let (limit, truncated) = match max_lines {
        Some(max) if total_weight > max || total_chars > max * 60 => {
            let mut count = 0;
            let mut used = 0;
            let mut chars = 0;
            for line in &lines {
                chars += line.text.chars().count().max(20);
                used += weight(line);
                count += 1;
                if used >= max || chars > max * 45 {
                    break;
                }
            }
            (count.max(1), true)
        }
        _ => (lines.len(), false),
    };
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    let secondary = cx.theme().secondary;
    let mono = cx.theme().mono_font_family.clone();
    let visible: Vec<&RichLine> = lines.iter().take(limit).collect();

    // Group consecutive table rows into one visual table.
    let mut blocks: Vec<AnyElement> = Vec::new();
    let mut i = 0;
    let mut text_index = 0usize;
    while i < visible.len() {
        if matches!(visible[i].kind, LineKind::TableRow { .. }) {
            let start = i;
            i += 1;
            while i < visible.len() && matches!(visible[i].kind, LineKind::TableRow { .. }) {
                i += 1;
            }
            let table_rows: Vec<RichLine> = visible[start..i].iter().map(|l| (*l).clone()).collect();
            let table_scope = SharedString::from(format!("{scope}-table-{start}"));
            blocks.push(render_table_block(&table_scope, &table_rows, keywords, ref_label, cx));
            continue;
        }
        let line = visible[i];
        let idx = text_index;
        text_index += 1;
        i += 1;
        blocks.push(match &line.kind {
            LineKind::Blank => div().h_2().into_any_element(),
            LineKind::Rule => div()
                .w_full()
                .h(px(1.))
                .my_2()
                .bg(border)
                .into_any_element(),
            LineKind::Heading(level) => {
                let heading = div().font_semibold().mt_1().child(line_text(&scope, idx, line, cx));
                match level {
                    1 => heading.text_xl(),
                    2 => heading.text_lg(),
                    3 => heading.text_base(),
                    _ => heading.text_sm(),
                }
                .into_any_element()
            }
            LineKind::CodeBlock { lang } => v_flex()
                .w_full()
                .gap_1()
                .rounded_md()
                .bg(secondary)
                .border_1()
                .border_color(border)
                .px_3()
                .py_2()
                .when_some(lang.clone(), |el, lang| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .font_family(mono.clone())
                            .child(lang),
                    )
                })
                .child(
                    v_flex()
                        .font_family(mono.clone())
                        .text_sm()
                        .children(line.text.split('\n').map(|row| {
                            div().min_h(px(18.)).child(if row.is_empty() {
                                " ".to_string()
                            } else {
                                row.to_string()
                            })
                        })),
                )
                .into_any_element(),
            LineKind::Property => div()
                .text_xs()
                .text_color(muted)
                .child(line_text(&scope, idx, line, cx))
                .into_any_element(),
            LineKind::Paragraph => div().child(line_text(&scope, idx, line, cx)).into_any_element(),
            LineKind::Bullet => h_flex()
                .items_start()
                .gap_2()
                .child(div().text_color(muted).child("•"))
                .child(div().flex_1().min_w_0().child(line_text(&scope, idx, line, cx)))
                .into_any_element(),
            LineKind::Ordered(n) => h_flex()
                .items_start()
                .gap_1p5()
                .child(div().text_color(muted).child(format!("{n}.")))
                .child(div().flex_1().min_w_0().child(line_text(&scope, idx, line, cx)))
                .into_any_element(),
            LineKind::Quote => div()
                .border_l_2()
                .border_color(border)
                .pl_3()
                .text_color(muted)
                .child(line_text(&scope, idx, line, cx))
                .into_any_element(),
            LineKind::Task { checked } => h_flex()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .text_color(if *checked { muted } else { cx.theme().foreground })
                        .child(if *checked { "☑" } else { "☐" }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .when(*checked, |d| d.text_color(muted))
                        .child(line_text(&scope, idx, line, cx)),
                )
                .into_any_element(),
            LineKind::Image { src } => render_image(&scope, idx, src, &line.text, cx),
            LineKind::TableRow { .. } => unreachable!("table rows are grouped above"),
        });
    }

    let element = typography
        .apply(v_flex().gap_1())
        .children(blocks)
        .into_any_element();
    (element, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_are_padded_on_char_boundaries() {
        let lines = parse::parse_rich("看 #读书/方法 和 `代码` 以及 [[项目]] 结尾", &["读书".into()], &|_| String::new());
        let line = padded(&lines[0]);
        for (range, _) in &line.styles {
            assert!(line.text.is_char_boundary(range.start) && line.text.is_char_boundary(range.end));
        }
        let tag = line.targets.iter().find(|(_, t)| matches!(t, Target::Tag(_))).unwrap().0.clone();
        assert_eq!(&line.text[tag], format!("{CHIP_PAD}#读书/方法{CHIP_PAD}"));
        assert_eq!(line.text.matches(CHIP_PAD).count(), 6);
    }
}
