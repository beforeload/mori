//! The main memo stream: title menu, search with filters, composer and the virtualized card list.

use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use chrono::NaiveDate;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ListAlignment, ListState, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity, Window, div, list,
    prelude::FluentBuilder as _, px,
};

use crate::core::model::{Memo, MemoId};
use crate::core::query::{MemoFilter, QuickFilter, SortOrder, filter_memos};
use crate::core::{ai, io, share, stats, time};

use super::composer::{Composer, ComposerEvent, ComposerMode};
use super::dialogs::prompt_text;
use super::library::Library;
use super::memo_card::{CardOptions, danger_menu_item, memo_card, menu_item};
use super::sidebar::sidebar_toggle;
use super::tasks::run_blocking;
use super::theme::surfaces;
use super::{Command, Lucide, Route, dispatch};

/// Cards grow with the window up to this width, then stay centred.
const CONTENT_MAX_WIDTH: f32 = 1080.;

/// (source key, label) for the 笔记来源 filter; `None` shows every source.
fn sources() -> [(Option<&'static str>, &'static str); 6] { [
    (None, t!("全部")),
    (Some("app"), t!("应用内")),
    (Some("api"), "API"),
    (Some("mcp"), "MCP"),
    (Some("ai"), t!("AI 对话")),
    (Some("import"), t!("导入")),
]}

#[derive(Clone, Copy)]
enum RangePreset {
    Week,
    Month,
    Year,
}

