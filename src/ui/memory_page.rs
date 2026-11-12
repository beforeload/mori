//! Memory profile: distill memos into a structured memory that other AI tools can read.

use std::collections::{BTreeMap, HashSet};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Task, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::core::ai::{self, ChatMessage};
use crate::core::model::{AiConfig, Memo, MemoryFragment, MemoryProfile};
use crate::core::time;

use super::sidebar::sidebar_toggle;
use super::library::Library;
use super::tasks::run_blocking;
use super::{Command, Lucide, dispatch};

const MIN_MEMOS: usize = 10;
fn ranges() -> [(&'static str, Option<i64>); 4] { [
    (t!("全部时间"), None),
    (t!("近一年"), Some(365)),
    (t!("近半年"), Some(182)),
    (t!("近三个月"), Some(91)),
]}

#[derive(Clone, Debug, PartialEq)]
enum Stage {
    Idle,
    Reading,
    Extracting { done: usize, total: usize },
    Integrating,
    Done,
    Failed(String),
}

enum Progressed {
    Extracting(usize, usize),
    Integrating,
    Finished(MemoryProfile),
    Failed(String),
}

fn generate(cfg: AiConfig, batches: Vec<(String, Vec<String>)>, memo_count: usize, tx: smol::channel::Sender<Progressed>) {
    let total = batches.len();
    let mut fragments: Vec<MemoryFragment> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut last_error = None;
    for (i, (notes, ids)) in batches.iter().enumerate() {
        let _ = tx.send_blocking(Progressed::Extracting(i, total));
        match ai::chat_complete(&cfg, &ai::memory_extract_messages(notes)) {
            Ok(reply) => {
                for fragment in ai::parse_fragments(&reply, ids) {
                    if seen.insert(fragment.content.clone()) {
                        fragments.push(fragment);
                    }
                }
            }
            Err(err) => {
                if i == 0 {
                    let _ = tx.send_blocking(Progressed::Failed(err.to_string()));
                    return;
                }
                last_error = Some(err.to_string());
            }
        }
    }
    if fragments.is_empty() {
        let _ = tx.send_blocking(Progressed::Failed(
            last_error.unwrap_or_else(|| "没有从笔记中提取到关于你的记忆，试试扩大笔记范围".into()),
        ));
        return;
    }
    let _ = tx.send_blocking(Progressed::Integrating);
    let input: Vec<MemoryFragment> = fragments.iter().take(400).cloned().collect();
    match ai::chat_complete(&cfg, &ai::memory_integrate_messages(&input)) {
        Ok(reply) => {
            let (profile, document) = ai::parse_memory_documents(&reply);
            let _ = tx.send_blocking(Progressed::Finished(MemoryProfile {
                profile,
                document,
                fragments,
                generated_at: time::now(),
                memo_count,
            }));
        }
        Err(err) => {
            let _ = tx.send_blocking(Progressed::Failed(err.to_string()));
        }
    }
}

pub struct MemoryPage {
    profile: MemoryProfile,
    stage: Stage,
    tab: usize,
    range: usize,
    tag_input: Entity<InputState>,
    compare_input: Entity<InputState>,
    compare: Option<(String, String)>,
    comparing: bool,
    _task: Option<Task<()>>,
    _compare_task: Option<Task<()>>,
}

