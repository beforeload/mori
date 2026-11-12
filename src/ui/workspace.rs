use std::collections::HashSet;
use std::time::Instant;

use chrono::NaiveDate;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    Anchor, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable, Image,
    ImageFormat, InteractiveElement as _, IntoElement, KeyBinding, MouseButton, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled as _, Subscription, Window, actions, div,
    prelude::FluentBuilder as _, px,
};
use sha2::{Digest, Sha256};

use crate::core::model::Appearance;
use crate::core::{parse, share, time};

use super::chat_page::ChatPage;
use super::composer::{FormatBold, FormatHighlight, FormatUnderline};
use super::detail::MemoDetail;
use super::dialogs::{confirm, prompt_fields, prompt_text};
use super::graph_page::GraphPage;
use super::inbox_page::InboxPage;
use super::insight_page::InsightPage;
use super::library::Library;
use super::map_page::MapPage;
use super::memo_card::menu_item;
use super::memory_page::MemoryPage;
use super::review_page::ReviewPage;
use super::settings_page::{SettingsPage, SettingsSection};
use super::sidebar::{AUTO_COLLAPSE_WIDTH, SIDEBAR_WIDTH, SidebarCollapsed, render_sidebar};
use super::stats_page::StatsPage;
use super::stream::MemoStream;
use super::theme::{self, surfaces};
use super::trash_page::TrashPage;
use super::walk_page::WalkPage;
use super::{Command, Lucide, Route, WorkspaceGlobal, window_pin};

actions!(
    mori,
    [
        FocusSearch,
        NewMemo,
        ToggleSidebar,
        GoHome,
        LockApp,
        OpenSettings,
        CloseDetail,
        ToggleDarkMode,
        Quit
    ]
);

const CONTEXT: &str = "Workspace";
const TOP_BAR: f32 = 40.;
const SHEET_TITLE_BAR: f32 = 48.;
const DETAIL_WIDTH: f32 = 420.;
/// The memo stream keeps at least this much width before the detail panel floats instead.
const MIN_MAIN_WIDTH: f32 = 520.;
const SIDEBAR_ANIMATION_SECS: f32 = 0.18;

fn smoothstep(t: f32) -> f32 {
    t * t * (3. - 2. * t)
}

fn approach(value: &mut f32, target: f32, step: f32) {
    *value = if *value < target { (*value + step).min(target) } else { (*value - step).max(target) };
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-k", FocusSearch, None),
        KeyBinding::new("cmd-n", NewMemo, None),
        KeyBinding::new("cmd-/", NewMemo, None),
        KeyBinding::new("cmd-\\", ToggleSidebar, None),
        KeyBinding::new("cmd-shift-f", GoHome, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-l", LockApp, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("escape", CloseDetail, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-d", ToggleDarkMode, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-b", FormatBold, Some(super::composer::CONTEXT)),
        KeyBinding::new("cmd-shift-h", FormatHighlight, Some(super::composer::CONTEXT)),
        KeyBinding::new("cmd-u", FormatUnderline, Some(super::composer::CONTEXT)),
    ]);
    // Fallbacks for when no element inside the workspace holds focus.
    cx.on_action(|_: &LockApp, cx| with_workspace(cx, |ws, window, cx| ws.lock(window, cx)));
    cx.on_action(|_: &ToggleDarkMode, cx| with_workspace(cx, |ws, window, cx| ws.toggle_dark_mode(window, cx)));
    cx.on_action(|_: &OpenSettings, cx| {
        with_workspace(cx, |ws, window, cx| ws.open_settings(SettingsSection::General, window, cx))
    });
    cx.on_action(|_: &FocusSearch, cx| with_workspace(cx, |ws, window, cx| ws.focus_search(window, cx)));
    cx.on_action(|_: &NewMemo, cx| with_workspace(cx, |ws, window, cx| ws.new_memo(window, cx)));
    cx.on_action(|_: &GoHome, cx| {
        with_workspace(cx, |ws, window, cx| ws.handle(Command::Navigate(Route::Memos), window, cx))
    });
    cx.on_action(|_: &ToggleSidebar, cx| with_workspace(cx, |ws, _, cx| ws.toggle_sidebar(cx)));
    cx.on_action(|_: &CloseDetail, cx| with_workspace(cx, |ws, _, cx| ws.escape(cx)));
}

fn with_workspace(cx: &mut App, f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static) {
    let Some(workspace) = cx.try_global::<WorkspaceGlobal>().and_then(|g| g.0.upgrade()) else {
        return;
    };
    let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) else {
        return;
    };
    let _ = window.update(cx, |_, window, cx| workspace.update(cx, |ws, cx| f(ws, window, cx)));
}

