//! "问问笔记": a chat that answers from the user's own memos.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};

use crate::core::ai::{self, ChatMessage};
use crate::core::model::{ChatRecord, MemoId};
use crate::core::store::NewMemo;
use crate::core::time;

use super::insight_page::cited_indices;
use super::library::Library;
use super::memo_card::mini_card;
use super::tasks::{StreamEvent, run_blocking, stream_chat};
use super::{Command, Lucide, dispatch};

fn starters() -> [&'static str; 4] {
    [
        t!("最近一个月我主要在思考什么？"),
        t!("根据我的笔记，我是一个怎样的人？"),
        t!("我在工作上有哪些反复出现的困扰？"),
        t!("帮我把关于读书的笔记整理成几个主题"),
    ]
}

pub struct ChatPage {
    messages: Vec<ChatRecord>,
    streaming: Option<(String, Vec<MemoId>)>,
    input: Entity<TextareaState>,
    use_memory: bool,
    error: Option<String>,
    scroll: ScrollHandle,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ChatPage {
    pub fn reveal_latest(&self) {
        self.scroll.scroll_to_bottom();
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = Library::global(cx);
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder(t!("问问你的笔记…  ↩ 发送，⇧↩ 换行"))
        });
        let subscriptions = vec![cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = event {
                this.send(window, cx);
            }
        })];
        let messages = library.read(cx).store().chat_history().unwrap_or_default();
        Self {
            messages,
            streaming: None,
            input,
            use_memory: true,
            error: None,
            scroll: ScrollHandle::new(),
            _task: None,
            _subscriptions: subscriptions,
        }
    }

    fn ask(&mut self, question: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.set_value(question.to_string(), window, cx));
        self.send(window, cx);
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.streaming.is_some() {
            return;
        }
        let question = self.input.read(cx).value().trim().to_string();
        if question.is_empty() {
            return;
        }
        let library = Library::global(cx);
        let cfg = library.read(cx).settings().ai.clone();
        if !cfg.is_configured() {
            window.push_notification(t!("请先在设置中配置 AI 服务"), cx);
            return;
        }
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        self.error = None;
        let user_id = library.update(cx, |lib, _| lib.store_mut().add_chat("user", &question, &[]).unwrap_or(0));
        self.messages.push(ChatRecord {
            id: user_id,
            role: "user".into(),
            content: question.clone(),
            refs: Vec::new(),
            created_at: time::now(),
        });
        self.streaming = Some((String::new(), Vec::new()));
        self.scroll.scroll_to_bottom();
        cx.notify();

        let lib = library.read(cx);
        let memory = (self.use_memory && !lib.store().memory_profile().is_empty())
            .then(|| lib.store().memory_profile().to_markdown());
        let history: Vec<ChatMessage> = self
            .messages
            .iter()
            .rev()
            .skip(1)
            .take(6)
            .rev()
            .map(|m| {
                let content: String = m.content.chars().take(1500).collect();
                if m.role == "user" { ChatMessage::user(content) } else { ChatMessage::assistant(content) }
            })
            .collect();
        let uses_dense = cfg.uses_remote_embeddings();
        self._task = Some(cx.spawn(async move |this, cx| {
            let query_vector = if uses_dense {
                let cfg = cfg.clone();
                let q = question.clone();
                run_blocking(move || ai::embed(&cfg, &[q]))
                    .await
                    .and_then(|r| r.ok())
                    .and_then(|mut v| v.pop())
            } else {
                None
            };
            let _ = this.update(cx, |this, cx| {
                let library = Library::global(cx);
                let lib = library.read(cx);
                let store = lib.store();
                let hits = lib.index().search(&question, query_vector.as_deref(), 10, |id| store.get_active(id).is_some());
                let memos: Vec<_> = hits.iter().filter_map(|(id, _)| store.get(id)).collect();
                let (notes, ids) = ai::format_notes(&memos, 9_000);
                let mut messages = vec![ChatMessage::system(ai::chat_system(memory.as_deref()))];
                messages.extend(history);
                messages.push(ChatMessage::user(ai::chat_user_turn(&question, &notes)));
                this.streaming = Some((String::new(), ids.clone()));
                this._task = Some(stream_chat(cx, cfg, messages, move |this, event, cx| {
                    match event {
                        StreamEvent::Delta(delta) => {
                            if let Some((text, _)) = this.streaming.as_mut() {
                                text.push_str(&delta);
                            }
                        }
                        StreamEvent::Done(full) => {
                            let library = Library::global(cx);
                            let id = library.update(cx, |lib, _| {
                                lib.store_mut().add_chat("assistant", &full, &ids).unwrap_or(0)
                            });
                            this.messages.push(ChatRecord {
                                id,
                                role: "assistant".into(),
                                content: full,
                                refs: ids.clone(),
                                created_at: time::now(),
                            });
                            this.streaming = None;
                        }
                        StreamEvent::Failed(err) => {
                            this.error = Some(err);
                            this.streaming = None;
                        }
                    }
                    this.scroll.scroll_to_bottom();
                    cx.notify();
                }));
                cx.notify();
            });
        }));
    }

    fn render_message(&self, message: &ChatRecord, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        if message.role == "user" {
            return h_flex()
                .justify_end()
                .child(
                    div()
                        .max_w(px(560.))
                        .px_4()
                        .py_2()
                        .rounded_xl()
                        .bg(theme.primary)
                        .text_color(theme.primary_foreground)
                        .text_sm()
                        .child(message.content.clone()),
                )
                .into_any_element();
        }
        let library = Library::global(cx);
        let lib = library.read(cx);
        let cited: Vec<_> = cited_indices(&message.content)
            .into_iter()
            .filter_map(|n| message.refs.get(n - 1).and_then(|id| lib.store().get(id)).map(|m| (n, m)))
            .collect();
        let save_text = format!("{}\n\n#AI对话", message.content.trim());
        v_flex()
            .gap_2()
            .max_w(px(680.))
            .p_4()
            .rounded_xl()
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .child(TextView::markdown(SharedString::from(format!("chat-{}", message.id)), message.content.clone()))
            .when(!cited.is_empty(), |this| {
                this.child(div().text_xs().text_color(theme.muted_foreground).child(t!("参考的笔记")))
                    .children(cited.iter().map(|(n, m)| mini_card("chat-cite", m, Some(format!("[{n}]")), cx)))
            })
            .child(
                h_flex().justify_end().child(
                    Button::new(SharedString::from(format!("chat-save-{}", message.id)))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(Lucide::NotebookPen))
                        .label(t!("记成笔记"))
                        .on_click(move |_, window, cx| {
                            let text = save_text.clone();
                            let result = Library::global(cx).update(cx, |lib, cx| {
                                lib.mutate(cx, |s| {
                                    s.create_memo(NewMemo { content: text, created_at: None, images: vec![], source: "ai".into() })
                                })
                            });
                            if result.is_ok() {
                                window.push_notification(t!("已把这段讨论写回笔记"), cx);
                            }
                        }),
                ),
            )
            .into_any_element()
    }
}

