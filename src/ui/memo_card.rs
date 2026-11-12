use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Anchor, AnyElement, App, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, div, img,
    prelude::FluentBuilder as _,
};

use crate::core::model::Memo;
use crate::core::{stats, time};

use super::library::Library;
use super::rich_text::render_content;
use super::theme::{Typography, surfaces};
use super::{Command, Lucide, dispatch};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CardMode {
    Normal,
    Trash,
}

pub struct CardOptions {
    pub scope: &'static str,
    pub keywords: Vec<String>,
    pub expanded: bool,
    pub max_lines: usize,
    pub mode: CardMode,
    pub highlighted: bool,
}

impl CardOptions {
    pub fn new(scope: &'static str) -> Self {
        Self {
            scope,
            keywords: Vec::new(),
            expanded: false,
            max_lines: 10,
            mode: CardMode::Normal,
            highlighted: false,
        }
    }
}

pub fn menu_item(label: &str, icon: Lucide, command: Command) -> PopupMenuItem {
    PopupMenuItem::new(label.to_string())
        .icon(Icon::new(icon))
        .on_click(move |_, window, cx| dispatch(command.clone(), window, cx))
}

pub fn danger_menu_item(label: &str, icon: Lucide, command: Command, cx: &App) -> PopupMenuItem {
    let danger = cx.theme().danger;
    let label = label.to_string();
    PopupMenuItem::element(move |_, _| div().text_color(danger).child(label.clone()))
        .icon(Icon::new(icon).text_color(danger))
        .on_click(move |_, window, cx| dispatch(command.clone(), window, cx))
}

fn card_menu(memo: &Memo, mode: CardMode, scope: &'static str) -> impl IntoElement {
    let id = memo.id.clone();
    let pinned = memo.pinned;
    Button::new(SharedString::from(format!("{scope}-more-{id}")))
        .ghost()
        .xsmall()
        .icon(Icon::new(Lucide::Ellipsis))
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, cx| {
            let id = id.clone();
            if mode == CardMode::Trash {
                return menu
                    .item(menu_item(t!("恢复"), Lucide::ArchiveRestore, Command::Restore(id.clone())))
                    .separator()
                    .item(danger_menu_item(t!("彻底删除"), Lucide::Trash, Command::Purge(id), cx));
            }
            menu.item(menu_item(t!("分享图片"), Lucide::Share2, Command::ShareImage(id.clone())))
                .item(menu_item(t!("批注"), Lucide::MessageSquare, Command::Annotate(id.clone())))
                .item(menu_item(t!("编辑"), Lucide::SquarePen, Command::Edit(id.clone())))
                .item(menu_item(
                    if pinned { t!("取消置顶") } else { t!("置顶") },
                    if pinned { Lucide::PinOff } else { Lucide::Pin },
                    Command::TogglePin(id.clone()),
                ))
                .separator()
                .item(menu_item(t!("相关笔记与详情"), Lucide::PanelRight, Command::OpenMemo(id.clone())))
                .item(menu_item(t!("AI 洞察"), Lucide::Sparkles, Command::InsightMemo(id.clone())))
                .item(menu_item(t!("从这里开始漫步"), Lucide::Footprints, Command::WalkFrom(id.clone())))
                .item(menu_item(t!("历史版本"), Lucide::RotateCcw, Command::History(id.clone())))
                .separator()
                .item(menu_item(t!("复制内容"), Lucide::Copy, Command::CopyText(id.clone())))
                .item(menu_item(t!("复制引用链接"), Lucide::Link, Command::CopyLink(id.clone())))
                .item(menu_item(t!("修改时间"), Lucide::Clock, Command::EditTime(id.clone())))
                .separator()
                .item(danger_menu_item(t!("删除"), Lucide::Trash, Command::Trash(id), cx))
        })
}

pub fn image_strip(scope: &str, memo: &Memo, library: &Library, size_rem: f32) -> Option<AnyElement> {
    if memo.images.is_empty() {
        return None;
    }
    let store = library.store();
    Some(
        h_flex()
            .flex_wrap()
            .gap_2()
            .children(memo.images.iter().enumerate().map(|(i, name)| {
                let path = store.image_path(name);
                let open = path.clone();
                div()
                    .id(SharedString::from(format!("{scope}-img-{}-{i}", memo.id)))
                    .size(gpui_kit::rems(size_rem))
                    .rounded_md()
                    .overflow_hidden()
                    .cursor_pointer()
                    .child(img(path).size_full().object_fit(ObjectFit::Cover))
                    .on_click(move |_, _, cx| {
                        cx.open_url(&format!("file://{}", open.display()));
                    })
            }))
            .into_any_element(),
    )
}

