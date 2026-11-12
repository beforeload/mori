//! The memo input box, used both for new memos and for editing.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::input::{InlineToken, InputContent, InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, ClipboardEntry, Context, Entity, EventEmitter, FocusHandle, Focusable,
    ImageFormat, InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Styled as _, StyledImage as _, Subscription, Task, Window, actions, div, img, px,
    prelude::FluentBuilder as _,
};

use crate::core::model::MemoId;
use crate::core::parse;
use crate::core::store::NewMemo;
use crate::core::ai;

use super::Lucide;
use super::library::Library;
use super::tasks::run_blocking;

actions!(composer, [FormatBold, FormatHighlight, FormatUnderline]);

pub const CONTEXT: &str = "Composer";

#[derive(Clone, Debug, PartialEq)]
pub enum ComposerMode {
    New,
    Edit(MemoId),
}

pub enum ComposerEvent {
    Submitted(MemoId),
    Cancelled,
}

#[derive(Clone, Debug)]
enum Suggestion {
    Tags { start: usize, items: Vec<String> },
    Memos { start: usize, items: Vec<(MemoId, String)> },
}

pub struct Composer {
    input: Entity<TextareaState>,
    mode: ComposerMode,
    default_tag: Option<String>,
    images: Vec<String>,
    suggestion: Option<Suggestion>,
    ai_tags: Vec<String>,
    ai_loading: bool,
    error: Option<String>,
    _ai_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Focusable for Composer {
    fn focus_handle(&self, cx: &gpui_kit::App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

pub fn content_with_tokens(content: &str, library: &Library) -> InputContent {
    let mut value = InputContent::new(content.to_string());
    for (range, id) in parse::ref_spans(content) {
        let token = InlineToken::new(id.clone(), content[range.clone()].to_string())
            .with_label(format!("↗ {}", library.ref_label(&id)));
        if let Ok(next) = value.clone().with_token(range, token) {
            value = next;
        }
    }
    value
}

impl Composer {
    pub fn new(mode: ComposerMode, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 18)
                .placeholder(t!("现在的想法是..."))
        });
        let subscriptions = vec![cx.subscribe_in(&input, window, Self::on_input_event)];
        let mut composer = Self {
            input,
            mode: ComposerMode::New,
            default_tag: None,
            images: Vec::new(),
            suggestion: None,
            ai_tags: Vec::new(),
            ai_loading: false,
            error: None,
            _ai_task: None,
            _subscriptions: subscriptions,
        };
        composer.set_mode(mode, window, cx);
        composer
    }

    /// New memos written while a tag is open keep that tag.
    /// In a tag view the composer starts with `#tag `, like flomo; text the user typed is kept.
    pub fn set_default_tag(&mut self, tag: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.input.read(cx).value().trim().to_string();
        let untouched = current.is_empty() || self.default_tag.as_ref().is_some_and(|t| current == format!("#{t}"));
        self.default_tag = tag;
        if untouched && self.mode == ComposerMode::New {
            self.prefill_default_tag(window, cx);
        }
        cx.notify();
    }

