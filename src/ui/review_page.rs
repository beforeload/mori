use chrono::Datelike;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use crate::core::model::{MemoId, ReviewRange, ReviewScope};
use crate::core::{review, time};

use super::sidebar::sidebar_toggle;
use super::Lucide;
use super::library::Library;
use super::memo_card::{CardOptions, memo_card, mini_card};

const COUNTS: [usize; 6] = [4, 8, 12, 16, 20, 24];
fn scopes() -> [(ReviewScope, &'static str); 4] { [
    (ReviewScope::All, t!("全部内容")),
    (ReviewScope::IncludeTags, t!("包含标签")),
    (ReviewScope::ExcludeTags, t!("排除标签")),
    (ReviewScope::Untagged, t!("无标签")),
]}

pub struct ReviewPage {
    batch: u32,
    items: Vec<MemoId>,
    index: usize,
    focus_mode: bool,
    show_settings: bool,
    tags_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl ReviewPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = Library::global(cx);
        let tags = library.read(cx).settings().review.tags.join(" ");
        let tags_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("标签，空格分隔，例如 读书 日记"))
                .default_value(tags)
        });
        let subscriptions = vec![
            cx.observe(&library, |this, _, cx| this.refresh(cx)),
            cx.subscribe_in(&tags_input, window, |this, state, event, _, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    let tags: Vec<String> = state
                        .read(cx)
                        .value()
                        .split_whitespace()
                        .map(|t| t.trim_start_matches('#').to_string())
                        .filter(|t| !t.is_empty())
                        .collect();
                    this.update_config(cx, move |c| c.tags = tags);
                }
            }),
        ];
        let mut page = Self {
            batch: 0,
            items: Vec::new(),
            index: 0,
            focus_mode: true,
            show_settings: false,
            tags_input,
            _subscriptions: subscriptions,
        };
        page.refresh(cx);
        page
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let cfg = lib.settings().review.clone();
        let items = review::daily_review(lib.store(), &cfg, time::today(), self.batch);
        if items != self.items {
            self.items = items;
            self.index = self.index.min(self.items.len().saturating_sub(1));
        }
        cx.notify();
    }

    fn update_config(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut crate::core::model::ReviewConfig)) {
        let library = Library::global(cx);
        let _ = library.update(cx, |lib, cx| lib.update_settings(cx, |s| f(&mut s.review)));
        self.index = 0;
        self.refresh(cx);
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let cfg = library.read(cx).settings().review.clone();
        let theme = cx.theme();
        let pool = review::review_pool(library.read(cx).store(), &cfg).len();
        let label = |text: &str| div().w_20().text_sm().text_color(theme.muted_foreground).child(text.to_string());
        v_flex()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_3()
                    .child(label(t!("内容范围")))
                    .child(
                        TabBar::new("review-scope")
                            .segmented()
                            .small()
                            .selected_index(scopes().iter().position(|(s, _)| *s == cfg.scope).unwrap_or(0))
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                let scope = scopes()[*ix].0;
                                this.update_config(cx, move |c| c.scope = scope);
                            }))
                            .children(scopes().iter().map(|(_, l)| *l)),
                    ),
            )
            .when(matches!(cfg.scope, ReviewScope::IncludeTags | ReviewScope::ExcludeTags), |this| {
                this.child(
                    h_flex()
                        .gap_3()
                        .child(label(t!("标签")))
                        .child(div().flex_1().child(Input::new(&self.tags_input).small())),
                )
            })
            .child(
                h_flex()
                    .gap_3()
                    .child(label(t!("时间范围")))
                    .child(
                        TabBar::new("review-range")
                            .segmented()
                            .small()
                            .selected_index(ReviewRange::ALL.iter().position(|r| *r == cfg.range).unwrap_or(0))
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                let range = ReviewRange::ALL[*ix];
                                this.update_config(cx, move |c| c.range = range);
                            }))
                            .children(ReviewRange::ALL.iter().map(|r| r.label())),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(label(t!("每天数量")))
                    .child(
                        TabBar::new("review-count")
                            .segmented()
                            .small()
                            .selected_index(COUNTS.iter().position(|c| *c == cfg.count).unwrap_or(1))
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                let count = COUNTS[*ix];
                                this.update_config(cx, move |c| c.count = count);
                            }))
                            .children(COUNTS.iter().map(|c| t!("{} 条", c))),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("符合条件的过往笔记共 {} 条。每天的回顾内容固定，点「换一批」可以重新抽取。", pool)),
            )
    }

    fn render_on_this_day(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let today = time::today();
        let memos: Vec<_> = lib
            .store()
            .active_memos()
            .filter(|m| {
                let d = time::date_of(m.created_at);
                d.year() < today.year() && d.month() == today.month() && d.day() == today.day()
            })
            .collect();
        if memos.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .gap_2()
                .child(div().text_sm().font_semibold().child(t!("往年今日 · {} 条", memos.len())))
                .children(memos.iter().map(|m| {
                    mini_card(
                        "onthisday",
                        m,
                        Some(t!("{} 年前", today.year() - time::date_of(m.created_at).year())),
                        cx,
                    )
                })),
        )
    }
}

