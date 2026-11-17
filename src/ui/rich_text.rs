//! Renders memo content: tags, memo references and links are clickable; inline
//! markup becomes bold, highlight, underline, strikethrough and code runs.
//! Block Markdown (headings, fenced code, tables, rules, task lists) is rendered
//! alongside flomo-style lines. `# Title` (space after `#`) is a heading; `#tag`
//! (no space) stays a clickable tag.

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, FontWeight, HighlightStyle, InteractiveText, IntoElement,
    ParentElement as _, SharedString, StrikethroughStyle, Styled as _, StyledText, UnderlineStyle,
    div, px,
};
use gpui_kit::prelude::FluentBuilder as _;

use crate::core::parse::{self, InlineStyle, LineKind, RichLine, Target};

use super::theme::Typography;
use super::{Command, dispatch};

fn highlight_for(style: &InlineStyle, cx: &App) -> HighlightStyle {
    let theme = cx.theme();
    let mut h = HighlightStyle::default();
    if style.bold {
        h.font_weight = Some(FontWeight::BOLD);
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
        h.color = Some(theme.red);
        h.background_color = Some(theme.red.opacity(0.08));
    }
    if style.tag {
        h.color = Some(theme.blue);
        h.background_color = Some(theme.blue.opacity(0.1));
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
        h.color = Some(theme.blue);
        h.background_color = Some(theme.secondary);
    }
    if style.search_hit {
        h.background_color = Some(theme.yellow.opacity(0.6));
    }
    h
}

fn line_text(scope: &SharedString, index: usize, line: &RichLine, cx: &App) -> AnyElement {
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
                Target::Tag(tag) => Command::OpenTag(tag.clone()),
                Target::Memo(id) => Command::OpenMemo(id.clone()),
                Target::Url(url) => Command::OpenUrl(url.clone()),
            };
            dispatch(command, window, cx);
        })
        .into_any_element()
}

fn render_table_block(rows: &[RichLine], cx: &App) -> AnyElement {
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
        .children(rows.iter().map(|row| {
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
                .when(is_header, |r| r.bg(theme.secondary))
                .children(cells.into_iter().map(|cell| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .py_1()
                        .border_r_1()
                        .border_color(theme.border)
                        .text_sm()
                        .when(is_header, |d| d.font_bold())
                        .child(cell)
                }))
                .into_any_element()
        }))
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
    let total_chars: usize = lines.iter().map(|l| l.text.chars().count()).sum();
    let (limit, truncated) = match max_lines {
        Some(max) if lines.len() > max || total_chars > max * 60 => {
            let mut count = 0;
            let mut chars = 0;
            for line in &lines {
                chars += line.text.chars().count().max(20);
                count += 1;
                if count >= max || chars > max * 45 {
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
            blocks.push(render_table_block(&table_rows, cx));
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
                let heading = div().font_bold().mt_1().child(line_text(&scope, idx, line, cx));
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
            LineKind::TableRow { .. } => unreachable!("table rows are grouped above"),
        });
    }

    let element = typography
        .apply(v_flex().gap_1())
        .children(blocks)
        .into_any_element();
    (element, truncated)
}