impl MemoryPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let profile = Library::global(cx).read(cx).store().memory_profile();
        let tag_input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("只看某个标签（可选）")));
        let compare_input = cx.new(|cx| {
            InputState::new(window, cx).default_value(t!("给我推荐一本适合现在的我读的书，并说明理由。"))
        });
        Self {
            stage: if profile.is_empty() { Stage::Idle } else { Stage::Done },
            profile,
            tab: 0,
            range: 0,
            tag_input,
            compare_input,
            compare: None,
            comparing: false,
            _task: None,
            _compare_task: None,
        }
    }

    fn selected<'a>(&self, lib: &'a Library, cx: &gpui_kit::App) -> Vec<&'a Memo> {
        let cutoff = ranges()[self.range]
            .1
            .map(|d| time::start_of_day(time::today() - chrono::Duration::days(d)));
        let tag = self.tag_input.read(cx).value().trim().trim_start_matches('#').to_string();
        lib.store()
            .active_memos()
            .filter(|m| cutoff.is_none_or(|c| m.created_at >= c))
            .filter(|m| tag.is_empty() || m.has_tag_or_child(&tag))
            .collect()
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let cfg = lib.settings().ai.clone();
        if !cfg.is_configured() {
            window.push_notification(t!("请先在设置中配置 AI 服务"), cx);
            return;
        }
        let memos = self.selected(lib, cx);
        if memos.len() < MIN_MEMOS {
            self.stage = Stage::Failed(t!("至少需要 {} 条笔记才能生成记忆档案（建议 30 条以上），当前范围内只有 {} 条", MIN_MEMOS, memos.len()));
            cx.notify();
            return;
        }
        let memo_count = memos.len();
        let sampled = ai::sample_memos(&memos, 500);
        let batches: Vec<(String, Vec<String>)> = sampled
            .chunks(25)
            .map(|chunk| ai::format_notes(chunk, 9_000))
            .collect();
        self.stage = Stage::Reading;
        cx.notify();
        let (tx, rx) = smol::channel::unbounded();
        std::thread::spawn(move || generate(cfg, batches, memo_count, tx));
        self._task = Some(cx.spawn(async move |this, cx| {
            while let Ok(progress) = rx.recv().await {
                let finished = matches!(progress, Progressed::Finished(_) | Progressed::Failed(_));
                let _ = this.update(cx, |this, cx| {
                    match progress {
                        Progressed::Extracting(done, total) => this.stage = Stage::Extracting { done, total },
                        Progressed::Integrating => this.stage = Stage::Integrating,
                        Progressed::Finished(profile) => {
                            Library::global(cx).update(cx, |lib, _| {
                                let _ = lib.store_mut().save_memory_profile(&profile);
                            });
                            this.profile = profile;
                            this.stage = Stage::Done;
                            this.tab = 0;
                        }
                        Progressed::Failed(err) => this.stage = Stage::Failed(err),
                    }
                    cx.notify();
                });
                if finished {
                    break;
                }
            }
        }));
    }

    fn run_compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let question = self.compare_input.read(cx).value().trim().to_string();
        if question.is_empty() || self.profile.is_empty() {
            return;
        }
        let cfg = Library::global(cx).read(cx).settings().ai.clone();
        if !cfg.is_configured() {
            window.push_notification(t!("请先在设置中配置 AI 服务"), cx);
            return;
        }
        let memory = self.profile.to_markdown();
        self.comparing = true;
        self.compare = None;
        cx.notify();
        self._compare_task = Some(cx.spawn(async move |this, cx| {
            let (cfg_a, cfg_b, q_a, q_b) = (cfg.clone(), cfg, question.clone(), question);
            let with = run_blocking(move || {
                ai::chat_complete(
                    &cfg_a,
                    &[
                        ChatMessage::system(format!("你是用户的 AI 助手。以下是用户的记忆档案，请结合它回答：\n{memory}")),
                        ChatMessage::user(q_a),
                    ],
                )
            });
            let without = run_blocking(move || {
                ai::chat_complete(&cfg_b, &[ChatMessage::system("你是一个乐于助人的 AI 助手。"), ChatMessage::user(q_b)])
            });
            let (a, b) = (with.await, without.await);
            let fmt = |r: Option<anyhow::Result<String>>| match r {
                Some(Ok(t)) => t,
                Some(Err(e)) => t!("出错了：{}", e),
                None => t!("出错了").into(),
            };
            let _ = this.update(cx, |this, cx| {
                this.compare = Some((fmt(a), fmt(b)));
                this.comparing = false;
                cx.notify();
            });
        }));
    }

    fn export(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let markdown = self.profile.to_markdown();
        let dir = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_default();
        let path = cx.prompt_for_new_path(&dir, Some("memory.md"));
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(path))) = path.await {
                let result = std::fs::write(&path, markdown);
                cx.update(|cx| {
                    if let Some(window) = cx.active_window() {
                        let _ = window.update(cx, |_, window, cx| {
                            window.push_notification(
                                match result {
                                    Ok(()) => t!("已导出到 {}", path.display()),
                                    Err(e) => t!("导出失败：{}", e),
                                },
                                cx,
                            )
                        });
                    }
                });
            }
        })
        .detach();
    }

    fn render_progress(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let (label, value) = match &self.stage {
            Stage::Reading => (t!("读取笔记…").to_string(), 5.0),
            Stage::Extracting { done, total } => (
                t!("提取记忆碎片 {}/{}", done + 1, total),
                10.0 + 75.0 * (*done as f32 / (*total).max(1) as f32),
            ),
            Stage::Integrating => (t!("整合档案…").to_string(), 90.0),
            Stage::Failed(err) => {
                return Some(div().text_sm().text_color(theme.danger).child(err.clone()).into_any_element());
            }
            _ => return None,
        };
        Some(
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .gap_4()
                        .text_xs()
                        .children([t!("读取笔记"), t!("提取记忆"), t!("整合档案"), t!("完成")].iter().enumerate().map(|(i, s)| {
                            let active = match &self.stage {
                                Stage::Reading => i == 0,
                                Stage::Extracting { .. } => i == 1,
                                Stage::Integrating => i == 2,
                                _ => false,
                            };
                            div()
                                .when(active, |d| d.text_color(theme.primary).font_semibold())
                                .when(!active, |d| d.text_color(theme.muted_foreground))
                                .child(s.to_string())
                        })),
                )
                .child(Progress::new("memory-progress").value(value))
                .child(div().text_xs().text_color(theme.muted_foreground).child(t!("{}  可以先去做别的事，完成后这里会显示结果。", label)))
                .into_any_element(),
        )
    }

    fn render_fragments(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mut groups: BTreeMap<String, Vec<&MemoryFragment>> = BTreeMap::new();
        for f in &self.profile.fragments {
            groups.entry(f.category.clone()).or_default().push(f);
        }
        v_flex()
            .gap_4()
            .children(groups.into_iter().map(|(category, items)| {
                v_flex()
                    .gap_1p5()
                    .child(div().text_sm().font_semibold().child(format!("{category} · {}", items.len())))
                    .children(items.into_iter().enumerate().map(|(i, f)| {
                        let source = f.memo_ids.first().cloned();
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(div().text_color(theme.muted_foreground).child("·"))
                            .child(div().flex_1().child(f.content.clone()))
                            .when_some(source, |this, id| {
                                this.child(
                                    Button::new(SharedString::from(format!("frag-src-{category}-{i}")))
                                        .ghost()
                                        .xsmall()
                                        .label(t!("来源"))
                                        .on_click(move |_, window, cx| dispatch(Command::OpenMemo(id.clone()), window, cx)),
                                )
                            })
                    }))
            }))
            .into_any_element()
    }
}

