//! "API 输入": memos that arrived from outside the app, and how to send them.

use gpui_kit::component::button::Button;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use super::sidebar::sidebar_toggle;
use super::library::Library;
use super::memo_card::{CardOptions, memo_card};
use super::settings_page::SettingsSection;
use super::theme::surfaces;
use super::{Command, Lucide, dispatch};

pub struct InboxPage {
    _subscriptions: Vec<Subscription>,
}

impl InboxPage {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = Library::global(cx);
        Self {
            _subscriptions: vec![cx.observe(&library, |_, _, cx| cx.notify())],
        }
    }
}

impl Render for InboxPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let settings = lib.settings().clone();
        let running = lib.api_running();
        let memos: Vec<_> = lib
            .store()
            .active_memos()
            .filter(|m| matches!(m.source.as_str(), "api" | "mcp"))
            .collect();
        let theme = cx.theme().clone();
        let s = surfaces(cx);
        let method = |icon: Lucide, title: &str, desc: String| {
            h_flex()
                .gap_3()
                .items_start()
                .child(div().mt_0p5().child(Icon::new(icon).small().text_color(s.accent)))
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(div().text_sm().font_medium().child(title.to_string()))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(desc)),
                )
        };
        v_flex()
            .id("inbox-scroll")
            .size_full()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(800.))
                    .mx_auto()
                    .px_6()
                    .pb_6()
                    .gap_3()
                    .child(
                        h_flex()
                            .h_9()
                            .gap_2()
                            .child(h_flex().gap_1().children(sidebar_toggle(cx)).child(div().text_base().font_semibold().child(t!("API 输入"))))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(t!("{} 条", memos.len()))),
                    )
                    .child(
                        v_flex()
                            .gap_3()
                            .p_4()
                            .rounded_lg()
                            .bg(s.card)
                            .child(div().text_sm().child(t!("在任何地方记录，自动汇入 Mori")))
                            .child(method(
                                Lucide::Webhook,
                                t!("记录 API"),
                                if running {
                                    t!("POST http://127.0.0.1:{}/api/v1/memos ，可配合快捷指令、Raycast、Alfred 或脚本使用", settings.api_port)
                                } else {
                                    t!("未启用。开启后可通过 HTTP 请求写入笔记，配合快捷指令、Raycast、Alfred 或脚本使用").into()
                                },
                            ))
                            .child(method(
                                Lucide::Smartphone,
                                t!("快捷指令"),
                                t!("在「快捷指令」中添加「获取 URL 内容」，方法选 POST，请求体 JSON 填 content，即可从菜单栏或 Siri 记录").into(),
                            ))
                            .child(method(
                                Lucide::Bot,
                                "MCP",
                                t!("在 Claude、Cursor 等 AI 工具里说「把这段结论记到 Mori」，笔记会出现在这里").into(),
                            ))
                            .child(
                                h_flex().child(
                                    Button::new("inbox-settings")
                                        .outline()
                                        .small()
                                        .label(if running { t!("查看接口配置") } else { t!("去开启") })
                                        .on_click(|_, window, cx| {
                                            dispatch(Command::OpenSettings(SettingsSection::Open), window, cx)
                                        }),
                                ),
                            ),
                    )
                    .when(memos.is_empty(), |this| {
                        this.child(
                            div()
                                .py_10()
                                .flex()
                                .justify_center()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(t!("还没有通过 API 或 MCP 写入的笔记")),
                        )
                    })
                    .children(memos.iter().map(|memo| memo_card(memo, CardOptions::new("inbox"), lib, cx))),
            )
    }
}
