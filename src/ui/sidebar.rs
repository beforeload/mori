use std::collections::{HashMap, HashSet};

use chrono::{Datelike, NaiveDate};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    Anchor, AnyElement, App, Context, Global, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div,
    prelude::FluentBuilder as _, px, rems,
};

use crate::core::stats::{self, DayStat};
use crate::core::tags::TagNode;
use crate::core::time;

use super::library::Library;
use super::memo_card::{danger_menu_item, menu_item};
use super::settings_page::SettingsSection;
use super::theme::surfaces;
use super::workspace::{ToggleSidebar, Workspace};
use super::{Command, Lucide, Route, dispatch};

pub const SIDEBAR_WIDTH: f32 = 244.;
/// Narrower windows stop docking the sidebar; ≡ then opens it as a drawer over the content.
pub const AUTO_COLLAPSE_WIDTH: f32 = 820.;

/// Set by the workspace each frame: true while the sidebar is not docked, so page
/// headers show the ≡ button that brings it back.
pub struct SidebarCollapsed(pub bool);

impl Global for SidebarCollapsed {}

/// The ≡ button for page headers; `None` while the sidebar is docked.
pub fn sidebar_toggle(cx: &App) -> Option<AnyElement> {
    let collapsed = cx.try_global::<SidebarCollapsed>().is_some_and(|c| c.0);
    collapsed.then(|| {
        Button::new("show-sidebar")
            .ghost()
            .small()
            .icon(Icon::new(Lucide::Menu).text_color(cx.theme().muted_foreground))
            .tooltip_with_action(t!("显示侧边栏"), &ToggleSidebar, None)
            .on_click(|_, window, cx| dispatch(Command::ToggleSidebar, window, cx))
            .into_any_element()
    })
}

const HEATMAP_WEEKS: i64 = 12;
fn months() -> [&'static str; 12] {
    [t!("一月"), t!("二月"), t!("三月"), t!("四月"), t!("五月"), t!("六月"), t!("七月"), t!("八月"), t!("九月"), t!("十月"), t!("十一月"), t!("十二月")]
}

fn heatmap(days: &HashMap<NaiveDate, DayStat>, selected: Option<NaiveDate>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let s = surfaces(cx);
    let today = time::today();
    let last_week = stats::week_start(today);
    let first_week = last_week - chrono::Duration::weeks(HEATMAP_WEEKS - 1);
    let max = (0..HEATMAP_WEEKS * 7)
        .filter_map(|i| days.get(&(first_week + chrono::Duration::days(i))))
        .map(|d| d.count)
        .max()
        .unwrap_or(0);
    let cell = rems(0.85);
    let weeks: Vec<NaiveDate> = (0..HEATMAP_WEEKS).map(|w| first_week + chrono::Duration::weeks(w)).collect();
    let labels: Vec<Option<&str>> = weeks
        .iter()
        .enumerate()
        .map(|(i, week)| {
            let month_of = |d: NaiveDate| (d + chrono::Duration::days(6)).month0();
            let m = month_of(*week);
            let starts = i == 0 || month_of(weeks[i - 1]) != m;
            // Skip a label squeezed against the left edge by a month that barely started.
            (starts && !(i == 0 && month_of(weeks[1.min(weeks.len() - 1)]) != m)).then(|| months()[m as usize])
        })
        .collect();
    v_flex()
        .gap_2()
        .child(
            h_flex().justify_between().children(weeks.iter().map(|week| {
                let week = *week;
                v_flex().gap(rems(0.4)).children((0..7).map(move |d| {
                    let date = week + chrono::Duration::days(d);
                    let count = days.get(&date).map(|s| s.count).unwrap_or(0);
                    let level = stats::heat_level(count, max);
                    let future = date > today;
                    let tip = t!("{} · {} 条笔记", time::fmt_day(date), count);
                    div()
                        .id(SharedString::from(format!("heat-{date}")))
                        .size(cell)
                        .rounded(px(3.))
                        .bg(if future { theme.transparent } else { s.heat[level] })
                        .when(date == today, |c| c.border_1().border_color(theme.muted_foreground))
                        .when(selected == Some(date), |c| c.border_1().border_color(s.accent))
                        .when(!future, |c| c.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)))
                        .when(count > 0, |c| {
                            c.cursor_pointer()
                                .on_click(move |_, window, cx| dispatch(Command::OpenDay(date), window, cx))
                        })
                }))
            })),
        )
        .child(
            h_flex().justify_between().children(labels.into_iter().map(|label| {
                div()
                    .w(cell)
                    .text_xs()
                    .whitespace_nowrap()
                    .text_color(theme.muted_foreground)
                    .children(label)
            })),
        )
}