    fn prefill_default_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.default_tag.as_ref().map(|t| format!("#{t} ")).unwrap_or_default();
        self.set_text(&text, window, cx);
    }

    pub fn set_mode(&mut self, mode: ComposerMode, window: &mut Window, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        match &mode {
            ComposerMode::New => {
                self.images.clear();
            }
            ComposerMode::Edit(id) => {
                let lib = library.read(cx);
                if let Some(memo) = lib.store().get(id) {
                    let content = content_with_tokens(&memo.content, lib);
                    self.images = memo.images.clone();
                    self.input.update(cx, |state, cx| state.set_value(content, window, cx));
                }
            }
        }
        self.mode = mode;
        self.suggestion = None;
        self.ai_tags.clear();
        cx.notify();
    }

    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        let content = content_with_tokens(text, library.read(cx));
        self.input.update(cx, |state, cx| {
            state.set_value(content, window, cx);
            let end = text.len();
            state.set_selected_range(end..end, cx);
        });
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| state.focus(window, cx));
    }

    pub fn is_empty(&self, cx: &gpui_kit::App) -> bool {
        self.input.read(cx).value().trim().is_empty() && self.images.is_empty()
    }

    fn on_input_event(
        &mut self,
        state: &Entity<TextareaState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.error = None;
                self.refresh_suggestion(state, cx);
            }
            InputEvent::PressEnter { secondary: true, .. } => self.submit(window, cx),
            _ => {}
        }
    }

    fn refresh_suggestion(&mut self, state: &Entity<TextareaState>, cx: &mut Context<Self>) {
        let (text, cursor) = {
            let s = state.read(cx);
            (s.value().to_string(), s.cursor())
        };
        let library = Library::global(cx);
        let lib = library.read(cx);
        self.suggestion = None;
        if let Some((start, query)) = parse::token_before_cursor(&text, cursor, '#') {
            let counts = lib.tag_counts();
            let q = query.to_lowercase();
            let mut items: Vec<(String, usize)> = counts
                .iter()
                .filter(|(tag, _)| q.is_empty() || tag.to_lowercase().contains(&q))
                .map(|(t, c)| (t.clone(), *c))
                .collect();
            items.sort_by(|a, b| {
                let ap = a.0.to_lowercase().starts_with(&q);
                let bp = b.0.to_lowercase().starts_with(&q);
                bp.cmp(&ap).then(b.1.cmp(&a.1))
            });
            let mut items: Vec<String> = items.into_iter().take(8).map(|(t, _)| t).collect();
            if !query.is_empty() && !items.contains(&query) {
                items.push(query.clone());
            }
            if !items.is_empty() {
                self.suggestion = Some(Suggestion::Tags { start, items });
            }
        } else if let Some((start, query)) = parse::token_before_cursor(&text, cursor, '@') {
            let q = query.to_lowercase();
            let editing = match &self.mode {
                ComposerMode::Edit(id) => Some(id.clone()),
                ComposerMode::New => None,
            };
            let items: Vec<(MemoId, String)> = lib
                .store()
                .active_memos()
                .filter(|m| Some(&m.id) != editing.as_ref())
                .filter(|m| q.is_empty() || m.content.to_lowercase().contains(&q))
                .take(6)
                .map(|m| (m.id.clone(), m.snippet(40)))
                .collect();
            self.suggestion = Some(Suggestion::Memos { start, items });
        }
        cx.notify();
    }

    fn apply_tag(&mut self, start: usize, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| {
            let cursor = state.cursor();
            state.set_selected_range(start..cursor, cx);
            state.replace(format!("#{tag} "), window, cx);
            state.focus(window, cx);
        });
        self.suggestion = None;
        cx.notify();
    }

    fn apply_reference(&mut self, start: usize, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let label = Library::global(cx).read(cx).ref_label(id);
        self.input.update(cx, |state, cx| {
            let cursor = state.cursor();
            let token = InlineToken::new(id.to_string(), parse::ref_token(id)).with_label(format!("↗ {label}"));
            if state.replace_range_with_token(start..cursor, token, window, cx).is_ok() {
                state.insert(" ", window, cx);
            }
            state.focus(window, cx);
        });
        self.suggestion = None;
        cx.notify();
    }

    pub fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| {
            state.insert(text.to_string(), window, cx);
            state.focus(window, cx);
        });
    }

    fn wrap_selection(&mut self, marker: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| {
            let range = state.selected_range();
            let selected = state.value()[range.clone()].to_string();
            state.replace(format!("{marker}{selected}{marker}"), window, cx);
            if selected.is_empty() {
                let caret = range.start + marker.len();
                state.set_selected_range(caret..caret, cx);
            }
            state.focus(window, cx);
        });
    }

    fn prefix_line(&mut self, prefix: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let cursor = state.cursor();
            let line_start = text[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
            state.set_selected_range(line_start..line_start, cx);
            state.replace(prefix.to_string(), window, cx);
            let caret = cursor + prefix.len();
            state.set_selected_range(caret..caret, cx);
            state.focus(window, cx);
        });
    }

    fn pick_images(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t!("添加图片").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let library = Library::global(cx);
                for path in paths {
                    match library.read(cx).store().import_image_file(&path) {
                        Ok(name) => this.images.push(name),
                        Err(err) => this.error = Some(t!("无法添加图片：{}", err)),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn paste_images(&mut self, entries: Vec<(Vec<u8>, &'static str)>, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        for (bytes, ext) in entries {
            match library.read(cx).store().save_image_bytes(&bytes, ext) {
                Ok(name) => self.images.push(name),
                Err(err) => self.error = Some(t!("无法保存图片：{}", err)),
            }
        }
        cx.notify();
    }

    fn suggest_tags_with_ai(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let content = self.input.read(cx).value().to_string();
        if content.trim().is_empty() {
            self.error = Some(t!("先写点内容，再让 AI 推荐标签").into());
            cx.notify();
            return;
        }
        let library = Library::global(cx);
        let lib = library.read(cx);
        let cfg = lib.settings().ai.clone();
        if !cfg.is_configured() || cfg.api_key.trim().is_empty() && cfg.base_url.contains("openai.com") {
            let memo_tags = lib
                .store()
                .active_memos()
                .map(|m| (m.id.clone(), m.tags.clone()))
                .collect();
            self.ai_tags = lib.index().suggest_tags(&content, &memo_tags, 5);
            if self.ai_tags.is_empty() {
                self.error = Some(t!("没有找到相似笔记的标签；配置 AI 服务后可获得更好的推荐").into());
            }
            cx.notify();
            return;
        }
        let existing: Vec<String> = lib.tag_counts().keys().cloned().collect();
        self.ai_loading = true;
        cx.notify();
        self._ai_task = Some(cx.spawn(async move |this, cx| {
            let result = run_blocking(move || {
                ai::chat_complete(&cfg, &ai::tag_suggest_messages(&content, &existing))
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.ai_loading = false;
                match result {
                    Some(Ok(reply)) => {
                        this.ai_tags = ai::parse_string_list(&reply);
                        if this.ai_tags.is_empty() {
                            this.error = Some(t!("AI 没有给出标签建议").into());
                        }
                    }
                    Some(Err(err)) => this.error = Some(err.to_string()),
                    None => {}
                }
                cx.notify();
            });
        }));
    }

    fn append_tag(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let end = text.len();
            state.set_selected_range(end..end, cx);
            let sep = if text.is_empty() || text.ends_with(char::is_whitespace) { "" } else { " " };
            state.replace(format!("{sep}#{tag} "), window, cx);
            state.focus(window, cx);
        });
        self.ai_tags.retain(|t| t != tag);
        cx.notify();
    }

    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let content = self.input.read(cx).value().trim().to_string();
        if content.is_empty() && self.images.is_empty() {
            return;
        }
        let library = Library::global(cx);
        let images = self.images.clone();
        let mode = self.mode.clone();
        let result = library.update(cx, |lib, cx| {
            lib.mutate(cx, |store| match &mode {
                ComposerMode::New => store.create_memo(NewMemo {
                    content: content.clone(),
                    created_at: None,
                    images,
                    source: "app".into(),
                }),
                ComposerMode::Edit(id) => store.update_memo(id, content.clone(), images).map(|_| id.clone()),
            })
        });
        match result {
            Ok(id) => {
                if mode == ComposerMode::New {
                    self.prefill_default_tag(window, cx);
                    self.images.clear();
                    self.ai_tags.clear();
                }
                self.suggestion = None;
                cx.emit(ComposerEvent::Submitted(id));
            }
            Err(err) => {
                window.push_notification(t!("保存失败：{}", err), cx);
            }
        }
        cx.notify();
    }

    fn render_suggestions(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let suggestion = self.suggestion.clone()?;
        let theme = cx.theme();
        let row = h_flex()
            .flex_wrap()
            .gap_1()
            .px_2()
            .py_1p5()
            .rounded_md()
            .bg(theme.secondary)
            .text_xs();
        Some(match suggestion {
            Suggestion::Tags { start, items } => row
                .child(div().text_color(theme.muted_foreground).mr_1().child(t!("标签")))
                .children(items.into_iter().enumerate().map(|(i, tag)| {
                    let t = tag.clone();
                    Button::new(("tag-suggest", i))
                        .ghost()
                        .xsmall()
                        .label(format!("#{tag}"))
                        .on_click(cx.listener(move |this, _, window, cx| this.apply_tag(start, &t, window, cx)))
                })),
            Suggestion::Memos { start, items } => {
                let empty = items.is_empty();
                row.flex_col()
                    .items_start()
                    .child(div().text_color(theme.muted_foreground).child(t!("引用笔记（输入关键词筛选）")))
                    .when(empty, |r| r.child(div().text_color(theme.muted_foreground).child(t!("没有匹配的笔记"))))
                    .children(items.into_iter().map(|(id, snippet)| {
                        let rid = id.clone();
                        Button::new(SharedString::from(format!("ref-suggest-{id}")))
                            .ghost()
                            .xsmall()
                            .label(snippet)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.apply_reference(start, &rid, window, cx)
                            }))
                    }))
            }
        })
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let editing = matches!(self.mode, ComposerMode::Edit(_));
        let empty = self.is_empty(cx);
        let muted = cx.theme().muted_foreground;
        let divider = || div().w_px().h_4().bg(cx.theme().border).mx_1p5();
        let tool = |id: &'static str, icon: Lucide, tip: &'static str| {
            Button::new(id)
                .ghost()
                .small()
                .icon(Icon::new(icon).text_color(muted))
                .tooltip(tip)
        };
        let view = cx.entity().downgrade();
        h_flex()
            .gap_0p5()
            .child(tool("tb-tag", Lucide::Hash, t!("插入标签")).on_click(cx.listener(|this, _, window, cx| this.insert_text("#", window, cx))))
            .child(tool("tb-image", Lucide::Image, t!("添加图片（也可直接粘贴）")).on_click(cx.listener(|this, _, window, cx| this.pick_images(window, cx))))
            .child(divider())
            .child(
                Button::new("tb-format")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::ALargeSmall).text_color(muted))
                    .tooltip(t!("文本格式"))
                    .dropdown_menu_with_anchor(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                        let item = |label: &'static str, icon: Lucide, marker: &'static str| {
                            let view = view.clone();
                            gpui_kit::component::menu::PopupMenuItem::new(label)
                                .icon(Icon::new(icon))
                                .on_click(move |_, window, cx| {
                                    let _ = view.update(cx, |this, cx| this.wrap_selection(marker, window, cx));
                                })
                        };
                        menu.item(item(t!("加粗  ⌘B"), Lucide::Bold, "**"))
                            .item(item(t!("高亮  ⌘⇧H"), Lucide::Highlighter, "=="))
                            .item(item(t!("下划线  ⌘U"), Lucide::Underline, "__"))
                            .item(item(t!("删除线"), Lucide::Strikethrough, "~~"))
                    }),
            )
            .child(tool("tb-list", Lucide::List, t!("无序列表")).on_click(cx.listener(|this, _, window, cx| this.prefix_line("- ", window, cx))))
            .child(tool("tb-olist", Lucide::ListOrdered, t!("有序列表")).on_click(cx.listener(|this, _, window, cx| this.prefix_line("1. ", window, cx))))
            .child(divider())
            .child(tool("tb-ref", Lucide::AtSign, t!("引用笔记")).on_click(cx.listener(|this, _, window, cx| this.insert_text("@", window, cx))))
            .child(
                tool("tb-ai-tags", Lucide::WandSparkles, t!("推荐标签"))
                    .loading(self.ai_loading)
                    .on_click(cx.listener(|this, _, window, cx| this.suggest_tags_with_ai(window, cx))),
            )
            .child(div().flex_1())
            .when(editing, |this| {
                this.child(
                    Button::new("tb-cancel")
                        .ghost()
                        .small()
                        .label(t!("取消"))
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::Cancelled))),
                )
            })
            .child(
                Button::new("tb-send")
                    .small()
                    .w(px(46.))
                    .when(empty, |b| b.secondary())
                    .when(!empty, |b| b.primary())
                    .icon(Icon::new(Lucide::SendHorizontal))
                    .disabled(empty)
                    .tooltip(if editing { t!("保存 ⌘↩") } else { t!("发送 ⌘↩") })
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            )
    }
}