/// A small clickable summary that opens the memo in the detail panel.
pub fn mini_card(scope: &str, memo: &Memo, note: Option<String>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let s = surfaces(cx);
    let id = memo.id.clone();
    v_flex()
        .id(SharedString::from(format!("{scope}-mini-{}", memo.id)))
        .w_full()
        .gap_1()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(s.card)
        .border_1()
        .border_color(theme.border.opacity(0.5))
        .cursor_pointer()
        .hover(|d| d.border_color(theme.border))
        .child(
            h_flex()
                .gap_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(time::fmt_minute(memo.created_at))
                .child(div().flex_1())
                .children(note),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.foreground)
                .line_clamp(3)
                .child(memo.snippet(160)),
        )
        .on_click(move |_, window, cx| dispatch(Command::OpenMemo(id.clone()), window, cx))
        .into_any_element()
}

pub fn memo_card(memo: &Memo, opts: CardOptions, library: &Library, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let s = surfaces(cx);
    let settings = library.settings();
    let typography = Typography::from_settings(settings);
    let id = memo.id.clone();
    let scope = opts.scope;
    let ref_label = |rid: &str| library.ref_label(rid);
    let fold = settings.fold_long_memos && !opts.expanded && opts.mode != CardMode::Trash;
    let (content, truncated) = render_content(
        format!("{scope}-rt-{id}"),
        &memo.content,
        &opts.keywords,
        fold.then_some(opts.max_lines),
        &ref_label,
        typography,
        cx,
    );
    let backlinks = if settings.show_backlinks { library.store().backlinks(&id).len() } else { 0 };
    let mut time_label = time::fmt_minute(memo.created_at);
    if opts.mode == CardMode::Trash
        && let Some(deleted) = memo.deleted_at
    {
        time_label = t!("{} · 删除于 {}", time_label, time::relative_day(deleted));
    }
    let show_source = settings.show_source && !matches!(memo.source.as_str(), "app" | "import");
    let edit_id = id.clone();
    let expand_id = id.clone();
    let detail_id = id.clone();

    v_flex()
        .id(SharedString::from(format!("{scope}-card-{id}")))
        .w_full()
        .gap_2()
        .px_4()
        .pt_3()
        .pb_3p5()
        .rounded_lg()
        .bg(s.card)
        .when(opts.highlighted, |d| d.border_1().border_color(s.accent.opacity(0.6)))
        .hover(|d| d.bg(s.card_hover))
        .child(
            h_flex()
                .gap_2()
                .h_6()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(time_label)
                .when(memo.pinned, |this| {
                    this.child(Icon::new(Lucide::Pin).xsmall().text_color(s.accent))
                })
                .when(show_source, |this| {
                    this.child(
                        div()
                            .px_1p5()
                            .rounded_sm()
                            .bg(theme.muted)
                            .child(stats::source_label(&memo.source).to_string()),
                    )
                })
                .child(div().flex_1())
                .child(card_menu(memo, opts.mode, scope)),
        )
        .child(
            div()
                .id(SharedString::from(format!("{scope}-body-{id}")))
                .text_color(theme.foreground)
                .child(content)
                .when(opts.mode == CardMode::Normal, |this| {
                    this.on_click(move |event, window, cx| {
                        if event.click_count() == 2 {
                            dispatch(Command::Edit(edit_id.clone()), window, cx);
                        }
                    })
                }),
        )
        .when(truncated, |this| {
            this.child(
                div()
                    .id(SharedString::from(format!("{scope}-expand-{id}")))
                    .text_sm()
                    .text_color(theme.link)
                    .cursor_pointer()
                    .child(t!("展开"))
                    .on_click(move |_, window, cx| dispatch(Command::Expand(expand_id.clone()), window, cx)),
            )
        })
        .when(opts.expanded && settings.fold_long_memos && memo.char_count > 300, |this| {
            let collapse_id = id.clone();
            this.child(
                div()
                    .id(SharedString::from(format!("{scope}-collapse-{id}")))
                    .text_sm()
                    .text_color(theme.link)
                    .cursor_pointer()
                    .child(t!("收起"))
                    .on_click(move |_, window, cx| dispatch(Command::Expand(collapse_id.clone()), window, cx)),
            )
        })
        .children(image_strip(scope, memo, library, 5.75))
        .when(backlinks > 0 && opts.mode != CardMode::Trash, |this| {
            this.child(
                h_flex()
                    .id(SharedString::from(format!("{scope}-backlinks-{id}")))
                    .gap_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .cursor_pointer()
                    .hover(|d| d.text_color(theme.foreground))
                    .child(Icon::new(Lucide::MessagesSquare).xsmall())
                    .child(t!("{} 条批注", backlinks))
                    .on_click(move |_, window, cx| dispatch(Command::OpenMemo(detail_id.clone()), window, cx)),
            )
        })
        .into_any_element()
}