impl Render for ChatPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let configured = lib.settings().ai.is_configured();
        let has_memory = !lib.store().memory_profile().is_empty();
        let theme = cx.theme().clone();
        let streaming = self.streaming.clone();
        let empty = self.messages.is_empty() && streaming.is_none();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .w_full()
                    .max_w(px(820.))
                    .mx_auto()
                    .px_6()
                    .pt_4()
                    .pb_2()
                    .gap_2()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(t!("读过你所有笔记的 AI 伙伴")))
                    .child(div().flex_1())
                    .child(
                        Switch::new("use-memory")
                            .small()
                            .label(if has_memory { t!("结合记忆档案") } else { t!("结合记忆档案（未生成）") })
                            .checked(self.use_memory && has_memory)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.use_memory = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("chat-clear")
                            .ghost()
                            .small()
                            .icon(Icon::new(Lucide::Eraser))
                            .label(t!("清空对话"))
                            .disabled(self.messages.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let library = Library::global(cx);
                                library.update(cx, |lib, _| {
                                    let _ = lib.store_mut().clear_chat();
                                });
                                this.messages.clear();
                                this._task = None;
                                this.streaming = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("chat-scroll")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .vertical_scrollbar(&self.scroll)
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(820.))
                            .mx_auto()
                            .px_6()
                            .py_4()
                            .gap_4()
                            .when(!configured, |this| {
                                this.child(
                                    h_flex()
                                        .gap_2()
                                        .p_3()
                                        .rounded_md()
                                        .bg(theme.warning.opacity(0.12))
                                        .child(Icon::new(Lucide::CircleAlert).small().text_color(theme.warning))
                                        .child(div().flex_1().text_sm().child(t!("还没有配置 AI 服务")))
                                        .child(
                                            Button::new("chat-settings")
                                                .small()
                                                .label(t!("去设置"))
                                                .on_click(|_, window, cx| dispatch(Command::OpenSettings(super::settings_page::SettingsSection::Ai), window, cx)),
                                        ),
                                )
                            })
                            .when(empty, |this| {
                                this.child(
                                    v_flex()
                                        .gap_3()
                                        .py_8()
                                        .items_center()
                                        .child(Icon::new(Lucide::MessageCircle).large().text_color(theme.muted_foreground))
                                        .child(div().text_sm().text_color(theme.muted_foreground).child(t!("回顾、思考、追问。回答会标注参考了哪几条笔记。")))
                                        .child(h_flex().flex_wrap().justify_center().gap_2().children(starters().into_iter().enumerate().map(|(i, q)| {
                                            Button::new(("starter", i))
                                                .outline()
                                                .small()
                                                .label(q)
                                                .on_click(cx.listener(move |this, _, window, cx| this.ask(q, window, cx)))
                                        }))),
                                )
                            })
                            .children(self.messages.iter().map(|m| self.render_message(m, cx)).collect::<Vec<_>>())
                            .when_some(streaming, |this, (text, _)| {
                                this.child(
                                    v_flex()
                                        .max_w(px(680.))
                                        .p_4()
                                        .rounded_xl()
                                        .bg(theme.background)
                                        .border_1()
                                        .border_color(theme.border)
                                        .child(if text.is_empty() {
                                            div().text_sm().text_color(theme.muted_foreground).child(t!("正在翻阅你的笔记…")).into_any_element()
                                        } else {
                                            TextView::markdown("chat-streaming", text).into_any_element()
                                        }),
                                )
                            })
                            .when_some(self.error.clone(), |this, err| {
                                this.child(div().text_sm().text_color(theme.danger).child(err))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .max_w(px(820.))
                    .mx_auto()
                    .px_6()
                    .pb_4()
                    .pt_2()
                    .gap_2()
                    .items_end()
                    .child(
                        div()
                            .flex_1()
                            .p_2()
                            .rounded_lg()
                            .bg(theme.background)
                            .border_1()
                            .border_color(theme.border)
                            .child(Textarea::new(&self.input).appearance(false)),
                    )
                    .child(
                        Button::new("chat-send")
                            .primary()
                            .icon(Icon::new(Lucide::SendHorizontal))
                            .loading(self.streaming.is_some())
                            .disabled(!configured)
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    ),
            )
    }
}
