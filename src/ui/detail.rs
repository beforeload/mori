use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _,
};

use crate::core::model::{MemoId, MemoVersion};
use crate::core::{parse, time};

use super::composer::{Composer, ComposerEvent, ComposerMode};
use super::dialogs::confirm;
use super::library::Library;
use super::memo_card::{CardOptions, memo_card, mini_card};
use super::workspace::Workspace;
use super::{Command, Lucide, WorkspaceGlobal, dispatch};

pub struct MemoDetail {
    memo_id: MemoId,
    tab: usize,
    hide_same_tag: bool,
    editing: Option<Entity<Composer>>,
    annotating: Option<Entity<Composer>>,
    versions: Vec<MemoVersion>,
    _edit_subscription: Option<Subscription>,
    _annotate_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

fn section_title(title: &str, count: usize, cx: &gpui_kit::App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .pt_2()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(title.to_string())
        .child(count.to_string())
}

impl MemoDetail {
    pub fn new(id: &str, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = Library::global(cx);
        let subscriptions = vec![cx.observe(&library, |this, _, cx| {
            this.load_versions(cx);
            cx.notify();
        })];
        let mut detail = Self {
            memo_id: id.to_string(),
            tab: 0,
            hide_same_tag: false,
            editing: None,
            annotating: None,
            versions: Vec::new(),
            _edit_subscription: None,
            _annotate_subscription: None,
            _subscriptions: subscriptions,
        };
        detail.load_versions(cx);
        detail
    }

    pub fn memo_id(&self) -> &str {
        &self.memo_id
    }

    pub fn set_memo(&mut self, id: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.memo_id != id {
            self.memo_id = id.to_string();
            self.editing = None;
            self.annotating = None;
            self.tab = 0;
            self.load_versions(cx);
        }
        cx.notify();
    }

    pub fn show_history(&mut self, cx: &mut Context<Self>) {
        self.tab = 1;
        self.load_versions(cx);
        cx.notify();
    }

    fn load_versions(&mut self, cx: &mut Context<Self>) {
        self.versions = Library::global(cx)
            .read(cx)
            .store()
            .versions(&self.memo_id)
            .unwrap_or_default();
    }

    pub fn start_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let composer = cx.new(|cx| Composer::new(ComposerMode::Edit(self.memo_id.clone()), window, cx));
        composer.update(cx, |c, cx| c.focus(window, cx));
        self._edit_subscription = Some(cx.subscribe(&composer, |this, _, event: &ComposerEvent, cx| {
            if matches!(event, ComposerEvent::Submitted(_) | ComposerEvent::Cancelled) {
                this.editing = None;
                cx.notify();
            }
        }));
        self.editing = Some(composer);
        cx.notify();
    }