fn nav_item(route: Route, icon: Lucide, current: Option<Route>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let s = surfaces(cx);
    let active = current == Some(route);
    h_flex()
        .id(SharedString::from(format!("nav-{}", route.title())))
        .gap_2p5()
        .px_3()
        .h_9()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .when(active, |d| d.bg(s.accent).text_color(theme.primary_foreground).font_medium())
        .when(!active, |d| d.text_color(theme.sidebar_foreground).hover(|d| d.bg(theme.sidebar_accent)))
        .child(Icon::new(icon).small())
        .child(route.title())
        .on_click(move |_, window, cx| dispatch(Command::Navigate(route), window, cx))
}

fn section_label(label: &str, cx: &App) -> impl IntoElement {
    div()
        .px_3()
        .pt_4()
        .pb_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(label.to_string())
}

fn tag_row(
    node: &TagNode,
    depth: usize,
    selected: Option<&str>,
    expanded: &HashSet<String>,
    icons: &HashMap<String, String>,
    pinned: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let path = node.path.clone();
    let is_selected = selected == Some(path.as_str());
    let has_children = !node.children.is_empty() && !pinned;
    let is_expanded = expanded.contains(&path);
    let icon = icons.get(&path).cloned();
    let toggle_path = path.clone();
    let open_path = path.clone();
    let menu_path = path.clone();
    let row = h_flex()
        .id(SharedString::from(format!("tag-{}-{}", if pinned { "pin" } else { "tree" }, path)))
        .group("tag-row")
        .gap_2()
        .pl(rems(0.75 + depth as f32 * 1.0))
        .pr_2()
        .h_8()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .when(is_selected, |d| d.bg(theme.sidebar_accent).text_color(theme.foreground).font_medium())
        .when(!is_selected, |d| d.text_color(theme.sidebar_foreground).hover(|d| d.bg(theme.sidebar_accent)))
        .child(match icon {
            Some(emoji) => div().w_4().flex().justify_center().child(emoji).into_any_element(),
            None => div()
                .w_4()
                .flex()
                .justify_center()
                .text_color(theme.muted_foreground)
                .font_semibold()
                .child("#")
                .into_any_element(),
        })
        .child(div().flex_1().min_w_0().truncate().child(if pinned { node.path.clone() } else { node.name.clone() }))
        .child(div().text_xs().text_color(theme.muted_foreground).child(node.count.to_string()))
        .when(has_children, |this| {
            this.child(
                div()
                    .id(SharedString::from(format!("tag-toggle-{path}")))
                    .size_5()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .hover(|d| d.bg(theme.border))
                    .child(
                        Icon::new(if is_expanded { Lucide::ChevronDown } else { Lucide::ChevronRight })
                            .xsmall()
                            .text_color(theme.muted_foreground),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        dispatch(Command::ToggleTagExpanded(toggle_path.clone()), window, cx)
                    }),
            )
        })
        .on_click(move |_, window, cx| dispatch(Command::OpenTag(open_path.clone()), window, cx))
        .context_menu(move |menu, _, cx| {
            let tag_pinned = Library::global(cx).read(cx).store().tag_meta(&menu_path).pinned;
            menu.item(menu_item(
                if tag_pinned { t!("取消置顶") } else { t!("置顶") },
                if tag_pinned { Lucide::PinOff } else { Lucide::Pin },
                Command::TogglePinTag(menu_path.clone()),
            ))
            .item(menu_item(t!("编辑标签/图标"), Lucide::Pencil, Command::EditTag(menu_path.clone())))
            .item(menu_item(t!("AI 洞察此标签"), Lucide::Sparkles, Command::InsightMemos(Vec::new(), format!("#{}", menu_path))))
            .separator()
            .item(danger_menu_item(t!("删除"), Lucide::Trash, Command::DeleteTag(menu_path.clone()), cx))
        });
    let mut column = v_flex().child(row);
    if has_children && is_expanded {
        for child in &node.children {
            column = column.child(tag_row(child, depth + 1, selected, expanded, icons, false, cx));
        }
    }
    column.into_any_element()
}

