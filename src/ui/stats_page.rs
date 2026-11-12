use chrono::{Datelike, NaiveDate};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px, relative, rems,
};

use crate::core::stats::{self, weekday_label};
use crate::core::time;

use super::sidebar::sidebar_toggle;
use super::library::Library;
use super::{Command, Lucide, dispatch};

pub struct StatsPage {
    month: NaiveDate,
    year: i32,
    _subscriptions: Vec<Subscription>,
}

fn panel(title: &str, cx: &App) -> gpui_kit::Div {
    v_flex()
        .gap_3()
        .p_4()
        .rounded_lg()
        .bg(cx.theme().background)
        .border_1()
        .border_color(cx.theme().border)
        .child(div().text_sm().font_semibold().child(title.to_string()))
}

fn vertical_bars(id: &str, values: &[(String, usize)], color: Hsla, cx: &App) -> AnyElement {
    let max = values.iter().map(|(_, v)| *v).max().unwrap_or(0).max(1);
    let theme = cx.theme();
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .h_32()
                .items_end()
                .gap_1()
                .children(values.iter().enumerate().map(|(i, (label, v))| {
                    let tip = format!("{label}：{v}");
                    div()
                        .id(SharedString::from(format!("{id}-{i}")))
                        .flex_1()
                        .h(relative((*v as f32 / max as f32).max(0.02)))
                        .rounded_t_sm()
                        .bg(if *v == 0 { theme.secondary } else { color })
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                })),
        )
        .child(h_flex().gap_1().children(values.iter().map(|(label, _)| {
            div()
                .flex_1()
                .text_xs()
                .text_color(theme.muted_foreground)
                .flex()
                .justify_center()
                .child(label.clone())
        })))
        .into_any_element()
}

fn horizontal_bars(values: &[(String, usize)], color: Hsla, cx: &App) -> AnyElement {
    let max = values.iter().map(|(_, v)| *v).max().unwrap_or(0).max(1);
    let theme = cx.theme();
    v_flex()
        .gap_1p5()
        .children(values.iter().map(|(label, v)| {
            h_flex()
                .gap_2()
                .text_xs()
                .child(div().w_24().truncate().text_color(theme.muted_foreground).child(label.clone()))
                .child(
                    div().flex_1().h_3().rounded_sm().bg(theme.secondary).child(
                        div()
                            .h_full()
                            .rounded_sm()
                            .w(relative(*v as f32 / max as f32))
                            .bg(color),
                    ),
                )
                .child(div().w_10().text_right().child(v.to_string()))
        }))
        .into_any_element()
}

impl StatsPage {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = Library::global(cx);
        let today = time::today();
        Self {
            month: time::month_start(today),
            year: today.year(),
            _subscriptions: vec![cx.observe(&library, |_, _, cx| cx.notify())],
        }
    }

    fn render_month(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let days = library.read(cx).day_stats();
        let stat = stats::month_stat(&days, self.month);
        let theme = cx.theme();
        let first = self.month;
        let offset = first.weekday().num_days_from_monday() as usize;
        let count = time::days_in_month(first) as usize;
        let max = (0..count)
            .filter_map(|d| days.get(&(first + chrono::Duration::days(d as i64))))
            .map(|d| d.count)
            .max()
            .unwrap_or(0);
        let today = time::today();
        let cells: Vec<Option<NaiveDate>> = (0..offset)
            .map(|_| None)
            .chain((0..count).map(|d| Some(first + chrono::Duration::days(d as i64))))
            .collect();
        panel(t!("月统计"), cx)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("month-prev")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::ChevronLeft))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.month = time::add_months(this.month, -1);
                                cx.notify();
                            })),
                    )
                    .child(div().text_sm().font_medium().child(time::fmt_month(self.month)))
                    .child(
                        Button::new("month-next")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::ChevronRight))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.month = time::add_months(this.month, 1);
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!(
                                "{} 条笔记 · {} 字 · {} 天 · 单日最多 {} 条 / {} 字",
                                stat.memos, stat.chars, stat.days, stat.max_day_memos, stat.max_day_chars
                            )),
                    ),
            )
            .child(
                h_flex().gap_1().children((0..7).map(|i| {
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .flex()
                        .justify_center()
                        .child(weekday_label(i))
                })),
            )
            .child(v_flex().gap_1().children(cells.chunks(7).map(|week| {
                h_flex().gap_1().children((0..7).map(|i| match week.get(i).copied().flatten() {
                    None => div().flex_1().h(rems(2.4)).into_any_element(),
                    Some(date) => {
                        let count = days.get(&date).map(|d| d.count).unwrap_or(0);
                        let level = stats::heat_level(count, max);
                        div()
                            .id(SharedString::from(format!("cal-{date}")))
                            .flex_1()
                            .h(rems(2.4))
                            .rounded_md()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .bg(if level == 0 { theme.secondary.opacity(0.6) } else { theme.green.opacity(0.18 + 0.16 * level as f32) })
                            .when(date == today, |d| d.border_1().border_color(theme.foreground.opacity(0.5)))
                            .child(date.day().to_string())
                            .when(count > 0, |d| {
                                d.child(div().text_color(theme.muted_foreground).child(format!("{count}")))
                                    .cursor_pointer()
                                    .on_click(move |_, window, cx| dispatch(Command::OpenDay(date), window, cx))
                            })
                            .into_any_element()
                    }
                }))
            })))
    }

    fn render_year(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let days = library.read(cx).day_stats();
        let theme = cx.theme();
        let year = self.year;
        let start = stats::week_start(NaiveDate::from_ymd_opt(year, 1, 1).unwrap());
        let end = NaiveDate::from_ymd_opt(year, 12, 31).unwrap();
        let weeks = (end - start).num_days() / 7 + 1;
        let max = days
            .iter()
            .filter(|(d, _)| d.year() == year)
            .map(|(_, s)| s.count)
            .max()
            .unwrap_or(0);
        let months = stats::year_months(&days, year);
        let total: usize = months.iter().map(|m| m.count).sum();
        let chars: usize = months.iter().map(|m| m.chars).sum();
        let active_days = days.keys().filter(|d| d.year() == year).count();
        let cell = px(10.);
        panel(t!("年统计"), cx)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("year-prev")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::ChevronLeft))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.year -= 1;
                                cx.notify();
                            })),
                    )
                    .child(div().text_sm().font_medium().child(t!("{} 年", year)))
                    .child(
                        Button::new("year-next")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::ChevronRight))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.year += 1;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("累计 {} 条笔记 · {} 字 · 记录 {} 天", total, chars, active_days)),
                    ),
            )
            .child(
                h_flex()
                    .id("year-heatmap")
                    .gap(px(2.))
                    .overflow_x_scrollbar()
                    .children((0..weeks).map(|w| {
                        let week = start + chrono::Duration::weeks(w);
                        let days = days.clone();
                        v_flex().gap(px(2.)).children((0..7).map(move |d| {
                            let date = week + chrono::Duration::days(d);
                            let in_year = date.year() == year;
                            let count = days.get(&date).map(|s| s.count).unwrap_or(0);
                            let level = stats::heat_level(count, max);
                            div()
                                .size(cell)
                                .rounded_sm()
                                .bg(if !in_year {
                                    theme.transparent
                                } else if level == 0 {
                                    theme.secondary
                                } else {
                                    theme.green.opacity(0.2 + 0.16 * level as f32)
                                })
                        }))
                    })),
            )
            .child(vertical_bars(
                "year-months",
                &months
                    .iter()
                    .enumerate()
                    .map(|(i, m)| (t!("{}月", i + 1), m.count))
                    .collect::<Vec<_>>(),
                theme.green.opacity(0.7),
                cx,
            ))
    }
}

