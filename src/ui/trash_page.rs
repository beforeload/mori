use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use super::sidebar::sidebar_toggle;
use super::Lucide;
use super::dialogs::confirm;
use super::library::Library;
use super::memo_card::{CardMode, CardOptions, memo_card};

pub struct TrashPage {
    _subscriptions: Vec<Subscription>,
}

impl TrashPage {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = Library::global(cx);
        Self {
            _subscriptions: vec![cx.observe(&library, |_, _, cx| cx.notify())],
        }
    }
}

impl Render for TrashPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let trashed = lib.store().trashed_memos();
        let count = trashed.len();
        let theme = cx.theme();
        v_flex()
            .id("trash-scroll")
            .size_full()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(760.))
                    .mx_auto()
                    .px_6()
                    .py_4()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(h_flex().gap_1().children(sidebar_toggle(cx)).child(div().text_lg().font_semibold().child(t!("回收站"))))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(t!("{} 条", count)))
                            .child(div().flex_1())
                            .child(
                                Button::new("empty-trash")
                                    .danger()
                                    .small()
                                    .icon(Icon::new(Lucide::Trash))
                                    .label(t!("清空回收站"))
                                    .disabled(count == 0)
                                    .on_click(move |_, window, cx| {
                                        confirm(
                                            window,
                                            cx,
                                            &t!("彻底删除回收站中的 {} 条笔记？", count),
                                            t!("此操作无法撤销，相关图片与历史版本也会被删除。"),
                                            t!("清空"),
                                            true,
                                            |window, cx| {
                                                let result = Library::global(cx)
                                                    .update(cx, |lib, cx| lib.mutate(cx, |s| s.empty_trash()));
                                                if let Ok(n) = result {
                                                    window.push_notification(
                                                        Notification::success(t!("已彻底删除 {} 条笔记", n)),
                                                        cx,
                                                    );
                                                }
                                            },
                                        );
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("删除的笔记会保留在这里，可以随时恢复。使用卡片右上角菜单恢复或彻底删除。")),
                    )
                    .when(count == 0, |this| {
                        this.child(
                            div()
                                .py_16()
                                .flex()
                                .justify_center()
                                .text_color(theme.muted_foreground)
                                .child(t!("回收站是空的")),
                        )
                    })
                    .children(trashed.iter().map(|memo| {
                        memo_card(
                            memo,
                            CardOptions {
                                mode: CardMode::Trash,
                                ..CardOptions::new("trash")
                            },
                            lib,
                            cx,
                        )
                    })),
            )
    }
}
