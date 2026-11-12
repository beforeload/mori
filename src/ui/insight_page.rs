//! AI insight: read a chosen set of memos from a chosen perspective.

use std::sync::LazyLock;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};
use regex::Regex;

use crate::core::ai::{self, PERSPECTIVES};
use crate::core::model::{CustomPerspective, InsightRecord, Memo, MemoId};
use crate::core::time;

use super::sidebar::sidebar_toggle;
use super::library::Library;
use super::memo_card::mini_card;
use super::tasks::{StreamEvent, stream_chat};
use super::{Command, Lucide, dispatch};

fn ranges() -> [(&'static str, Option<i64>); 5] { [
    (t!("全部时间"), None),
    (t!("近 7 天"), Some(7)),
    (t!("近 30 天"), Some(30)),
    (t!("近 90 天"), Some(90)),
    (t!("近一年"), Some(365)),
]}

static CITE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d{1,3}(?:\s*[,，、]\s*\d{1,3})*)\]").unwrap());

pub fn cited_indices(text: &str) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for caps in CITE_RE.captures_iter(text) {
        for part in caps[1].split([',', '，', '、']) {
            if let Ok(n) = part.trim().parse::<usize>()
                && n > 0
                && !out.contains(&n)
            {
                out.push(n);
            }
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
enum Scope {
    All,
    Tag(String),
    Memos(Vec<MemoId>, String),
}

#[derive(Clone, Debug, PartialEq)]
enum Status {
    Idle,
    Running,
    Done,
    Failed(String),
}

pub struct InsightPage {
    scope: Scope,
    range: usize,
    perspective: usize,
    tag_input: Entity<InputState>,
    custom_name: Entity<InputState>,
    custom_prompt: Entity<TextareaState>,
    show_custom_form: bool,
    output: String,
    context_ids: Vec<MemoId>,
    output_title: String,
    status: Status,
    history: Vec<InsightRecord>,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl InsightPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = Library::global(cx);
        let tag_input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("标签，例如 读书")));
        let custom_name = cx.new(|cx| InputState::new(window, cx).placeholder(t!("视角名称，例如 老朋友")));
        let custom_prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder(t!("写下你珍藏的提示词，例如：像一位认识我十年的老朋友那样，坦诚地指出我最近的变化。"))
        });
        let history = library.read(cx).store().insights().unwrap_or_default();
        Self {
            scope: Scope::All,
            range: 2,
            perspective: 0,
            tag_input,
            custom_name,
            custom_prompt,
            show_custom_form: false,
            output: String::new(),
            context_ids: Vec::new(),
            output_title: String::new(),
            status: Status::Idle,
            history,
            _task: None,
            _subscriptions: vec![cx.observe(&library, |_, _, cx| cx.notify())],
        }
    }

    pub fn set_memos(&mut self, ids: Vec<MemoId>, label: String, _: &mut Window, cx: &mut Context<Self>) {
        self.scope = Scope::Memos(ids, label);
        self.status = Status::Idle;
        self.output.clear();
        cx.notify();
    }

    pub fn set_tag(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.scope = Scope::Tag(tag.to_string());
        self.range = 0;
        self.tag_input.update(cx, |s, cx| s.set_value(tag.to_string(), window, cx));
        self.status = Status::Idle;
        self.output.clear();
        cx.notify();
    }

    fn selected_memos<'a>(&self, lib: &'a Library, cx: &App) -> Vec<&'a Memo> {
        let store = lib.store();
        let cutoff = ranges()[self.range]
            .1
            .map(|days| time::start_of_day(time::today() - chrono::Duration::days(days)));
        let in_range = |m: &&Memo| cutoff.is_none_or(|c| m.created_at >= c);
        match &self.scope {
            Scope::All => store.active_memos().filter(in_range).collect(),
            Scope::Tag(_) => {
                let tag = self.tag_input.read(cx).value().trim().trim_start_matches('#').to_string();
                if tag.is_empty() {
                    return Vec::new();
                }
                store
                    .active_memos()
                    .filter(|m| m.has_tag_or_child(&tag))
                    .filter(in_range)
                    .collect()
            }
            Scope::Memos(ids, _) => ids.iter().filter_map(|id| store.get_active(id)).collect(),
        }
    }

    fn perspectives(&self, cx: &App) -> Vec<(String, String, String, Option<usize>)> {
        let mut list: Vec<(String, String, String, Option<usize>)> = PERSPECTIVES
            .iter()
            .map(|p| {
                (
                    crate::i18n::tr(p.name).to_string(),
                    crate::i18n::tr(p.description).to_string(),
                    p.instruction.to_string(),
                    None,
                )
            })
            .collect();
        let customs = Library::global(cx).read(cx).settings().custom_perspectives.clone();
        for (i, c) in customs.into_iter().enumerate() {
            list.push((c.name, t!("自定义视角").into(), c.prompt, Some(i)));
        }
        list
    }

    fn scope_label(&self, cx: &App) -> String {
        let range = ranges()[self.range].0;
        match &self.scope {
            Scope::All => t!("全部笔记 · {}", range),
            Scope::Tag(_) => format!("#{} · {range}", self.tag_input.read(cx).value().trim().trim_start_matches('#')),
            Scope::Memos(_, label) => label.clone(),
        }
    }

    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let cfg = lib.settings().ai.clone();
        if !cfg.is_configured() {
            window.push_notification(t!("请先在设置中配置 AI 服务"), cx);
            return;
        }
        let memos = self.selected_memos(lib, cx);
        if memos.is_empty() {
            self.status = Status::Failed(t!("所选范围内没有笔记").into());
            cx.notify();
            return;
        }
        let sampled = ai::sample_memos(&memos, 60);
        let (notes, ids) = ai::format_notes(&sampled, 14_000);
        let perspectives = self.perspectives(cx);
        let (name, _, instruction, _) = perspectives[self.perspective.min(perspectives.len() - 1)].clone();
        let messages = ai::insight_messages(&instruction, &name, &notes, ids.len());
        let scope = self.scope_label(cx);
        self.context_ids = ids.clone();
        self.output.clear();
        self.output_title = t!("{} · {} · {} 条笔记", name, scope, ids.len());
        self.status = Status::Running;
        cx.notify();
        self._task = Some(stream_chat(cx, cfg, messages, move |this, event, cx| {
            match event {
                StreamEvent::Delta(delta) => this.output.push_str(&delta),
                StreamEvent::Done(full) => {
                    this.output = full;
                    this.status = Status::Done;
                    let record = InsightRecord {
                        id: 0,
                        created_at: time::now(),
                        perspective: name.clone(),
                        scope: scope.clone(),
                        memo_ids: ids.clone(),
                        content: this.output.clone(),
                    };
                    let library = Library::global(cx);
                    library.update(cx, |lib, _| {
                        let _ = lib.store_mut().add_insight(&record);
                    });
                    this.history = library.read(cx).store().insights().unwrap_or_default();
                }
                StreamEvent::Failed(err) => this.status = Status::Failed(err),
            }
            cx.notify();
        }));
    }

    fn save_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.custom_name.read(cx).value().trim().to_string();
        let prompt = self.custom_prompt.read(cx).value().trim().to_string();
        if name.is_empty() || prompt.is_empty() {
            window.push_notification(t!("请填写视角名称和提示词"), cx);
            return;
        }
        let library = Library::global(cx);
        let _ = library.update(cx, |lib, cx| {
            lib.update_settings(cx, |s| s.custom_perspectives.push(CustomPerspective { name, prompt }))
        });
        self.custom_name.update(cx, |s, cx| s.set_value("", window, cx));
        self.custom_prompt.update(cx, |s, cx| s.set_value("", window, cx));
        self.show_custom_form = false;
        self.perspective = self.perspectives(cx).len() - 1;
        cx.notify();
    }

    fn render_scope(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let count = self.selected_memos(library.read(cx), cx).len();
        let theme = cx.theme();
        let scope_index = match &self.scope {
            Scope::All => 0,
            Scope::Tag(_) => 1,
            Scope::Memos(..) => 2,
        };
        let has_memos = matches!(self.scope, Scope::Memos(..));
        v_flex()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .child(div().text_sm().font_semibold().child(t!("1. 选择洞察的原料")))
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        TabBar::new("insight-scope")
                            .segmented()
                            .small()
                            .selected_index(scope_index)
                            .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                                this.scope = match *ix {
                                    0 => Scope::All,
                                    1 => Scope::Tag(String::new()),
                                    _ => this.scope.clone(),
                                };
                                cx.notify();
                            }))
                            .child(t!("全部笔记"))
                            .child(t!("按标签"))
                            .when(has_memos, |t| t.child(t!("指定笔记"))),
                    )
                    .when(scope_index == 1, |this| {
                        this.child(div().w(px(200.)).child(Input::new(&self.tag_input).small()))
                    }),
            )
            .when(scope_index != 2, |this| {
                this.child(
                    TabBar::new("insight-range")
                        .segmented()
                        .small()
                        .selected_index(self.range)
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            this.range = *ix;
                            cx.notify();
                        }))
                        .children(ranges().iter().map(|(l, _)| *l)),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if count > 60 {
                        t!("范围内共 {} 条笔记，将挑选 60 条有代表性的笔记（近期优先，兼顾全部时间）交给 AI", count)
                    } else {
                        t!("范围内共 {} 条笔记", count)
                    }),
            )
    }

    fn render_perspectives(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let perspectives = self.perspectives(cx);
        v_flex()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .child(div().text_sm().font_semibold().child(t!("2. 选择洞察视角")))
                    .child(div().flex_1())
                    .child(
                        Button::new("custom-perspective")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::Plus))
                            .label(t!("自定义视角"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_custom_form = !this.show_custom_form;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(perspectives.into_iter().enumerate().map(|(i, (name, desc, _, custom))| {
                        let selected = i == self.perspective;
                        v_flex()
                            .id(("perspective", i))
                            .w(px(152.))
                            .h(gpui_kit::rems(5.4))
                            .gap_0p5()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .cursor_pointer()
                            .border_color(if selected { theme.primary } else { theme.border })
                            .when(selected, |d| d.bg(theme.primary.opacity(0.06)))
                            .hover(|d| d.border_color(theme.primary.opacity(0.6)))
                            .child(
                                h_flex()
                                    .child(div().flex_1().text_sm().font_medium().child(name))
                                    .when_some(custom, |this, ci| {
                                        this.child(
                                            Button::new(("delete-perspective", ci))
                                                .ghost()
                                                .xsmall()
                                                .icon(Icon::new(Lucide::X))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    let _ = Library::global(cx).update(cx, |lib, cx| {
                                                        lib.update_settings(cx, |s| {
                                                            if ci < s.custom_perspectives.len() {
                                                                s.custom_perspectives.remove(ci);
                                                            }
                                                        })
                                                    });
                                                    this.perspective = 0;
                                                    cx.notify();
                                                })),
                                        )
                                    }),
                            )
                            .child(div().text_xs().text_color(theme.muted_foreground).line_clamp(2).child(desc))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.perspective = i;
                                cx.notify();
                            }))
                    })),
            )
            .when(self.show_custom_form, |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .p_3()
                        .rounded_md()
                        .bg(theme.secondary.opacity(0.5))
                        .child(Input::new(&self.custom_name).small())
                        .child(Textarea::new(&self.custom_prompt).small())
                        .child(
                            h_flex().justify_end().child(
                                Button::new("save-perspective")
                                    .primary()
                                    .small()
                                    .label(t!("保存视角"))
                                    .on_click(cx.listener(|this, _, window, cx| this.save_custom(window, cx))),
                            ),
                        ),
                )
            })
    }

    fn render_output(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.status == Status::Idle && self.output.is_empty() {
            return None;
        }
        let library = Library::global(cx);
        let lib = library.read(cx);
        let theme = cx.theme();
        let cited: Vec<(usize, &Memo)> = cited_indices(&self.output)
            .into_iter()
            .filter_map(|n| {
                self.context_ids
                    .get(n - 1)
                    .and_then(|id| lib.store().get(id))
                    .map(|m| (n, m))
            })
            .collect();
        Some(
            v_flex()
                .gap_3()
                .p_5()
                .rounded_lg()
                .bg(theme.background)
                .border_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(Lucide::Sparkles).small().text_color(theme.primary))
                        .child(div().text_sm().font_semibold().child(self.output_title.clone()))
                        .child(div().flex_1())
                        .when(self.status == Status::Running, |this| {
                            this.child(div().text_xs().text_color(theme.muted_foreground).child(t!("AI 正在阅读你的笔记…")))
                        }),
                )
                .when_some(
                    match &self.status {
                        Status::Failed(e) => Some(e.clone()),
                        _ => None,
                    },
                    |this, err| this.child(div().text_sm().text_color(theme.danger).child(err)),
                )
                .when(!self.output.is_empty(), |this| {
                    this.child(TextView::markdown("insight-output", self.output.clone()))
                })
                .when(!cited.is_empty(), |this| {
                    this.child(div().pt_2().text_xs().text_color(theme.muted_foreground).child(t!("洞察来源 · 点击回到灵感初现的现场")))
                        .children(cited.iter().map(|(n, m)| mini_card("insight-cite", m, Some(format!("[{n}]")), cx)))
                })
                .when(self.status == Status::Done, |this| {
                    let text = format!("{}\n\n#AI洞察", self.output.trim());
                    this.child(
                        h_flex().justify_end().child(
                            Button::new("insight-save-memo")
                                .outline()
                                .small()
                                .icon(Icon::new(Lucide::NotebookPen))
                                .label(t!("把这份洞察记成笔记"))
                                .on_click(move |_, window, cx| {
                                    let text = text.clone();
                                    let result = Library::global(cx).update(cx, |lib, cx| {
                                        lib.mutate(cx, |s| {
                                            s.create_memo(crate::core::store::NewMemo {
                                                content: text,
                                                created_at: None,
                                                images: vec![],
                                                source: "ai".into(),
                                            })
                                        })
                                    });
                                    if result.is_ok() {
                                        window.push_notification(t!("已保存为笔记"), cx);
                                    }
                                }),
                        ),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .gap_2()
            .when(!self.history.is_empty(), |this| {
                this.child(div().text_sm().font_semibold().pt_2().child(t!("洞察记录")))
            })
            .children(self.history.iter().take(30).map(|record| {
                let r = record.clone();
                let rid = record.id;
                h_flex()
                    .id(SharedString::from(format!("insight-history-{}", record.id)))
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border.opacity(0.6))
                    .cursor_pointer()
                    .hover(|d| d.border_color(theme.border))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(time::fmt_minute(record.created_at)))
                    .child(div().text_sm().font_medium().child(record.perspective.clone()))
                    .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme.muted_foreground).child(record.scope.clone()))
                    .child(
                        Button::new(SharedString::from(format!("insight-del-{rid}")))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::Trash))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let library = Library::global(cx);
                                library.update(cx, |lib, _| {
                                    let _ = lib.store_mut().delete_insight(rid);
                                });
                                this.history.retain(|h| h.id != rid);
                                cx.notify();
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this._task = None;
                        this.output = r.content.clone();
                        this.context_ids = r.memo_ids.clone();
                        this.output_title = format!("{} · {} · {}", r.perspective, r.scope, time::fmt_minute(r.created_at));
                        this.status = Status::Done;
                        cx.notify();
                    }))
            }))
    }
}