impl Render for StatsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let overview = lib.overview().clone();
        let dist = stats::distributions(lib.store().active_memos());
        let mut top_tags: Vec<(String, usize)> = lib
            .tag_counts()
            .iter()
            .filter(|(t, _)| !t.contains('/'))
            .map(|(t, c)| (format!("#{t}"), *c))
            .collect();
        top_tags.sort_by_key(|t| std::cmp::Reverse(t.1));
        top_tags.truncate(15);
        let theme = cx.theme().clone();
        let big = |value: String, label: &str| {
            v_flex()
                .flex_1()
                .gap_1()
                .p_4()
                .rounded_lg()
                .bg(theme.background)
                .border_1()
                .border_color(theme.border)
                .child(div().text_2xl().font_semibold().child(value))
                .child(div().text_xs().text_color(theme.muted_foreground).child(label.to_string()))
        };
        let hours: Vec<(String, usize)> = dist
            .hours
            .iter()
            .enumerate()
            .map(|(h, c)| (if h % 3 == 0 { format!("{h}") } else { String::new() }, *c))
            .collect();
        let weekdays: Vec<(String, usize)> = dist
            .weekdays
            .iter()
            .enumerate()
            .map(|(i, c)| (t!("周{}", weekday_label(i)), *c))
            .collect();

        v_flex()
            .id("stats-scroll")
            .size_full()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(960.))
                    .mx_auto()
                    .px_6()
                    .py_4()
                    .gap_4()
                    .child(h_flex().gap_1().children(sidebar_toggle(cx)).child(div().text_lg().font_semibold().child(t!("记录统计"))))
                    .child(
                        h_flex()
                            .gap_3()
                            .child(big(overview.memos.to_string(), t!("累计笔记")))
                            .child(big(overview.chars.to_string(), t!("累计字数")))
                            .child(big(overview.tags.to_string(), t!("标签")))
                            .child(big(overview.days.to_string(), t!("记录天数")))
                            .child(big(overview.streak.to_string(), t!("连续记录"))),
                    )
                    .child(self.render_month(cx))
                    .child(self.render_year(cx))
                    .child(
                        h_flex()
                            .gap_4()
                            .items_start()
                            .child(div().flex_1().child(panel(t!("时间分布（24 小时）"), cx).child(vertical_bars("hours", &hours, theme.blue.opacity(0.7), cx))))
                            .child(div().flex_1().child(panel(t!("星期分布"), cx).child(vertical_bars("weekdays", &weekdays, theme.blue.opacity(0.7), cx)))),
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .items_start()
                            .child(div().flex_1().child(panel(t!("字数分布"), cx).child(horizontal_bars(&dist.lengths, theme.magenta.opacity(0.6), cx))))
                            .child(div().flex_1().child(panel(t!("媒体分布"), cx).child(horizontal_bars(&dist.media, theme.cyan.opacity(0.7), cx))))
                            .child(div().flex_1().child(panel(t!("记录来源"), cx).child(horizontal_bars(&dist.sources, theme.yellow.opacity(0.8), cx)))),
                    )
                    .child(
                        panel(t!("标签排行（一级标签）"), cx).child(if top_tags.is_empty() {
                            div().text_sm().text_color(theme.muted_foreground).child(t!("还没有标签")).into_any_element()
                        } else {
                            horizontal_bars(&top_tags, theme.green.opacity(0.7), cx)
                        }),
                    ),
            )
    }
}