impl Render for MemoryPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let configured = lib.settings().ai.is_configured();
        let count = self.selected(lib, cx).len();
        let running = matches!(self.stage, Stage::Reading | Stage::Extracting { .. } | Stage::Integrating);
        let has_profile = !self.profile.is_empty();
        let theme = cx.theme().clone();
        let body = match self.tab {
            0 => TextView::markdown("memory-profile", self.profile.profile.clone()).into_any_element(),
            1 => TextView::markdown("memory-document", self.profile.document.clone()).into_any_element(),
            _ => self.render_fragments(cx),
        };
        v_flex()
            .id("memory-scroll")
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
                            .child(h_flex().gap_1().children(sidebar_toggle(cx)).child(div().text_lg().font_semibold().child(t!("记忆档案"))))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(t!("把散落的笔记萃取成一份 AI 能读懂的结构化记忆：「用户画像」记录你是谁，「记忆文档」记录你在想什么。通过 MCP 或复制，让任何 AI 都真正了解你。")),
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
                                .child(div().flex_1().text_sm().child(t!("生成记忆档案需要先配置 AI 服务")))
                                .child(Button::new("memory-settings").small().label(t!("去设置")).on_click(|_, window, cx| {
                                    dispatch(Command::OpenSettings(super::settings_page::SettingsSection::Ai), window, cx)
                                })),
                        )
                    })
                    .child(
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
                                    .child(
                                        TabBar::new("memory-range")
                                            .segmented()
                                            .small()
                                            .selected_index(self.range)
                                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                                this.range = *ix;
                                                cx.notify();
                                            }))
                                            .children(ranges().iter().map(|(l, _)| *l)),
                                    )
                                    .child(div().w(px(200.)).child(Input::new(&self.tag_input).small()))
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("memory-generate")
                                            .primary()
                                            .icon(Icon::new(Lucide::Brain))
                                            .label(if has_profile { t!("重新生成") } else { t!("开始生成") })
                                            .loading(running)
                                            .disabled(running || !configured)
                                            .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(t!("范围内共 {} 条笔记（至少 {} 条，建议 30 条以上）。AI 会逐批阅读笔记、提取记忆碎片，再整合成档案，可能需要几分钟。", count, MIN_MEMOS)),
                            )
                            .children(self.render_progress(cx)),
                    )
                    .when(has_profile, |this| {
                        this.child(
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
                                        .child(
                                            TabBar::new("memory-tabs")
                                                .underline()
                                                .selected_index(self.tab)
                                                .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                                    this.tab = *ix;
                                                    cx.notify();
                                                }))
                                                .child(t!("用户画像"))
                                                .child(t!("记忆文档"))
                                                .child(t!("记忆碎片 {}", self.profile.fragments.len())),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child(t!(
                                                    "{} · 基于 {} 条笔记",
                                                    time::fmt_minute(self.profile.generated_at),
                                                    self.profile.memo_count
                                                )),
                                        ),
                                )
                                .child(body)
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .justify_end()
                                        .child(
                                            Button::new("memory-copy")
                                                .outline()
                                                .small()
                                                .icon(Icon::new(Lucide::Copy))
                                                .label(t!("复制记忆档案"))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    cx.write_to_clipboard(ClipboardItem::new_string(this.profile.to_markdown()));
                                                    window.push_notification(t!("已复制，可粘贴到任意 AI 的系统提示词中"), cx);
                                                })),
                                        )
                                        .child(
                                            Button::new("memory-export")
                                                .outline()
                                                .small()
                                                .icon(Icon::new(Lucide::FileDown))
                                                .label(t!("导出 memory.md"))
                                                .on_click(cx.listener(|this, _, window, cx| this.export(window, cx))),
                                        ),
                                ),
                        )
                        .child(
                            v_flex()
                                .gap_3()
                                .p_5()
                                .rounded_lg()
                                .bg(theme.background)
                                .border_1()
                                .border_color(theme.border)
                                .child(div().text_sm().font_semibold().child(t!("对比记忆效果")))
                                .child(div().text_xs().text_color(theme.muted_foreground).child(t!("同一个问题，分别在有记忆和没有记忆时回答，感受一下差别。")))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(div().flex_1().child(Input::new(&self.compare_input).small()))
                                        .child(
                                            Button::new("memory-compare")
                                                .small()
                                                .label(t!("对比"))
                                                .loading(self.comparing)
                                                .disabled(self.comparing || !configured)
                                                .on_click(cx.listener(|this, _, window, cx| this.run_compare(window, cx))),
                                        ),
                                )
                                .when_some(self.compare.clone(), |this, (with, without)| {
                                    this.child(
                                        h_flex()
                                            .gap_3()
                                            .items_start()
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .gap_2()
                                                    .p_3()
                                                    .rounded_md()
                                                    .bg(theme.green.opacity(0.08))
                                                    .child(div().text_xs().font_semibold().child(t!("有记忆")))
                                                    .child(TextView::markdown("compare-with", with)),
                                            )
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .gap_2()
                                                    .p_3()
                                                    .rounded_md()
                                                    .bg(theme.secondary.opacity(0.6))
                                                    .child(div().text_xs().font_semibold().child(t!("没有记忆")))
                                                    .child(TextView::markdown("compare-without", without)),
                                            ),
                                    )
                                }),
                        )
                    }),
            )
    }
}