    /// Writes a new memo that references this one, without leaving the current page.
    pub fn start_annotate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let composer = cx.new(|cx| Composer::new(ComposerMode::New, window, cx));
        let text = format!("{} ", parse::ref_token(&self.memo_id));
        composer.update(cx, |c, cx| {
            c.set_text(&text, window, cx);
            c.focus(window, cx);
        });
        self._annotate_subscription = Some(cx.subscribe(&composer, |this, _, event: &ComposerEvent, cx| {
            if matches!(event, ComposerEvent::Submitted(_) | ComposerEvent::Cancelled) {
                this.annotating = None;
                cx.notify();
            }
        }));
        self.annotating = Some(composer);
        self.tab = 0;
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(ws) = cx.try_global::<WorkspaceGlobal>().and_then(|g| g.0.upgrade()) {
            cx.defer(move |cx| ws.update(cx, |ws: &mut Workspace, cx| ws.close_detail(cx)));
        }
    }

    fn render_links(&self, cx: &mut Context<Self>) -> AnyElement {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let store = lib.store();
        let Some(memo) = store.get(&self.memo_id) else {
            return div().into_any_element();
        };
        let refs: Vec<_> = memo.refs.iter().filter_map(|id| store.get_active(id)).collect();
        let backlinks = store.backlinks(&memo.id);
        let own_tags = memo.tags.clone();
        let hide = self.hide_same_tag;
        let related: Vec<(MemoId, f32)> = lib.index().related(&memo.id, 8, |id| {
            store.get_active(id).is_some_and(|m| {
                !hide || !m.tags.iter().any(|t| own_tags.iter().any(|o| o.split('/').next() == t.split('/').next()))
            })
        });
        let mut insight_ids: Vec<MemoId> = vec![memo.id.clone()];
        insight_ids.extend(related.iter().map(|(id, _)| id.clone()));
        let theme = cx.theme();
        v_flex()
            .gap_2()
            .when(!refs.is_empty(), |this| {
                this.child(section_title(t!("引用了"), refs.len(), cx))
                    .children(refs.iter().map(|m| mini_card("detail-ref", m, None, cx)))
            })
            .child(section_title(t!("被批注"), backlinks.len(), cx))
            .when(backlinks.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t!("还没有笔记批注它。点卡片菜单的「批注」写下新想法，就能建立连接。")),
                )
            })
            .children(backlinks.iter().map(|m| mini_card("detail-back", m, None, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .pt_2()
                    .child(section_title(t!("相关笔记"), related.len(), cx))
                    .child(div().flex_1())
                    .child(
                        Switch::new("hide-same-tag")
                            .xsmall()
                            .label(t!("隐藏同标签"))
                            .checked(self.hide_same_tag)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.hide_same_tag = *checked;
                                cx.notify();
                            })),
                    ),
            )
            .when(related.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(if lib.is_indexing() { t!("正在建立语义索引…") } else { t!("暂时没有找到相关笔记") }),
                )
            })
            .children(related.iter().filter_map(|(id, score)| {
                store
                    .get(id)
                    .map(|m| mini_card("detail-related", m, Some(t!("相似度 {}%", format!("{:.0}", score * 100.0))), cx))
            }))
            .when(!related.is_empty(), |this| {
                this.child(
                    Button::new("insight-related")
                        .outline()
                        .small()
                        .icon(Icon::new(Lucide::Sparkles))
                        .label(t!("AI 洞察这组笔记"))
                        .on_click(move |_, window, cx| {
                            dispatch(Command::InsightMemos(insight_ids.clone(), t!("相关笔记").into()), window, cx)
                        }),
                )
            })
            .into_any_element()
    }

    fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let memo_id = self.memo_id.clone();
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("每次编辑都会自动保存旧版本，保留最近 30 天，仅包含文字内容。")),
            )
            .when(self.versions.is_empty(), |this| {
                this.child(div().text_sm().text_color(theme.muted_foreground).py_4().child(t!("暂无历史版本")))
            })
            .children(self.versions.iter().map(|version| {
                let vid = version.id;
                let memo_id = memo_id.clone();
                v_flex()
                    .gap_1()
                    .p_3()
                    .rounded_md()
                    .bg(theme.secondary.opacity(0.5))
                    .child(
                        h_flex()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(time::relative_day(version.saved_at))
                            .child(div().w_2())
                            .child(t!("{} 字", parse::plain_char_count(&version.content)))
                            .child(div().flex_1())
                            .child(
                                Button::new(SharedString::from(format!("restore-{vid}")))
                                    .xsmall()
                                    .outline()
                                    .label(t!("恢复"))
                                    .on_click(move |_, window, cx| {
                                        let memo_id = memo_id.clone();
                                        confirm(window, cx, t!("恢复到这个版本？"), t!("当前内容会保存为一个新的历史版本。"), t!("恢复"), false, move |window, cx| {
                                            let id = memo_id.clone();
                                            let result = Library::global(cx)
                                                .update(cx, |lib, cx| lib.mutate(cx, |s| s.restore_version(&id, vid)));
                                            if let Err(err) = result {
                                                use gpui_kit::component::WindowExt as _;
                                                window.push_notification(t!("恢复失败：{}", err), cx);
                                            }
                                        });
                                    }),
                            ),
                    )
                    .child(div().text_sm().whitespace_normal().child(version.content.clone()))
            }))
            .into_any_element()
    }
}

impl Render for MemoDetail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let memo = lib.store().get(&self.memo_id).cloned();
        let theme = cx.theme();
        let header = h_flex()
            .px_4()
            .py_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(Lucide::PanelRight).small().text_color(theme.muted_foreground))
            .child(div().font_semibold().child(t!("笔记详情")))
            .child(div().flex_1())
            .child(
                Button::new("detail-close")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::X))
                    .tooltip(t!("关闭 Esc"))
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            );
        let Some(memo) = memo else {
            return v_flex()
                .size_full()
                .child(header)
                .child(div().p_4().text_color(theme.muted_foreground).child(t!("这条笔记已被彻底删除")))
                .into_any_element();
        };
        let card = match &self.editing {
            Some(composer) => composer.clone().into_any_element(),
            None => memo_card(
                &memo,
                CardOptions {
                    expanded: true,
                    mode: if memo.is_deleted() {
                        super::memo_card::CardMode::Trash
                    } else {
                        super::memo_card::CardMode::Normal
                    },
                    ..CardOptions::new("detail")
                },
                lib,
                cx,
            ),
        };
        let muted = cx.theme().muted_foreground;
        let body = if self.tab == 0 { self.render_links(cx) } else { self.render_history(cx) };
        v_flex()
            .size_full()
            .child(header)
            .child(
                v_flex()
                    .id("detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .gap_3()
                    .overflow_y_scrollbar()
                    .child(card)
                    .when_some(self.annotating.clone(), |this, composer| {
                        this.child(
                            v_flex()
                                .gap_1()
                                .child(
                                    h_flex()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(t!("写批注"))
                                        .child(div().flex_1())
                                        .child(
                                            Button::new("cancel-annotate")
                                                .ghost()
                                                .xsmall()
                                                .label(t!("取消"))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.annotating = None;
                                                    cx.notify();
                                                })),
                                        ),
                                )
                                .child(composer),
                        )
                    })
                    .child(
                        TabBar::new("detail-tabs")
                            .segmented()
                            .small()
                            .selected_index(self.tab)
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                this.tab = *ix;
                                this.load_versions(cx);
                                cx.notify();
                            }))
                            .child(t!("关联"))
                            .child(t!("历史版本 {}", self.versions.len())),
                    )
                    .child(body),
            )
            .into_any_element()
    }
}