impl Render for InsightPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let configured = library.read(cx).settings().ai.is_configured();
        let running = self.status == Status::Running;
        let theme = cx.theme().clone();
        v_flex()
            .id("insight-scroll")
            .size_full()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(820.))
                    .mx_auto()
                    .px_6()
                    .py_4()
                    .gap_4()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(h_flex().gap_1().children(sidebar_toggle(cx)).child(div().text_lg().font_semibold().child(t!("AI 洞察"))))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(t!("深度阅读你挑选的笔记，从一个视角识别主题、揭示盲点、提出好问题。AI 不替你思考，而是帮你发现更好的问题。")),
                            ),
                    )
                    .when(!configured, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .p_3()
                                .rounded_md()
                                .bg(theme.warning.opacity(0.12))
                                .child(Icon::new(Lucide::CircleAlert).small().text_color(theme.warning))
                                .child(div().flex_1().text_sm().child(t!("还没有配置 AI 服务。支持 OpenAI、DeepSeek、通义千问、Kimi、Ollama 等兼容接口。")))
                                .child(
                                    Button::new("goto-settings")
                                        .small()
                                        .label(t!("去设置"))
                                        .on_click(|_, window, cx| dispatch(Command::OpenSettings(super::settings_page::SettingsSection::Ai), window, cx)),
                                ),
                        )
                    })
                    .child(self.render_scope(cx))
                    .child(self.render_perspectives(cx))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("run-insight")
                                    .primary()
                                    .icon(Icon::new(Lucide::Sparkles))
                                    .label(if running { t!("洞察中…") } else { t!("开始洞察") })
                                    .loading(running)
                                    .disabled(running || !configured)
                                    .on_click(cx.listener(|this, _, window, cx| this.run(window, cx))),
                            )
                            .when(running, |this| {
                                this.child(
                                    Button::new("stop-insight")
                                        .outline()
                                        .label(t!("停止"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this._task = None;
                                            this.status = Status::Done;
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .children(self.render_output(cx))
                    .child(self.render_history(cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn citations() {
        assert_eq!(super::cited_indices("如你所说 [3]，以及 [1, 4] 和 [3]"), vec![3, 1, 4]);
    }
}