fn find_node<'a>(nodes: &'a [TagNode], path: &str) -> Option<&'a TagNode> {
    for node in nodes {
        if node.path == path {
            return Some(node);
        }
        if path.starts_with(&format!("{}/", node.path))
            && let Some(found) = find_node(&node.children, path)
        {
            return Some(found);
        }
    }
    None
}

fn profile_menu(nickname: String, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    Button::new("profile-menu")
        .ghost()
        .compact()
        .child(
            h_flex()
                .gap_1p5()
                .child(div().font_semibold().text_color(theme.foreground).child(nickname))
                .child(
                    div()
                        .px_1()
                        .rounded(px(3.))
                        .bg(theme.foreground)
                        .text_color(theme.background)
                        .text_size(px(10.))
                        .font_bold()
                        .child("PRO"),
                )
                .child(Icon::new(Lucide::ChevronDown).xsmall().text_color(theme.muted_foreground)),
        )
        .dropdown_menu_with_anchor(Anchor::TopLeft, |menu, _, _| {
            menu.item(menu_item(t!("记录统计"), Lucide::ChartNoAxesColumn, Command::Navigate(Route::Stats)))
                .item(menu_item(t!("AI 记忆档案"), Lucide::Brain, Command::Navigate(Route::Memory)))
                .item(menu_item(t!("认知地图"), Lucide::Mountain, Command::Navigate(Route::Map)))
                .item(menu_item(t!("笔记关系图"), Lucide::Waypoints, Command::Navigate(Route::Graph)))
                .separator()
                .item(menu_item(t!("回收站"), Lucide::Trash, Command::Navigate(Route::Trash)))
                .item(menu_item(t!("账号与资料"), Lucide::UserRound, Command::OpenSettings(SettingsSection::Account)))
                .item(menu_item(t!("设置"), Lucide::Settings, Command::OpenSettings(SettingsSection::General)))
        })
}

