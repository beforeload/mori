pub mod chat_page;
pub mod composer;
pub mod detail;
pub mod dialogs;
pub mod graph_page;
pub mod inbox_page;
pub mod insight_page;
pub mod library;
pub mod map_page;
pub mod memo_card;
pub mod memory_page;
pub mod review_page;
pub mod rich_text;
pub mod settings_page;
pub mod sidebar;
pub mod stats_page;
pub mod stream;
pub mod tasks;
pub mod theme;
pub mod trash_page;
pub mod walk_page;
pub mod window_pin;
pub mod workspace;

use chrono::NaiveDate;
use gpui_kit::{App, Global, WeakEntity, Window};

pub use gpui_kit::assets::IconName as Lucide;

use crate::core::model::MemoId;
use settings_page::SettingsSection;
use workspace::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Memos,
    Inbox,
    Review,
    Insight,
    Walk,
    Map,
    Graph,
    Stats,
    Memory,
    Trash,
}

impl Route {
    pub fn title(self) -> &'static str {
        match self {
            Route::Memos => t!("全部笔记"),
            Route::Inbox => t!("API 输入"),
            Route::Review => t!("每日回顾"),
            Route::Insight => t!("AI 洞察"),
            Route::Walk => t!("随机漫步"),
            Route::Map => t!("认知地图"),
            Route::Graph => t!("笔记关系图"),
            Route::Stats => t!("记录统计"),
            Route::Memory => t!("AI 记忆档案"),
            Route::Trash => t!("回收站"),
        }
    }

    /// Full-window views that cover the sidebar, like flomo's walk and map.
    pub fn is_overlay(self) -> bool {
        matches!(self, Route::Walk | Route::Map)
    }
}

/// Everything a memo card, link or menu can ask the workspace to do.
#[derive(Clone, Debug)]
pub enum Command {
    Navigate(Route),
    OpenSettings(SettingsSection),
    ToggleAgent,
    CloseOverlay,
    OpenTag(String),
    OpenDay(NaiveDate),
    OpenMemo(MemoId),
    OpenUrl(String),
    Annotate(MemoId),
    Edit(MemoId),
    Expand(MemoId),
    TogglePin(MemoId),
    CopyText(MemoId),
    CopyLink(MemoId),
    ShareImage(MemoId),
    EditTime(MemoId),
    InsightMemo(MemoId),
    InsightMemos(Vec<MemoId>, String),
    WalkFrom(MemoId),
    History(MemoId),
    Trash(MemoId),
    Restore(MemoId),
    Purge(MemoId),
    EditTag(String),
    DeleteTag(String),
    TogglePinTag(String),
    ToggleTagExpanded(String),
    ToggleSidebar,
    Refresh,
}

pub struct WorkspaceGlobal(pub WeakEntity<Workspace>);

impl Global for WorkspaceGlobal {}

/// Deferred so a command issued from inside any entity update cannot re-enter it.
pub fn dispatch(command: Command, window: &mut Window, cx: &mut App) {
    window.defer(cx, move |window, cx| {
        let Some(workspace) = cx
            .try_global::<WorkspaceGlobal>()
            .and_then(|g| g.0.upgrade())
        else {
            return;
        };
        workspace.update(cx, |ws, cx| ws.handle(command, window, cx));
    });
}