pub struct MemoStream {
    filter: MemoFilter,
    sort: SortOrder,
    semantic: bool,
    query_vector: Option<(String, Vec<f32>)>,
    search: Entity<InputState>,
    date_from: Entity<InputState>,
    date_to: Entity<InputState>,
    include_tags: Entity<InputState>,
    exclude_tags: Entity<InputState>,
    advanced_error: Option<String>,
    composer: Entity<Composer>,
    editing: Option<(MemoId, Entity<Composer>)>,
    expanded: HashSet<MemoId>,
    highlighted: Option<MemoId>,
    /// `Some` while 多选笔记 is active.
    selection: Option<HashSet<MemoId>>,
    items: Rc<Vec<MemoId>>,
    list_state: ListState,
    _query_task: Option<Task<()>>,
    _edit_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

fn parse_tag_list(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ',' || c == '，')
        .map(|t| t.trim().trim_start_matches('#').to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

impl MemoStream {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("⌘+K"));
        let date_from = cx.new(|cx| InputState::new(window, cx).placeholder(t!("开始 2024-01-01")));
        let date_to = cx.new(|cx| InputState::new(window, cx).placeholder(t!("结束 2024-12-31")));
        let include_tags = cx.new(|cx| InputState::new(window, cx).placeholder(t!("包含的标签，空格分隔")));
        let exclude_tags = cx.new(|cx| InputState::new(window, cx).placeholder(t!("不包含的标签，空格分隔")));
        let composer = cx.new(|cx| Composer::new(ComposerMode::New, window, cx));
        let library = Library::global(cx);
        let settings = library.read(cx).settings().clone();
        let sort = match (settings.sort_by_updated, settings.sort_ascending) {
            (true, true) => SortOrder::UpdatedAsc,
            (true, false) => SortOrder::UpdatedDesc,
            (false, true) => SortOrder::CreatedAsc,
            (false, false) => SortOrder::CreatedDesc,
        };
        let subscriptions = vec![
            cx.observe(&library, |this, _, cx| this.refresh(cx)),
            cx.subscribe_in(&search, window, |this, state, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.filter.keywords = state.read(cx).value().to_string();
                    this.schedule_query_embedding(cx);
                    this.refresh(cx);
                }
            }),
            cx.subscribe_in(&composer, window, |this, _, event, _, cx| {
                if let ComposerEvent::Submitted(id) = event {
                    this.highlighted = Some(id.clone());
                    this.list_state.scroll_to(gpui_kit::ListOffset::default());
                    cx.notify();
                }
            }),
        ];
        let mut stream = Self {
            filter: MemoFilter::default(),
            sort,
            semantic: false,
            query_vector: None,
            search,
            date_from,
            date_to,
            include_tags,
            exclude_tags,
            advanced_error: None,
            composer,
            editing: None,
            expanded: HashSet::new(),
            highlighted: None,
            selection: None,
            items: Rc::new(Vec::new()),
            list_state: ListState::new(0, ListAlignment::Top, px(600.)),
            _query_task: None,
            _edit_subscription: None,
            _subscriptions: subscriptions,
        };
        stream.refresh(cx);
        stream
    }

    pub fn filter(&self) -> &MemoFilter {
        &self.filter
    }

    pub fn contains(&self, id: &str) -> bool {
        self.items.iter().any(|i| i == id)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let store = lib.store();
        let ids: Vec<MemoId> = if self.semantic && !self.filter.keywords.trim().is_empty() {
            let base = MemoFilter {
                keywords: String::new(),
                ..self.filter.clone()
            };
            let dense = self
                .query_vector
                .as_ref()
                .filter(|(q, _)| *q == self.filter.keywords)
                .map(|(_, v)| v.as_slice());
            lib.index()
                .search(&self.filter.keywords, dense, 200, |id| {
                    store.get(id).is_some_and(|m| base.matches(m, &[]))
                })
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        } else {
            filter_memos(store, &self.filter, self.sort)
                .into_iter()
                .map(|m| m.id.clone())
                .collect()
        };
        if *self.items == ids {
            self.list_state.remeasure();
        } else {
            self.items = Rc::new(ids);
            self.list_state.reset(self.items.len() + 1);
        }
        cx.notify();
    }

    fn schedule_query_embedding(&mut self, cx: &mut Context<Self>) {
        if !self.semantic {
            return;
        }
        let cfg = Library::global(cx).read(cx).settings().ai.clone();
        let query = self.filter.keywords.trim().to_string();
        if !cfg.uses_remote_embeddings() || query.is_empty() {
            return;
        }
        self._query_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(450)).await;
            let q = query.clone();
            let result = run_blocking(move || ai::embed(&cfg, &[q])).await;
            if let Some(Ok(mut vectors)) = result {
                let _ = this.update(cx, |this, cx| {
                    this.query_vector = Some((query, vectors.remove(0)));
                    this.refresh(cx);
                });
            }
        }));
    }

    pub fn set_tag(&mut self, tag: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_filters(window, cx);
        self.filter.tag = tag.clone();
        self.composer.update(cx, |c, cx| c.set_default_tag(tag, window, cx));
        self.refresh(cx);
    }

    pub fn set_day(&mut self, day: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_filters(window, cx);
        self.filter.day = Some(day);
        self.refresh(cx);
    }

    pub fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selection = None;
        self.filter = MemoFilter::default();
        self.semantic = false;
        self.advanced_error = None;
        for input in [&self.search, &self.date_from, &self.date_to, &self.include_tags, &self.exclude_tags] {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        self.composer.update(cx, |c, cx| c.set_default_tag(None, window, cx));
        self.refresh(cx);
    }

    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| s.focus(window, cx));
    }

    pub fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |c, cx| c.focus(window, cx));
    }

    pub fn annotate(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = format!("{} ", crate::core::parse::ref_token(id));
        self.composer.update(cx, |c, cx| {
            c.set_text(&text, window, cx);
            c.focus(window, cx);
        });
        self.list_state.scroll_to(gpui_kit::ListOffset::default());
        cx.notify();
    }

    pub fn start_edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.contains(id) {
            return false;
        }
        let composer = cx.new(|cx| Composer::new(ComposerMode::Edit(id.to_string()), window, cx));
        composer.update(cx, |c, cx| c.focus(window, cx));
        self._edit_subscription = Some(cx.subscribe_in(&composer, window, |this, _, event, _, cx| {
            if matches!(event, ComposerEvent::Submitted(_) | ComposerEvent::Cancelled) {
                this.editing = None;
                this.list_state.remeasure();
                cx.notify();
            }
        }));
        self.editing = Some((id.to_string(), composer));
        if let Some(ix) = self.items.iter().position(|i| i == id) {
            self.list_state.scroll_to_reveal_item(ix);
        }
        self.list_state.remeasure();
        cx.notify();
        true
    }

    pub fn toggle_expand(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_string());
        }
        self.list_state.remeasure();
        cx.notify();
    }

    pub fn is_selecting(&self) -> bool {
        self.selection.is_some()
    }

    fn start_selecting(&mut self, cx: &mut Context<Self>) {
        self.selection = Some(HashSet::new());
        self.editing = None;
        self.list_state.remeasure();
        cx.notify();
    }

    pub fn stop_selecting(&mut self, cx: &mut Context<Self>) {
        self.selection = None;
        self.list_state.remeasure();
        cx.notify();
    }

    fn toggle_selected(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(selection) = &mut self.selection
            && !selection.remove(id)
        {
            selection.insert(id.to_string());
        }
        cx.notify();
    }

    fn toggle_select_all(&mut self, cx: &mut Context<Self>) {
        if let Some(selection) = &mut self.selection {
            if selection.len() == self.items.len() {
                selection.clear();
            } else {
                *selection = self.items.iter().cloned().collect();
            }
        }
        cx.notify();
    }

    /// Selected ids in list order.
    fn selected_ids(&self) -> Vec<MemoId> {
        let Some(selection) = &self.selection else {
            return Vec::new();
        };
        self.items.iter().filter(|id| selection.contains(*id)).cloned().collect()
    }

    fn trash_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selected_ids();
        if ids.is_empty() {
            return;
        }
        let trashed = ids.clone();
        let result = Library::global(cx).update(cx, |lib, cx| {
            lib.mutate(cx, |s| {
                for id in &trashed {
                    s.trash(id)?;
                }
                Ok(trashed.len())
            })
        });
        self.stop_selecting(cx);
        match result {
            Ok(n) => window.push_notification(
                Notification::new().message(t!("{} 条笔记已移入回收站", n)).action(move |_, _, cx| {
                    let ids = ids.clone();
                    Button::new("undo-batch-trash").primary().xsmall().label(t!("撤销")).on_click(cx.listener(move |this, _, window, cx| {
                        let ids = ids.clone();
                        let _ = Library::global(cx).update(cx, |lib, cx| {
                            lib.mutate(cx, |s| {
                                for id in &ids {
                                    s.restore(id)?;
                                }
                                Ok(())
                            })
                        });
                        this.dismiss(window, cx);
                    }))
                }),
                cx,
            ),
            Err(e) => window.push_notification(Notification::warning(t!("删除失败：{}", e)), cx),
        }
    }

    fn tag_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selected_ids();
        if ids.is_empty() {
            return;
        }
        prompt_text(
            window,
            cx,
            &t!("给 {} 条笔记添加标签", ids.len()),
            t!("标签会追加到每条笔记末尾，用 / 分级，例如 读书/方法。"),
            "",
            t!("添加"),
            move |value, window, cx| {
                let tag: String = value.trim().trim_start_matches('#').chars().filter(|c| !c.is_whitespace()).collect();
                if tag.is_empty() {
                    return Err(t!("请输入标签").into());
                }
                let count = Library::global(cx)
                    .update(cx, |lib, cx| {
                        lib.mutate(cx, |s| {
                            let mut count = 0;
                            for id in &ids {
                                let Some(memo) = s.get(id).cloned() else { continue };
                                if memo.tags.contains(&tag) {
                                    continue;
                                }
                                s.update_memo(id, format!("{} #{tag}", memo.content.trim_end()), memo.images.clone())?;
                                count += 1;
                            }
                            Ok(count)
                        })
                    })
                    .map_err(|e| e.to_string())?;
                window.push_notification(Notification::success(t!("已为 {} 条笔记添加 #{}", count, tag)), cx);
                Ok(())
            },
        );
    }

    fn export_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selected_ids();
        let title = t!("{}（{} 条）", self.export_title(), ids.len());
        export_memos(&ids, &title, window, cx);
    }

    fn export_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.items.as_ref().clone();
        export_memos(&ids, &self.export_title(), window, cx);
    }

    fn export_title(&self) -> String {
        match &self.filter.tag {
            Some(tag) => tag.clone(),
            None => self.title(),
        }
    }

    fn set_source(&mut self, source: Option<String>, cx: &mut Context<Self>) {
        self.filter.source = source;
        self.refresh(cx);
    }

    fn set_quick(&mut self, quick: QuickFilter, cx: &mut Context<Self>) {
        self.filter.quick = if self.filter.quick == Some(quick) { None } else { Some(quick) };
        self.refresh(cx);
    }

    fn set_semantic(&mut self, semantic: bool, cx: &mut Context<Self>) {
        self.semantic = semantic;
        self.schedule_query_embedding(cx);
        self.refresh(cx);
    }

    fn set_sort(&mut self, sort: SortOrder, cx: &mut Context<Self>) {
        self.sort = sort;
        let library = Library::global(cx);
        let _ = library.update(cx, |lib, cx| {
            lib.update_settings(cx, |s| {
                s.sort_by_updated = sort.by_updated();
                s.sort_ascending = matches!(sort, SortOrder::CreatedAsc | SortOrder::UpdatedAsc);
            })
        });
        self.refresh(cx);
    }

    fn apply_advanced(&mut self, cx: &mut Context<Self>) {
        let from = self.date_from.read(cx).value().trim().to_string();
        let to = self.date_to.read(cx).value().trim().to_string();
        self.advanced_error = None;
        let parse = |s: &str| -> Result<Option<NaiveDate>, String> {
            if s.is_empty() {
                Ok(None)
            } else {
                time::parse_date(s)
                    .map(Some)
                    .ok_or_else(|| t!("无法识别日期「{}」，请使用 2024-01-31 格式", s))
            }
        };
        match (parse(&from), parse(&to)) {
            (Ok(f), Ok(t)) => {
                self.filter.date_from = f;
                self.filter.date_to = t;
            }
            (Err(e), _) | (_, Err(e)) => {
                self.advanced_error = Some(e);
                cx.notify();
                return;
            }
        }
        self.filter.include_tags = parse_tag_list(&self.include_tags.read(cx).value());
        self.filter.exclude_tags = parse_tag_list(&self.exclude_tags.read(cx).value());
        self.refresh(cx);
    }

    fn reset_advanced(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [&self.date_from, &self.date_to, &self.include_tags, &self.exclude_tags] {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        self.filter.date_from = None;
        self.filter.date_to = None;
        self.filter.include_tags.clear();
        self.filter.exclude_tags.clear();
        self.filter.quick = None;
        self.advanced_error = None;
        self.refresh(cx);
    }

    fn set_range_preset(&mut self, preset: RangePreset, window: &mut Window, cx: &mut Context<Self>) {
        use chrono::Datelike as _;
        let today = time::today();
        let from = match preset {
            RangePreset::Week => {
                today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64)
            }
            RangePreset::Month => today.with_day(1).unwrap_or(today),
            RangePreset::Year => today - chrono::Duration::days(365),
        };
        self.date_from.update(cx, |s, cx| s.set_value(time::fmt_day(from), window, cx));
        self.date_to.update(cx, |s, cx| s.set_value(time::fmt_day(today), window, cx));
        self.apply_advanced(cx);
    }

    fn title(&self) -> String {
        if let Some(tag) = &self.filter.tag {
            format!("#{tag}")
        } else if let Some(day) = self.filter.day {
            time::fmt_day(day)
        } else if !self.filter.keywords.trim().is_empty() {
            if self.semantic { t!("语义搜索").into() } else { t!("搜索结果").into() }
        } else if let Some(q) = self.filter.quick {
            q.label().into()
        } else {
            t!("全部笔记").into()
        }
    }

    fn shows_composer(&self) -> bool {
        self.selection.is_none()
            && self.filter.source.is_none()
            && self.filter.day.is_none()
            && self.filter.keywords.trim().is_empty()
            && self.filter.quick.is_none()
            && !self.filter.has_advanced()
    }

    /// flomo-style header: ≡ while the sidebar is tucked away, otherwise 🏠 / parent / tag ▾ in tag view.
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let toggle = sidebar_toggle(cx);
        let collapsed = toggle.is_some();
        let slash = move || div().px_0p5().text_color(muted).child("/");
        let mut parents: Vec<(String, String)> = Vec::new();
        if let Some(tag) = &self.filter.tag {
            let parts: Vec<&str> = tag.split('/').collect();
            for i in 0..parts.len().saturating_sub(1) {
                parents.push((parts[..=i].join("/"), parts[i].to_string()));
            }
        }
        h_flex()
            .flex_1()
            .min_w_0()
            .gap_0p5()
            .children(toggle)
            .when(!collapsed && self.filter.tag.is_some(), |this| {
                this.child(
                    Button::new("crumb-home")
                        .ghost()
                        .small()
                        .icon(Icon::new(Lucide::House).text_color(muted))
                        .tooltip(t!("全部笔记"))
                        .on_click(|_, window, cx| dispatch(Command::Navigate(Route::Memos), window, cx)),
                )
                .child(slash())
            })
            .children(parents.into_iter().enumerate().map(move |(i, (path, name))| {
                h_flex()
                    .child(
                        Button::new(("crumb", i))
                            .ghost()
                            .compact()
                            .child(div().text_base().text_color(muted).child(name))
                            .on_click(move |_, window, cx| dispatch(Command::OpenTag(path.clone()), window, cx)),
                    )
                    .child(slash())
            }))
            .child(self.render_title_menu(cx))
    }

    fn render_title_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let sort = self.sort;
        let source = self.filter.source.clone();
        let tag = self.filter.tag.clone();
        let label = match &tag {
            Some(tag) => tag.rsplit('/').next().unwrap_or(tag).to_string(),
            None => self.title(),
        };
        Button::new("title-menu")
            .ghost()
            .compact()
            .child(
                h_flex()
                    .gap_1()
                    .min_w_0()
                    .child(div().text_base().font_semibold().truncate().child(label))
                    .child(Icon::new(Lucide::ChevronDown).xsmall().text_color(cx.theme().muted_foreground)),
            )
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, window, cx| {
                let mut menu = menu.min_w(px(200.));
                if let Some(tag) = &tag {
                    let pinned = Library::global(cx).read(cx).store().tag_meta(tag).pinned;
                    menu = menu
                        .item(menu_item(
                            if pinned { t!("取消置顶") } else { t!("置顶") },
                            if pinned { Lucide::PinOff } else { Lucide::ArrowUpToLine },
                            Command::TogglePinTag(tag.clone()),
                        ))
                        .item(menu_item(t!("编辑标签/图标"), Lucide::Pencil, Command::EditTag(tag.clone())))
                        .separator();
                }
                let sort_view = view.clone();
                menu = menu.submenu_with_icon(Some(Icon::new(Lucide::ArrowUpDown)), t!("排序方式"), window, cx, move |sub, _, _| {
                    SortOrder::ALL.iter().fold(sub, |sub, &order| {
                        let view = sort_view.clone();
                        sub.item(PopupMenuItem::new(order.label()).checked(order == sort).on_click(move |_, _, cx| {
                            let _ = view.update(cx, |this, cx| this.set_sort(order, cx));
                        }))
                    })
                });
                let source_view = view.clone();
                let current_source = source.clone();
                menu = menu.submenu_with_icon(Some(Icon::new(Lucide::Funnel)), t!("笔记来源"), window, cx, move |sub, _, _| {
                    sources().iter().fold(sub, |sub, &(key, name)| {
                        let view = source_view.clone();
                        let checked = current_source.as_deref() == key;
                        sub.item(PopupMenuItem::new(name).checked(checked).on_click(move |_, _, cx| {
                            let _ = view.update(cx, |this, cx| this.set_source(key.map(str::to_string), cx));
                        }))
                    })
                });
                let select_view = view.clone();
                let export_view = view.clone();
                menu = menu
                    .item(PopupMenuItem::new(t!("多选笔记")).icon(Icon::new(Lucide::CircleCheck)).on_click(move |_, _, cx| {
                        let _ = select_view.update(cx, |this, cx| this.start_selecting(cx));
                    }))
                    .item(PopupMenuItem::new(t!("导出")).icon(Icon::new(Lucide::Download)).on_click(move |_, window, cx| {
                        let _ = export_view.update(cx, |this, cx| this.export_view(window, cx));
                    }));
                match &tag {
                    Some(tag) => menu
                        .item(menu_item(t!("AI 洞察此标签"), Lucide::Sparkles, Command::InsightMemos(Vec::new(), format!("#{tag}"))))
                        .separator()
                        .item(danger_menu_item(t!("删除"), Lucide::Trash, Command::DeleteTag(tag.clone()), cx)),
                    None => menu
                        .separator()
                        .item(menu_item(t!("记录统计"), Lucide::ChartNoAxesColumn, Command::Navigate(Route::Stats)))
                        .item(menu_item(t!("回收站"), Lucide::Trash, Command::Navigate(Route::Trash))),
                }
            })
    }

    fn render_search(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let s = surfaces(cx);
        let view = cx.entity().downgrade();
        let inputs = [
            self.date_from.clone(),
            self.date_to.clone(),
            self.include_tags.clone(),
            self.exclude_tags.clone(),
        ];
        let filtering = self.filter.quick.is_some() || self.filter.has_advanced() || self.semantic;
        h_flex()
            .w(px(240.))
            .min_w(px(132.))
            .flex_shrink(1.)
            .h_9()
            .pl_2p5()
            .pr_1()
            .gap_1()
            .rounded_lg()
            .bg(s.card)
            .child(Icon::new(Lucide::Search).small().text_color(theme.muted_foreground))
            .child(div().flex_1().child(Input::new(&self.search).small().appearance(false).cleanable(true)))
            .child(
                Popover::new("filter-popover")
                    .anchor(Anchor::TopRight)
                    .trigger(
                        Button::new("filter-trigger")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::ListFilter).text_color(if filtering { s.accent } else { theme.muted_foreground })),
                    )
                    .content(move |_, _, cx| filter_panel(view.clone(), inputs.clone(), cx)),
            )
    }

    fn render_chips(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let mut chips: Vec<(SharedString, u8)> = Vec::new();
        if let Some(day) = self.filter.day {
            chips.push((time::fmt_day(day).into(), 0));
        }
        if let Some(q) = self.filter.quick {
            chips.push((q.label().into(), 1));
        }
        if self.filter.has_advanced() {
            let mut parts = Vec::new();
            if self.filter.date_from.is_some() || self.filter.date_to.is_some() {
                parts.push(format!(
                    "{} ~ {}",
                    self.filter.date_from.map(time::fmt_day).unwrap_or_default(),
                    self.filter.date_to.map(time::fmt_day).unwrap_or_default()
                ));
            }
            if !self.filter.include_tags.is_empty() {
                parts.push(t!("含 #{}", self.filter.include_tags.join(" #")));
            }
            if !self.filter.exclude_tags.is_empty() {
                parts.push(t!("不含 #{}", self.filter.exclude_tags.join(" #")));
            }
            chips.push((parts.join("，").into(), 2));
        }
        if self.semantic {
            chips.push((t!("语义搜索").into(), 3));
        }
        if let Some(source) = &self.filter.source {
            chips.push((t!("来源：{}", stats::source_label(source)).into(), 4));
        }
        if chips.is_empty() {
            return None;
        }
        Some(
            h_flex()
                .gap_1p5()
                .flex_wrap()
                .children(chips.into_iter().map(|(label, kind)| {
                    Button::new(SharedString::from(format!("chip-{kind}")))
                        .outline()
                        .xsmall()
                        .label(label)
                        .icon(Icon::new(Lucide::X))
                        .on_click(cx.listener(move |this, _, window, cx| match kind {
                            0 => {
                                this.filter.day = None;
                                this.refresh(cx);
                            }
                            1 => {
                                this.filter.quick = None;
                                this.refresh(cx);
                            }
                            2 => {
                                let quick = this.filter.quick;
                                this.reset_advanced(window, cx);
                                this.filter.quick = quick;
                                this.refresh(cx);
                            }
                            3 => this.set_semantic(false, cx),
                            _ => this.set_source(None, cx),
                        }))
                }))
                .child(
                    Button::new("chip-clear")
                        .ghost()
                        .xsmall()
                        .label(t!("清除全部"))
                        .on_click(cx.listener(|this, _, window, cx| this.clear_filters(window, cx))),
                ),
        )
    }

    fn render_item(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let library = Library::global(cx);
        let lib = library.read(cx);
        if ix >= self.items.len() {
            let theme = cx.theme();
            let text = if self.items.is_empty() {
                if self.filter.is_empty() {
                    t!("还没有笔记。写下第一条想法吧：不必完整，不必正确，记下就好。").to_string()
                } else {
                    t!("没有找到匹配的笔记").to_string()
                }
            } else {
                t!("已全部加载笔记").to_string()
            };
            return div()
                .py_6()
                .w_full()
                .flex()
                .justify_center()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(text)
                .into_any_element();
        }
        let id = &self.items[ix];
        if let Some((edit_id, composer)) = &self.editing
            && edit_id == id
        {
            return div().pb_3().child(composer.clone()).into_any_element();
        }
        let Some(memo) = lib.store().get(id) else {
            return div().into_any_element();
        };
        let keywords = if self.semantic { Vec::new() } else { self.filter.keyword_list() };
        let card = memo_card(
            memo,
            CardOptions {
                keywords,
                expanded: self.expanded.contains(id),
                highlighted: self.highlighted.as_ref() == Some(id),
                ..CardOptions::new("stream")
            },
            lib,
            cx,
        );
        let Some(selection) = &self.selection else {
            return div().pb_3().child(card).into_any_element();
        };
        let selected = selection.contains(id);
        let s = surfaces(cx);
        let theme = cx.theme();
        let toggle_id = id.clone();
        // A layer over the card so clicks select instead of opening links, menus or the editor.
        div()
            .pb_3()
            .child(
                div()
                    .relative()
                    .child(card)
                    .child(
                        div()
                            .id(SharedString::from(format!("select-{id}")))
                            .absolute()
                            .inset_0()
                            .rounded_lg()
                            .cursor_pointer()
                            .border_2()
                            .border_color(if selected { s.accent } else { gpui_kit::transparent_black() })
                            .when(selected, |d| d.bg(s.accent.opacity(0.04)))
                            .hover(|d| d.bg(theme.foreground.opacity(0.02)))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_selected(&toggle_id, cx)))
                            .child(
                                div()
                                    .absolute()
                                    .top_3()
                                    .right_3()
                                    .size_5()
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .border_1()
                                    .border_color(if selected { s.accent } else { theme.muted_foreground.opacity(0.5) })
                                    .bg(if selected { s.accent } else { s.card })
                                    .when(selected, |d| d.child(Icon::new(Lucide::Check).xsmall().text_color(gpui_kit::white()))),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_selection_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let selection = self.selection.as_ref()?;
        let theme = cx.theme();
        let s = surfaces(cx);
        let count = selection.len();
        let all = count > 0 && count == self.items.len();
        let none = count == 0;
        Some(
            h_flex()
                .absolute()
                .bottom_6()
                .left_0()
                .right_0()
                .justify_center()
                .child(
                    h_flex()
                        .gap_1()
                        .pl_4()
                        .pr_1p5()
                        .py_1p5()
                        .rounded_xl()
                        .bg(s.card)
                        .border_1()
                        .border_color(theme.border)
                        .shadow_lg()
                        .occlude()
                        .child(div().text_sm().mr_2().child(t!("已选择 {} 条", count)))
                        .child(
                            Button::new("select-all")
                                .ghost()
                                .small()
                                .label(if all { t!("取消全选") } else { t!("全选") })
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_select_all(cx))),
                        )
                        .child(
                            Button::new("select-tag")
                                .ghost()
                                .small()
                                .icon(Icon::new(Lucide::Hash))
                                .label(t!("添加标签"))
                                .disabled(none)
                                .on_click(cx.listener(|this, _, window, cx| this.tag_selected(window, cx))),
                        )
                        .child(
                            Button::new("select-export")
                                .ghost()
                                .small()
                                .icon(Icon::new(Lucide::Download))
                                .label(t!("导出"))
                                .disabled(none)
                                .on_click(cx.listener(|this, _, window, cx| this.export_selected(window, cx))),
                        )
                        .child(
                            Button::new("select-trash")
                                .ghost()
                                .small()
                                .icon(Icon::new(Lucide::Trash).text_color(theme.danger))
                                .label(t!("删除"))
                                .disabled(none)
                                .on_click(cx.listener(|this, _, window, cx| this.trash_selected(window, cx))),
                        )
                        .child(
                            Button::new("select-done")
                                .primary()
                                .small()
                                .label(t!("完成"))
                                .on_click(cx.listener(|this, _, _, cx| this.stop_selecting(cx))),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// Writes the memos to a Markdown file in ~/Downloads/Mori and offers to reveal it.
fn export_memos(ids: &[MemoId], title: &str, window: &mut Window, cx: &mut App) {
    let result = {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let store = lib.store();
        let memos: Vec<&Memo> = ids.iter().filter_map(|id| store.get_active(id)).collect();
        if memos.is_empty() {
            None
        } else {
            Some(share::output_path(title, "md").and_then(|path| {
                io::export_markdown_memos(store, &memos, title, &path).map(|count| (count, path))
            }))
        }
    };
    match result {
        None => window.push_notification(Notification::warning(t!("没有可导出的笔记")), cx),
        Some(Ok((count, path))) => {
            let reveal = path.clone();
            window.push_notification(
                Notification::success(t!("{} 条笔记已导出到 {}", count, path.display()))
                    .title(t!("导出完成"))
                    .action(move |_, _, cx| {
                        let reveal = reveal.clone();
                        Button::new("reveal-export").xsmall().label(t!("在访达中显示")).on_click(cx.listener(move |this, _, window, cx| {
                            cx.reveal_path(&reveal);
                            this.dismiss(window, cx);
                        }))
                    }),
                cx,
            );
        }
        Some(Err(e)) => window.push_notification(Notification::warning(t!("导出失败：{}", e)), cx),
    }
}

fn filter_panel(view: WeakEntity<MemoStream>, inputs: [Entity<InputState>; 4], cx: &App) -> AnyElement {
    let Some(stream) = view.upgrade() else {
        return div().into_any_element();
    };
    let state = stream.read(cx);
    let theme = cx.theme();
    let s = surfaces(cx);
    let quick = state.filter.quick;
    let semantic = state.semantic;
    let error = state.advanced_error.clone();
    let [from, to, include, exclude] = inputs;
    let label = |text: &str| div().text_xs().text_color(theme.muted_foreground).child(text.to_string());
    let presets = [
        (t!("本周"), RangePreset::Week),
        (t!("本月"), RangePreset::Month),
        (t!("近一年"), RangePreset::Year),
    ];
    v_flex()
        .w(px(340.))
        .gap_3()
        .p_1()
        .child(label(t!("快捷筛选")))
        .child(
            h_flex().flex_wrap().gap_1p5().children(QuickFilter::ALL.iter().map(|&q| {
                let view = view.clone();
                let active = quick == Some(q);
                Button::new(SharedString::from(format!("quick-{}", q.label())))
                    .small()
                    .when(active, |b| b.primary())
                    .when(!active, |b| b.outline())
                    .label(q.label())
                    .on_click(move |_, _, cx| {
                        let _ = view.update(cx, |this, cx| this.set_quick(q, cx));
                    })
            })),
        )
        .child(div().h_px().bg(theme.border))
        .child(
            h_flex()
                .child(label(t!("自定范围 · 日期")))
                .child(div().flex_1())
                .children(presets.iter().map(|(name, preset)| {
                    let view = view.clone();
                    let preset = *preset;
                    Button::new(SharedString::from(format!("preset-{name}")))
                        .ghost()
                        .xsmall()
                        .label(*name)
                        .on_click(move |_, window, cx| {
                            let _ = view.update(cx, |this, cx| this.set_range_preset(preset, window, cx));
                        })
                })),
        )
        .child(
            h_flex()
                .gap_2()
                .child(div().flex_1().child(Input::new(&from).small()))
                .child("—")
                .child(div().flex_1().child(Input::new(&to).small())),
        )
        .child(label(t!("标签")))
        .child(Input::new(&include).small())
        .child(Input::new(&exclude).small())
        .when_some(error, |this, e| this.child(div().text_xs().text_color(theme.danger).child(e)))
        .child(div().h_px().bg(theme.border))
        .child(
            h_flex()
                .gap_2()
                .child(
                    Switch::new("semantic-switch").small().checked(semantic).label(t!("语义搜索")).on_change({
                        let view = view.clone();
                        move |checked: &bool, _, cx| {
                            let checked = *checked;
                            let _ = view.update(cx, |this, cx| this.set_semantic(checked, cx));
                        }
                    }),
                )
                .child(div().flex_1().text_xs().text_color(theme.muted_foreground).child(t!("按意思而不是字面匹配")))
                .child(div().size_2().rounded_full().bg(if semantic { s.accent } else { theme.transparent })),
        )
        .child(
            h_flex()
                .gap_2()
                .justify_end()
                .child(Button::new("filter-reset").ghost().small().label(t!("重置")).on_click({
                    let view = view.clone();
                    move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| this.reset_advanced(window, cx));
                    }
                }))
                .child(Button::new("filter-apply").primary().small().label("应用").on_click({
                    let view = view.clone();
                    move |_, _, cx| {
                        let _ = view.update(cx, |this, cx| this.apply_advanced(cx));
                    }
                })),
        )
        .into_any_element()
}

impl Render for MemoStream {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show_composer = self.shows_composer();
        let selection_bar = self.render_selection_bar(cx);
        v_flex()
            .relative()
            .size_full()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(CONTENT_MAX_WIDTH))
                    .mx_auto()
                    .px_6()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(self.render_header(cx))
                            .child(self.render_search(cx)),
                    )
                    .children(self.render_chips(cx))
                    .when(show_composer, |this| this.child(self.composer.clone())),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        list(
                            self.list_state.clone(),
                            cx.processor(|this, ix, window, cx| {
                                v_flex()
                                    .w_full()
                                    .items_center()
                                    .child(
                                        div()
                                            .w_full()
                                            .max_w(px(CONTENT_MAX_WIDTH))
                                            .px_6()
                                            .when(ix == 0, |d| d.pt_3())
                                            .child(this.render_item(ix, window, cx)),
                                    )
                                    .into_any_element()
                            }),
                        )
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(Scrollbar::vertical(&self.list_state).viewport_from_layout()),
                    ),
            )
            .children(selection_bar)
    }
}