pub fn render_sidebar(ws: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let library = Library::global(cx);
    let lib = library.read(cx);
    let theme = cx.theme();
    let s = surfaces(cx);
    let route = ws.route();
    let overview = lib.overview().clone();
    let days = lib.day_stats();
    let tree = lib.tag_tree();
    let selected_tag = if route == Route::Memos { ws.selected_tag(cx) } else { None };
    let selected_day = if route == Route::Memos { ws.selected_day(cx) } else { None };
    let nav_route = (selected_tag.is_none() && selected_day.is_none()).then_some(route);
    let settings = lib.settings().clone();
    let pinned_tags: Vec<TagNode> = lib
        .store()
        .pinned_tags()
        .iter()
        .filter_map(|t| find_node(&tree, t).cloned())
        .collect();
    let icons: HashMap<String, String> = lib
        .tag_counts()
        .keys()
        .filter_map(|t| lib.store().tag_meta(t).icon.map(|i| (t.clone(), i)))
        .collect();
    let expanded = ws.expanded_tags().clone();
    let stat = |value: usize, label: &'static str| {
        v_flex()
            .gap_0p5()
            .child(div().text_2xl().font_semibold().text_color(theme.muted_foreground).child(value.to_string()))
            .child(div().text_xs().text_color(theme.muted_foreground).child(label))
    };

    let muted = theme.muted_foreground;
    v_flex()
        .w(px(SIDEBAR_WIDTH))
        .flex_shrink_0()
        .h_full()
        .bg(s.sidebar)
        .child(
            div()
                .h(px(40.))
                .w_full()
                .flex_shrink_0()
                .on_mouse_down(MouseButton::Left, |event, window, _| {
                    if event.click_count == 2 {
                        window.titlebar_double_click();
                    } else {
                        window.start_window_move();
                    }
                }),
        )
        .child(
            v_flex()
                .px_4()
                .gap_4()
                .child(
                    h_flex()
                        .ml(px(-6.))
                        .mr(px(-6.))
                        .gap_0p5()
                        .child(profile_menu(settings.nickname.clone(), cx))
                        .child(div().flex_1())
                        .child(
                            Button::new("sidebar-refresh")
                                .ghost()
                                .small()
                                .icon(Icon::new(Lucide::RefreshCw).text_color(muted))
                                .tooltip(t!("刷新：立即读取 MCP / API 等外部写入"))
                                .on_click(|_, window, cx| dispatch(Command::Refresh, window, cx)),
                        )
                        .child(
                            Button::new("sidebar-hide")
                                .ghost()
                                .small()
                                .icon(Icon::new(Lucide::Menu).text_color(muted))
                                .tooltip_with_action(t!("隐藏侧边栏"), &ToggleSidebar, None)
                                .on_click(|_, window, cx| dispatch(Command::ToggleSidebar, window, cx)),
                        ),
                )
                .child(
                    h_flex()
                        .id("overview-stats")
                        .justify_between()
                        .pr_4()
                        .cursor_pointer()
                        .child(stat(overview.memos, t!("笔记")))
                        .child(stat(overview.tags, t!("标签")))
                        .child(stat(overview.days, t!("天")))
                        .on_click(|_, window, cx| dispatch(Command::Navigate(Route::Stats), window, cx)),
                )
                .when(settings.show_heatmap, |this| this.child(heatmap(&days, selected_day, cx))),
        )
        .child(
            v_flex()
                .id("sidebar-scroll")
                .flex_1()
                .min_h_0()
                .px_2()
                .pt_4()
                .pb_3()
                .gap_0p5()
                .overflow_y_scrollbar()
                .child(nav_item(Route::Memos, Lucide::LayoutGrid, nav_route, cx))
                .child(nav_item(Route::Inbox, Lucide::MessageSquareText, nav_route, cx))
                .child(nav_item(Route::Review, Lucide::Sparkle, nav_route, cx))
                .child(nav_item(Route::Insight, Lucide::ScanEye, nav_route, cx))
                .child(nav_item(Route::Walk, Lucide::Footprints, nav_route, cx))
                .child(nav_item(Route::Map, Lucide::Mountain, nav_route, cx))
                .child(section_label(t!("置顶标签"), cx))
                .when(pinned_tags.is_empty(), |this| {
                    this.child(
                        div()
                            .px_3()
                            .text_xs()
                            .text_color(theme.muted_foreground.opacity(0.7))
                            .child(t!("右键标签选择「置顶」")),
                    )
                })
                .children(pinned_tags.iter().map(|node| {
                    tag_row(node, 0, selected_tag.as_deref(), &expanded, &icons, true, cx)
                }))
                .child(section_label(t!("全部标签"), cx))
                .when(tree.is_empty(), |this| {
                    this.child(
                        div()
                            .px_3()
                            .text_xs()
                            .text_color(theme.muted_foreground.opacity(0.7))
                            .child(t!("输入 #标签 即可创建，用 / 分级，如 #读书/小说")),
                    )
                })
                .children(tree.iter().map(|node| {
                    tag_row(node, 0, selected_tag.as_deref(), &expanded, &icons, false, cx)
                })),
        )
}