impl Render for ReviewPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let theme = cx.theme();
        let total = self.items.len();
        let index = self.index.min(total.saturating_sub(1));
        let body = if total == 0 {
            div()
                .py_16()
                .flex()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child(t!("还没有可以回顾的过往笔记。坚持记录几天，这里就会出现与旧想法重逢的惊喜。"))
                .into_any_element()
        } else if self.focus_mode {
            let lib = library.read(cx);
            let card = lib.store().get(&self.items[index]).map(|memo| {
                memo_card(
                    memo,
                    CardOptions {
                        expanded: true,
                        ..CardOptions::new("review-focus")
                    },
                    lib,
                    cx,
                )
            });
            v_flex()
                .gap_4()
                .children(card)
                .child(
                    h_flex()
                        .gap_3()
                        .justify_center()
                        .child(
                            Button::new("review-prev")
                                .outline()
                                .icon(Icon::new(Lucide::ChevronLeft))
                                .label(t!("上一条"))
                                .disabled(index == 0)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.index = this.index.saturating_sub(1);
                                    cx.notify();
                                })),
                        )
                        .child(h_flex().gap_1().children((0..total).map(|i| {
                            div()
                                .size_2()
                                .rounded_full()
                                .bg(if i == index { theme.primary } else { theme.border })
                        })))
                        .child(
                            Button::new("review-next")
                                .primary()
                                .icon(Icon::new(Lucide::ChevronRight))
                                .label(if index + 1 >= total { t!("今日回顾完成") } else { t!("下一条") })
                                .disabled(index + 1 >= total)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.index = (this.index + 1).min(total - 1);
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element()
        } else {
            let lib = library.read(cx);
            v_flex()
                .gap_3()
                .children(self.items.iter().filter_map(|id| lib.store().get(id)).map(|memo| {
                    memo_card(
                        memo,
                        CardOptions {
                            expanded: true,
                            ..CardOptions::new("review-list")
                        },
                        lib,
                        cx,
                    )
                }))
                .into_any_element()
        };

        v_flex()
            .id("review-scroll")
            .size_full()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(760.))
                    .mx_auto()
                    .px_6()
                    .py_4()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(h_flex().gap_1().children(sidebar_toggle(cx)).child(div().text_lg().font_semibold().child(t!("每日回顾"))))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(if total == 0 {
                                        time::fmt_day(time::today())
                                    } else if self.focus_mode {
                                        t!("{} · 第 {} / {} 条", time::fmt_day(time::today()), index + 1, total)
                                    } else {
                                        t!("{} · 共 {} 条", time::fmt_day(time::today()), total)
                                    }),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("review-mode")
                                    .ghost()
                                    .small()
                                    .icon(Icon::new(if self.focus_mode { Lucide::List } else { Lucide::Square }))
                                    .label(if self.focus_mode { t!("列表") } else { t!("逐条") })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.focus_mode = !this.focus_mode;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("review-shuffle")
                                    .ghost()
                                    .small()
                                    .icon(Icon::new(Lucide::Shuffle))
                                    .label(t!("换一批"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.batch += 1;
                                        this.index = 0;
                                        this.refresh(cx);
                                    })),
                            )
                            .child(
                                Button::new("review-settings")
                                    .ghost()
                                    .small()
                                    .icon(Icon::new(Lucide::Settings))
                                    .selected(self.show_settings)
                                    .label(t!("回顾设置"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_settings = !this.show_settings;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(self.show_settings, |this| this.child(self.render_settings(cx)))
                    .child(body)
                    .children(self.render_on_this_day(cx)),
            )
    }
}