pub fn hash_passcode(passcode: &str, salt: &str) -> String {
    let digest = Sha256::digest(format!("{salt}:{passcode}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A press on empty top-bar space moves the window, a double press zooms it.
fn drag_strip() -> gpui_kit::Div {
    div().on_mouse_down(MouseButton::Left, |event, window, _| {
        if event.click_count == 2 {
            window.titlebar_double_click();
        } else {
            window.start_window_move();
        }
    })
}

pub struct Workspace {
    route: Route,
    previous_route: Route,
    /// The user's choice for wide windows (persisted).
    sidebar_hidden: bool,
    /// Narrow windows only: the sidebar slid in over the content.
    drawer_open: bool,
    narrow: bool,
    /// Animation progress (0 hidden – 1 shown) of the docked sidebar and of the drawer.
    sidebar_t: f32,
    drawer_t: f32,
    anim_tick: Option<Instant>,
    pinned: bool,
    stream: Entity<MemoStream>,
    inbox: Option<Entity<InboxPage>>,
    review: Option<Entity<ReviewPage>>,
    walk: Option<Entity<WalkPage>>,
    map: Option<Entity<MapPage>>,
    graph: Option<Entity<GraphPage>>,
    stats: Option<Entity<StatsPage>>,
    insight: Option<Entity<InsightPage>>,
    chat: Option<Entity<ChatPage>>,
    memory: Option<Entity<MemoryPage>>,
    trash: Option<Entity<TrashPage>>,
    settings: Option<Entity<SettingsPage>>,
    detail: Option<Entity<MemoDetail>>,
    expanded_tags: HashSet<String>,
    locked: bool,
    lock_input: Entity<InputState>,
    lock_error: Option<String>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let handle = cx.entity().downgrade();
        cx.set_global(WorkspaceGlobal(handle));
        let library = Library::global(cx);
        theme::apply(&library.read(cx).settings().clone(), window, cx);
        let stream = cx.new(|cx| MemoStream::new(window, cx));
        let lock_input = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(t!("输入密码解锁")));
        let locked = library.read(cx).settings().lock_enabled;
        let sidebar_hidden = library.read(cx).settings().sidebar_hidden;
        let narrow = window.viewport_size().width < px(AUTO_COLLAPSE_WIDTH);
        let subscriptions = vec![
            cx.observe(&library, |_, _, cx| cx.notify()),
            // Keeps workspace shortcuts (Esc, ⌘K…) alive after a focused child such as an inline
            // composer is removed and leaves nothing focused.
            cx.on_focus_lost(window, |this, window, cx| this.focus_handle.focus(window, cx)),
            cx.observe_window_appearance(window, |_, window, cx| {
                let settings = Library::global(cx).read(cx).settings().clone();
                if settings.appearance == Appearance::System {
                    theme::apply(&settings, window, cx);
                }
            }),
            cx.subscribe_in(&lock_input, window, |this, _, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.try_unlock(window, cx);
                }
            }),
        ];
        let focus_handle = cx.focus_handle();
        if locked {
            lock_input.update(cx, |s, cx| s.focus(window, cx));
        } else {
            stream.update(cx, |s, cx| s.focus_composer(window, cx));
        }
        Self {
            route: Route::Memos,
            previous_route: Route::Memos,
            sidebar_hidden,
            drawer_open: false,
            narrow,
            sidebar_t: if narrow || sidebar_hidden { 0. } else { 1. },
            drawer_t: 0.,
            anim_tick: None,
            pinned: false,
            stream,
            inbox: None,
            review: None,
            walk: None,
            map: None,
            graph: None,
            stats: None,
            insight: None,
            chat: None,
            memory: None,
            trash: None,
            settings: None,
            detail: None,
            expanded_tags: HashSet::new(),
            locked,
            lock_input,
            lock_error: None,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }

    pub fn route(&self) -> Route {
        self.route
    }

    /// Wide windows dock or hide the sidebar (remembered); narrow ones slide it in as a drawer.
    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.narrow {
            self.drawer_open = !self.drawer_open;
        } else {
            self.sidebar_hidden = !self.sidebar_hidden;
            let hidden = self.sidebar_hidden;
            let _ = Library::global(cx).update(cx, |lib, cx| lib.update_settings(cx, |s| s.sidebar_hidden = hidden));
        }
        cx.notify();
    }

    fn sidebar_docked(&self) -> bool {
        !self.narrow && !self.sidebar_hidden
    }

    fn step_sidebar_animation(&mut self, window: &mut Window) {
        let docked = if self.sidebar_docked() { 1. } else { 0. };
        let drawer = if self.narrow && self.drawer_open { 1. } else { 0. };
        if self.sidebar_t == docked && self.drawer_t == drawer {
            self.anim_tick = None;
            return;
        }
        let now = Instant::now();
        let dt = self.anim_tick.map(|t| (now - t).as_secs_f32()).unwrap_or(0.).min(0.05);
        self.anim_tick = Some(now);
        let step = dt / SIDEBAR_ANIMATION_SECS;
        approach(&mut self.sidebar_t, docked, step);
        approach(&mut self.drawer_t, drawer, step);
        window.request_animation_frame();
    }

    pub fn expanded_tags(&self) -> &HashSet<String> {
        &self.expanded_tags
    }

    pub fn selected_tag(&self, cx: &App) -> Option<String> {
        self.stream.read(cx).filter().tag.clone()
    }

    pub fn selected_day(&self, cx: &App) -> Option<NaiveDate> {
        self.stream.read(cx).filter().day
    }

    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(Route::Memos, window, cx);
        self.stream.update(cx, |s, cx| s.focus_search(window, cx));
    }

    pub fn new_memo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(Route::Memos, window, cx);
        self.stream.update(cx, |s, cx| {
            if !s.filter().is_empty() {
                s.clear_filters(window, cx);
            }
            s.focus_composer(window, cx)
        });
    }

    pub fn navigate(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        if route != self.route {
            self.focus_handle.focus(window, cx);
            if !self.route.is_overlay() {
                self.previous_route = self.route;
            }
        }
        self.route = route;
        match route {
            Route::Memos => {}
            Route::Inbox if self.inbox.is_none() => self.inbox = Some(cx.new(|cx| InboxPage::new(window, cx))),
            Route::Review if self.review.is_none() => self.review = Some(cx.new(|cx| ReviewPage::new(window, cx))),
            Route::Walk if self.walk.is_none() => self.walk = Some(cx.new(|cx| WalkPage::new(window, cx))),
            Route::Map if self.map.is_none() => self.map = Some(cx.new(|cx| MapPage::new(window, cx))),
            Route::Graph if self.graph.is_none() => self.graph = Some(cx.new(|cx| GraphPage::new(window, cx))),
            Route::Stats if self.stats.is_none() => self.stats = Some(cx.new(|cx| StatsPage::new(window, cx))),
            Route::Insight if self.insight.is_none() => self.insight = Some(cx.new(|cx| InsightPage::new(window, cx))),
            Route::Memory if self.memory.is_none() => self.memory = Some(cx.new(|cx| MemoryPage::new(window, cx))),
            Route::Trash if self.trash.is_none() => self.trash = Some(cx.new(|cx| TrashPage::new(window, cx))),
            _ => {}
        }
        cx.notify();
    }

    fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.route.is_overlay() {
            let back = self.previous_route;
            self.navigate(back, window, cx);
        }
    }

    fn escape(&mut self, cx: &mut Context<Self>) {
        if self.drawer_open {
            self.drawer_open = false;
        } else if self.detail.is_some() {
            self.detail = None;
        } else if self.route == Route::Memos && self.stream.read(cx).is_selecting() {
            self.stream.update(cx, |s, cx| s.stop_selecting(cx));
        } else if self.route.is_overlay() {
            self.route = self.previous_route;
        }
        cx.notify();
    }

    pub fn open_settings(&mut self, section: SettingsSection, window: &mut Window, cx: &mut Context<Self>) {
        let settings = match &self.settings {
            Some(s) => s.clone(),
            None => {
                let s = cx.new(|cx| SettingsPage::new(window, cx));
                self.settings = Some(s.clone());
                s
            }
        };
        settings.update(cx, |s, cx| s.set_section(section, cx));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .w(px(860.))
                .h(px(620.))
                .p_0()
                .child(settings.clone())
        });
    }

    fn toggle_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_sheet(cx) {
            window.close_sheet(cx);
            return;
        }
        let chat = match &self.chat {
            Some(c) => c.clone(),
            None => {
                let c = cx.new(|cx| ChatPage::new(window, cx));
                self.chat = Some(c.clone());
                c
            }
        };
        chat.read(cx).reveal_latest();
        window.open_sheet(cx, move |sheet, window, cx| {
            // The sheet body scrolls, so the chat needs a definite height to keep its input pinned.
            let body = window.viewport_size().height - cx.theme().sheet.margin_top - px(SHEET_TITLE_BAR);
            sheet
                .title(t!("问问笔记 · AI Agent"))
                .size(px(460.))
                .p_0()
                .child(div().h(body).min_h_0().overflow_hidden().child(chat.clone()))
        });
    }

    fn insight_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InsightPage> {
        self.navigate(Route::Insight, window, cx);
        self.insight.clone().unwrap()
    }

    fn open_detail(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<MemoDetail> {
        match &self.detail {
            Some(detail) => {
                detail.update(cx, |d, cx| d.set_memo(id, window, cx));
                detail.clone()
            }
            None => {
                let id = id.to_string();
                let detail = cx.new(|cx| MemoDetail::new(&id, window, cx));
                self.detail = Some(detail.clone());
                cx.notify();
                detail
            }
        }
    }

    pub fn close_detail(&mut self, cx: &mut Context<Self>) {
        self.detail = None;
        cx.notify();
    }

    pub fn toggle_dark_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        let next = if cx.theme().is_dark() { Appearance::Light } else { Appearance::Dark };
        let _ = library.update(cx, |lib, cx| lib.update_settings(cx, |s| s.appearance = next));
        theme::apply(&library.read(cx).settings().clone(), window, cx);
        cx.notify();
    }

    fn toggle_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = !self.pinned;
        if window_pin::set_pinned(window, next) {
            self.pinned = next;
            window.push_notification(if next { t!("窗口已置顶") } else { t!("已取消置顶") }, cx);
        }
        cx.notify();
    }

    pub fn lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        if !library.read(cx).settings().lock_enabled {
            window.push_notification(Notification::info(t!("请先在「设置 → 账号」中设置密码锁")).title(t!("尚未设置密码")), cx);
            self.open_settings(SettingsSection::Account, window, cx);
            return;
        }
        window.close_all_dialogs(cx);
        self.locked = true;
        self.lock_error = None;
        self.lock_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn try_unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.lock_input.read(cx).value().to_string();
        let settings = Library::global(cx).read(cx).settings().clone();
        if hash_passcode(&value, &settings.lock_salt) == settings.lock_hash {
            self.locked = false;
            self.lock_error = None;
            self.lock_input.update(cx, |s, cx| s.set_value("", window, cx));
            self.stream.update(cx, |s, cx| s.focus_composer(window, cx));
        } else {
            self.lock_error = Some(t!("密码不正确").into());
        }
        cx.notify();
    }

    fn copy(&self, text: String, message: &str, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        window.push_notification(Notification::success(message.to_string()), cx);
    }

    fn share_image(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        let lib = library.read(cx);
        let Some(memo) = lib.store().get(id).cloned() else {
            return;
        };
        let nickname = lib.settings().nickname.clone();
        let dark = cx.theme().is_dark();
        let svg = share::memo_share_svg(&memo, &nickname, dark, &|rid| lib.ref_label(rid));
        match share::render_png(&svg, 2.0).and_then(|png| share::save_png(&png, t!("笔记")).map(|p| (png, p))) {
            Ok((png, path)) => {
                cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, png)));
                let reveal = path.clone();
                window.push_notification(
                    Notification::success(t!("已保存到 {}，并复制到剪贴板", path.display()))
                        .title(t!("分享图已生成"))
                        .action(move |_, _, cx| {
                            let reveal = reveal.clone();
                            Button::new("reveal-memo-share").xsmall().label(t!("在访达中显示")).on_click(cx.listener(move |this, _, window, cx| {
                                cx.reveal_path(&reveal);
                                this.dismiss(window, cx);
                            }))
                        }),
                    cx,
                );
            }
            Err(e) => window.push_notification(Notification::warning(t!("生成分享图失败：{}", e)), cx),
        }
    }

    pub fn handle(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let library = Library::global(cx);
        if self.drawer_open
            && matches!(
                command,
                Command::Navigate(_) | Command::OpenTag(_) | Command::OpenDay(_) | Command::OpenSettings(_) | Command::ToggleAgent
            )
        {
            self.drawer_open = false;
        }
        match command {
            Command::ToggleSidebar => self.toggle_sidebar(cx),
            Command::Refresh => match library.update(cx, |lib, cx| lib.sync_now(cx)) {
                Ok(true) => window.push_notification(Notification::success(t!("已读取 MCP / API 等外部写入的最新内容")), cx),
                Ok(false) => window.push_notification(Notification::info(t!("已是最新")), cx),
                Err(e) => window.push_notification(Notification::warning(t!("刷新失败：{}", e)), cx),
            },
            Command::Navigate(route) => {
                if route == Route::Memos && self.route == Route::Memos {
                    self.stream.update(cx, |s, cx| s.clear_filters(window, cx));
                }
                self.navigate(route, window, cx);
            }
            Command::OpenSettings(section) => self.open_settings(section, window, cx),
            Command::ToggleAgent => self.toggle_agent(window, cx),
            Command::CloseOverlay => self.close_overlay(window, cx),
            Command::OpenTag(tag) => {
                self.stream.update(cx, |s, cx| s.set_tag(Some(tag.clone()), window, cx));
                let mut prefix = String::new();
                for part in tag.split('/') {
                    if !prefix.is_empty() {
                        self.expanded_tags.insert(prefix.clone());
                        prefix.push('/');
                    }
                    prefix.push_str(part);
                }
                self.navigate(Route::Memos, window, cx);
            }
            Command::OpenDay(day) => {
                self.stream.update(cx, |s, cx| s.set_day(day, window, cx));
                self.navigate(Route::Memos, window, cx);
            }
            Command::OpenMemo(id) => {
                if library.read(cx).store().get(&id).is_none() {
                    window.push_notification(Notification::warning(t!("这条笔记已不存在")), cx);
                    return;
                }
                if window.has_active_sheet(cx) {
                    window.close_sheet(cx);
                }
                self.open_detail(&id, window, cx);
            }
            Command::OpenUrl(url) => cx.open_url(&url),
            Command::Annotate(id) => {
                if window.has_active_sheet(cx) {
                    window.close_sheet(cx);
                }
                if self.route.is_overlay() {
                    let detail = self.open_detail(&id, window, cx);
                    detail.update(cx, |d, cx| d.start_annotate(window, cx));
                    return;
                }
                self.navigate(Route::Memos, window, cx);
                self.stream.update(cx, |s, cx| {
                    if !s.filter().is_empty() {
                        s.clear_filters(window, cx);
                    }
                    s.annotate(&id, window, cx)
                });
            }
            Command::Edit(id) => {
                let in_stream = self.route == Route::Memos && self.stream.update(cx, |s, cx| s.start_edit(&id, window, cx));
                if !in_stream {
                    let detail = self.open_detail(&id, window, cx);
                    detail.update(cx, |d, cx| d.start_edit(window, cx));
                }
            }
            Command::Expand(id) => self.stream.update(cx, |s, cx| s.toggle_expand(&id, cx)),
            Command::TogglePin(id) => {
                let pinned = library.read(cx).store().get(&id).is_some_and(|m| m.pinned);
                let _ = library.update(cx, |lib, cx| lib.mutate(cx, |s| s.set_pinned(&id, !pinned)));
            }
            Command::CopyText(id) => {
                if let Some(memo) = library.read(cx).store().get(&id) {
                    let text = memo.content.clone();
                    self.copy(text, t!("已复制笔记内容"), window, cx);
                }
            }
            Command::CopyLink(id) => {
                self.copy(parse::ref_token(&id), t!("已复制引用链接，粘贴到任意笔记即可建立引用"), window, cx);
            }
            Command::ShareImage(id) => self.share_image(&id, window, cx),
            Command::EditTime(id) => {
                let Some(memo) = library.read(cx).store().get(&id).cloned() else {
                    return;
                };
                prompt_text(window, cx, t!("修改笔记时间"), t!("格式：2024-05-01 21:30:00"), &time::fmt_datetime(memo.created_at), t!("保存"), move |value, _, cx| {
                    let ts = time::parse_datetime(&value).ok_or(t!("无法识别这个时间"))?;
                    Library::global(cx)
                        .update(cx, |lib, cx| lib.mutate(cx, |s| s.set_created_at(&id, ts)))
                        .map_err(|e| e.to_string())
                });
            }
            Command::InsightMemo(id) => {
                let page = self.insight_page(window, cx);
                page.update(cx, |p, cx| p.set_memos(vec![id], t!("单条笔记").into(), window, cx));
            }
            Command::InsightMemos(ids, label) => {
                let page = self.insight_page(window, cx);
                if ids.is_empty() && label.starts_with('#') {
                    let tag = label.trim_start_matches('#').to_string();
                    page.update(cx, |p, cx| p.set_tag(&tag, window, cx));
                } else {
                    page.update(cx, |p, cx| p.set_memos(ids, label, window, cx));
                }
            }
            Command::WalkFrom(id) => {
                self.navigate(Route::Walk, window, cx);
                if let Some(walk) = &self.walk {
                    walk.update(cx, |w, cx| w.start_from(&id, cx));
                }
            }
            Command::History(id) => {
                let detail = self.open_detail(&id, window, cx);
                detail.update(cx, |d, cx| d.show_history(cx));
            }
            Command::Trash(id) => {
                if library.update(cx, |lib, cx| lib.mutate(cx, |s| s.trash(&id))).is_ok() {
                    if self.detail.as_ref().is_some_and(|d| d.read(cx).memo_id() == id) {
                        self.detail = None;
                    }
                    let restore_id = id.clone();
                    window.push_notification(
                        Notification::new().message(t!("已移入回收站")).action(move |_, _, cx| {
                            let restore_id = restore_id.clone();
                            Button::new("undo-trash").primary().xsmall().label(t!("撤销")).on_click(cx.listener(move |this, _, window, cx| {
                                let id = restore_id.clone();
                                let _ = Library::global(cx).update(cx, |lib, cx| lib.mutate(cx, |s| s.restore(&id)));
                                this.dismiss(window, cx);
                            }))
                        }),
                        cx,
                    );
                }
            }
            Command::Restore(id) => {
                let _ = library.update(cx, |lib, cx| lib.mutate(cx, |s| s.restore(&id)));
                window.push_notification(Notification::success(t!("已恢复笔记")), cx);
            }
            Command::Purge(id) => {
                confirm(window, cx, t!("彻底删除这条笔记？"), t!("删除后无法恢复，历史版本也会一并删除。"), t!("彻底删除"), true, move |_, cx| {
                    let id = id.clone();
                    let _ = Library::global(cx).update(cx, |lib, cx| lib.mutate(cx, |s| s.purge(&id)));
                });
            }
            Command::EditTag(tag) => {
                let icon = library.read(cx).store().tag_meta(&tag).icon.unwrap_or_default();
                let old = tag.clone();
                prompt_fields(
                    window,
                    cx,
                    &t!("编辑 #{}", tag),
                    t!("改名时子标签会一起改；输入 父标签/新名称 可移动到其他标签下。图标填一个 Emoji，留空恢复默认。"),
                    &[(t!("名称"), &tag, t!("标签名称")), (t!("图标"), &icon, t!("例如 📚"))],
                    t!("保存"),
                    move |values, window, cx| {
                        let new = values[0].trim_start_matches('#').trim_matches('/').to_string();
                        if new.is_empty() {
                            return Err(t!("标签名称不能为空").into());
                        }
                        let icon: String = values[1].chars().take(2).collect();
                        let library = Library::global(cx);
                        if new != old {
                            let count = library
                                .update(cx, |lib, cx| lib.mutate(cx, |s| s.rename_tag(&old, &new)))
                                .map_err(|e| e.to_string())?;
                            window.push_notification(Notification::success(t!("已更新 {} 条笔记", count)), cx);
                        }
                        library
                            .update(cx, |lib, cx| lib.mutate(cx, |s| s.set_tag_icon(&new, Some(icon))))
                            .map_err(|e| e.to_string())?;
                        if new != old {
                            super::dispatch(Command::OpenTag(new), window, cx);
                        }
                        Ok(())
                    },
                );
            }
            Command::DeleteTag(tag) => self.confirm_delete_tag(tag, window, cx),
            Command::TogglePinTag(tag) => {
                let pinned = library.read(cx).store().tag_meta(&tag).pinned;
                let _ = library.update(cx, |lib, cx| lib.mutate(cx, |s| s.set_tag_pinned(&tag, !pinned)));
            }
            Command::ToggleTagExpanded(tag) => {
                if !self.expanded_tags.remove(&tag) {
                    self.expanded_tags.insert(tag);
                }
                cx.notify();
            }
        }
    }

    fn confirm_delete_tag(&mut self, tag: String, window: &mut Window, cx: &mut Context<Self>) {
        let count = Library::global(cx).read(cx).tag_counts().get(&tag).copied().unwrap_or(0);
        let remove_tag = tag.clone();
        let trash_tag = tag.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let remove_tag = remove_tag.clone();
            let trash_tag = trash_tag.clone();
            dialog
                .title(t!("删除标签 #{}", tag))
                .w(px(460.))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child(t!("共有 {} 条笔记使用此标签（含子标签）。", count)))
                .footer(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(Button::new("tag-cancel").outline().label(t!("取消")).on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(Button::new("tag-remove-only").label(t!("仅移除标签")).on_click(move |_, window, cx| {
                            let tag = remove_tag.clone();
                            let result = Library::global(cx).update(cx, |lib, cx| lib.mutate(cx, |s| s.remove_tag(&tag)));
                            window.close_dialog(cx);
                            if let Ok(n) = result {
                                window.push_notification(Notification::success(t!("已从 {} 条笔记中移除标签", n)), cx);
                            }
                            super::dispatch(Command::Navigate(Route::Memos), window, cx);
                        }))
                        .child(Button::new("tag-delete-all").danger().label(t!("删除标签和笔记")).on_click(move |_, window, cx| {
                            let tag = trash_tag.clone();
                            let result = Library::global(cx).update(cx, |lib, cx| lib.mutate(cx, |s| s.trash_tag_memos(&tag)));
                            window.close_dialog(cx);
                            if let Ok(n) = result {
                                window.push_notification(Notification::success(t!("{} 条笔记已移入回收站", n)), cx);
                            }
                            super::dispatch(Command::Navigate(Route::Memos), window, cx);
                        })),
                )
        });
    }

    fn render_lock(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .bg(surfaces(cx).page)
            .child(drag_strip().absolute().top_0().left_0().right_0().h(px(TOP_BAR)))
            .child(Icon::new(Lucide::Lock).large().text_color(theme.muted_foreground))
            .child(div().text_lg().font_semibold().child(t!("Mori 已锁定")))
            .child(div().w(px(260.)).child(Input::new(&self.lock_input).mask_toggle()))
            .when_some(self.lock_error.clone(), |this, e| this.child(div().text_sm().text_color(theme.danger).child(e)))
            .child(Button::new("unlock").primary().label(t!("解锁")).on_click(cx.listener(|this, _, window, cx| this.try_unlock(window, cx))))
    }

    fn render_window_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .gap_1()
            .child(
                Button::new("pin-window")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(if self.pinned { Lucide::PinOff } else { Lucide::Pin }).text_color(if self.pinned { surfaces(cx).accent } else { muted }))
                    .selected(self.pinned)
                    .tooltip(if self.pinned { t!("取消窗口置顶") } else { t!("窗口置顶") })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_pin(window, cx))),
            )
            .child(
                Button::new("more-menu")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::Ellipsis).text_color(muted))
                    .dropdown_menu_with_anchor(Anchor::TopRight, |menu, _, _| {
                        menu.item(menu_item(t!("问问笔记（AI Agent）"), Lucide::BotMessageSquare, Command::ToggleAgent))
                            .item(menu_item(t!("认知地图"), Lucide::Mountain, Command::Navigate(Route::Map)))
                            .item(menu_item(t!("笔记关系图"), Lucide::Waypoints, Command::Navigate(Route::Graph)))
                            .item(menu_item(t!("记录统计"), Lucide::ChartNoAxesColumn, Command::Navigate(Route::Stats)))
                            .item(menu_item(t!("AI 记忆档案"), Lucide::Brain, Command::Navigate(Route::Memory)))
                            .separator()
                            .item(menu_item(t!("导入导出"), Lucide::ArrowDownUp, Command::OpenSettings(SettingsSection::ImportExport)))
                            .item(menu_item(t!("回收站"), Lucide::Trash, Command::Navigate(Route::Trash)))
                            .item(menu_item(t!("设置"), Lucide::Settings, Command::OpenSettings(SettingsSection::General)))
                    }),
            )
    }

    fn render_page(&self) -> gpui_kit::AnyElement {
        use gpui_kit::IntoElement as _;
        match self.route {
            Route::Memos => Some(self.stream.clone().into_any_element()),
            Route::Inbox => self.inbox.clone().map(|e| e.into_any_element()),
            Route::Review => self.review.clone().map(|e| e.into_any_element()),
            Route::Walk => self.walk.clone().map(|e| e.into_any_element()),
            Route::Map => self.map.clone().map(|e| e.into_any_element()),
            Route::Graph => self.graph.clone().map(|e| e.into_any_element()),
            Route::Stats => self.stats.clone().map(|e| e.into_any_element()),
            Route::Insight => self.insight.clone().map(|e| e.into_any_element()),
            Route::Memory => self.memory.clone().map(|e| e.into_any_element()),
            Route::Trash => self.trash.clone().map(|e| e.into_any_element()),
        }
        .unwrap_or_else(|| div().into_any_element())
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let narrow = viewport.width < px(AUTO_COLLAPSE_WIDTH);
        if narrow != self.narrow {
            self.narrow = narrow;
            self.drawer_open = false;
        }
        self.step_sidebar_animation(window);
        let collapsed = !self.sidebar_docked();
        if cx.try_global::<SidebarCollapsed>().map(|c| c.0) != Some(collapsed) {
            cx.set_global(SidebarCollapsed(collapsed));
        }

        let theme = cx.theme().clone();
        let s = surfaces(cx);
        let root = div()
            .relative()
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .text_color(theme.foreground)
            .bg(s.page)
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| this.focus_search(window, cx)))
            .on_action(cx.listener(|this, _: &NewMemo, window, cx| this.new_memo(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &GoHome, window, cx| this.handle(Command::Navigate(Route::Memos), window, cx)))
            .on_action(cx.listener(|this, _: &LockApp, window, cx| this.lock(window, cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(SettingsSection::General, window, cx)))
            .on_action(cx.listener(|this, _: &CloseDetail, _, cx| this.escape(cx)))
            .on_action(cx.listener(|this, _: &ToggleDarkMode, window, cx| this.toggle_dark_mode(window, cx)));

        if self.locked {
            return root.child(self.render_lock(cx)).into_any_element();
        }
        if self.route.is_overlay() {
            return root
                .child(self.render_page())
                .child(drag_strip().absolute().top_0().left_0().w(px(80.)).h(px(TOP_BAR)))
                .children(self.detail.clone().map(|detail| floating_detail(detail, viewport.width.as_f32(), 64., cx)))
                .into_any_element();
        }

        let sidebar_px = SIDEBAR_WIDTH * smoothstep(self.sidebar_t);
        let drawer = smoothstep(self.drawer_t);
        let dock_detail = viewport.width.as_f32() - sidebar_px - DETAIL_WIDTH >= MIN_MAIN_WIDTH;
        let show_agent = Library::global(cx).read(cx).settings().show_agent_button;
        root.child(
            h_flex()
                .size_full()
                .items_stretch()
                .when(sidebar_px > 0.5, |this| {
                    // The sidebar keeps its width and slides out to the left as the slot narrows.
                    this.child(
                        div()
                            .relative()
                            .h_full()
                            .flex_shrink_0()
                            .w(px(sidebar_px))
                            .overflow_hidden()
                            .child(div().absolute().top_0().bottom_0().right_0().child(render_sidebar(self, cx))),
                    )
                })
                .child(
                    v_flex()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(
                            h_flex()
                                .h(px(TOP_BAR))
                                .flex_shrink_0()
                                .pr_3()
                                .child(drag_strip().flex_1().h_full().pl(px((80. - sidebar_px).max(0.))))
                                .child(self.render_window_actions(cx)),
                        )
                        .child(div().flex_1().min_h_0().child(self.render_page()))
                        .when(show_agent, |this| {
                            this.child(
                                div()
                                    .id("agent-fab")
                                    .absolute()
                                    .bottom_5()
                                    .right_5()
                                    .size_11()
                                    .rounded_full()
                                    .bg(s.card)
                                    .border_1()
                                    .border_color(theme.border)
                                    .shadow_lg()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|d| d.bg(s.accent_soft))
                                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(t!("问问笔记 · AI Agent")).build(window, cx))
                                    .child(Icon::new(Lucide::MessageCirclePlus).text_color(s.accent))
                                    .on_click(cx.listener(|this, _, window, cx| this.toggle_agent(window, cx))),
                            )
                        }),
                )
                .when_some(self.detail.clone().filter(|_| dock_detail), |this, detail| {
                    this.child(
                        div()
                            .w(px(DETAIL_WIDTH))
                            .flex_shrink_0()
                            .h_full()
                            .pt(px(TOP_BAR))
                            .border_l_1()
                            .border_color(theme.border)
                            .bg(s.card)
                            .child(detail),
                    )
                }),
        )
        .children(
            self.detail
                .clone()
                .filter(|_| !dock_detail)
                .map(|detail| floating_detail(detail, viewport.width.as_f32(), 12., cx)),
        )
        .when(drawer > 0.001, |this| {
            this.child(
                div()
                    .id("sidebar-backdrop")
                    .absolute()
                    .inset_0()
                    .bg(gpui_kit::black().opacity(0.18 * drawer))
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.drawer_open = false;
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .id("sidebar-drawer")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-(1. - drawer) * SIDEBAR_WIDTH))
                    .w(px(SIDEBAR_WIDTH))
                    .shadow_2xl()
                    .occlude()
                    .child(render_sidebar(self, cx)),
            )
        })
        .into_any_element()
    }
}

/// The memo detail as a panel over the content, for full-window overlays and narrow windows.
fn floating_detail(detail: Entity<MemoDetail>, viewport_width: f32, bottom: f32, cx: &App) -> gpui_kit::AnyElement {
    let theme = cx.theme();
    let s = surfaces(cx);
    div()
        .id("floating-detail")
        .absolute()
        .top(px(TOP_BAR + 8.))
        .right_3()
        .bottom(px(bottom))
        .w(px((viewport_width - 24.).min(DETAIL_WIDTH)))
        .rounded_xl()
        .overflow_hidden()
        .border_1()
        .border_color(theme.border)
        .bg(s.card)
        .shadow_2xl()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_scroll(|_, _, cx| cx.stop_propagation())
        .child(detail)
        .into_any_element()
}