fn image_extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        _ => "png",
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = cx.entity().downgrade();
        let library = Library::global(cx);
        let store_images: Vec<(usize, std::path::PathBuf)> = self
            .images
            .iter()
            .enumerate()
            .map(|(i, name)| (i, library.read(cx).store().image_path(name)))
            .collect();
        let focused = self.input.focus_handle(cx).is_focused(window);
        let s = super::theme::surfaces(cx);
        v_flex()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &FormatBold, window, cx| this.wrap_selection("**", window, cx)))
            .on_action(cx.listener(|this, _: &FormatHighlight, window, cx| this.wrap_selection("==", window, cx)))
            .on_action(cx.listener(|this, _: &FormatUnderline, window, cx| this.wrap_selection("__", window, cx)))
            .w_full()
            .gap_2()
            .px_3p5()
            .pt_3()
            .pb_2p5()
            .rounded_lg()
            .bg(s.card)
            .border_1()
            .border_color(if focused { s.accent.opacity(0.55) } else { theme.border })
            .child(
                Textarea::new(&self.input)
                    .appearance(false)
                    .on_paste(move |item, _, cx| {
                        let images: Vec<(Vec<u8>, &'static str)> = item
                            .entries()
                            .iter()
                            .filter_map(|entry| match entry {
                                ClipboardEntry::Image(image) => {
                                    Some((image.bytes.clone(), image_extension(image.format)))
                                }
                                _ => None,
                            })
                            .collect();
                        if images.is_empty() {
                            return false;
                        }
                        let _ = view.update(cx, |this, cx| this.paste_images(images, cx));
                        true
                    }),
            )
            .children(self.render_suggestions(cx))
            .when(!self.ai_tags.is_empty(), |this| {
                this.child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .text_xs()
                        .child(div().text_color(theme.muted_foreground).child(t!("推荐标签")))
                        .children(self.ai_tags.iter().enumerate().map(|(i, tag)| {
                            let t = tag.clone();
                            Button::new(("ai-tag", i))
                                .outline()
                                .xsmall()
                                .label(format!("#{tag}"))
                                .on_click(cx.listener(move |this, _, window, cx| this.append_tag(&t, window, cx)))
                        })),
                )
            })
            .when(!store_images.is_empty(), |this| {
                this.child(h_flex().flex_wrap().gap_2().children(store_images.into_iter().map(|(i, path)| {
                    div()
                        .id(("composer-image", i))
                        .relative()
                        .size_16()
                        .rounded_md()
                        .overflow_hidden()
                        .child(img(path).size_full().object_fit(gpui_kit::ObjectFit::Cover))
                        .child(
                            div().absolute().top_0().right_0().child(
                                Button::new(("remove-image", i))
                                    .xsmall()
                                    .icon(Icon::new(Lucide::X))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if i < this.images.len() {
                                            this.images.remove(i);
                                        }
                                        cx.notify();
                                    })),
                            ),
                        )
                })))
            })
            .when_some(self.error.clone(), |this, err| {
                this.child(div().text_xs().text_color(theme.danger).child(err))
            })
            .child(self.render_toolbar(cx))
    }
}
