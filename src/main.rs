#[macro_use]
mod i18n;
mod api_server;
mod cli;
mod core;
mod mcp;
mod ui;

use gpui_kit::component::Theme;
use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::{
    AppContext as _, Bounds, Menu, MenuItem, OsAction, TitlebarOptions, WindowBounds, WindowOptions,
    point, px, size,
};

use ui::library::Library;
use ui::workspace::{
    FocusSearch, GoHome, LockApp, NewMemo, OpenSettings, Quit, ToggleDarkMode, ToggleSidebar,
    Workspace,
};

pub fn install_menus(cx: &mut gpui_kit::App) {
    cx.set_menus(vec![
        Menu {
            name: "Mori".into(),
            items: vec![
                MenuItem::action(t!("设置…"), OpenSettings),
                MenuItem::separator(),
                MenuItem::action(t!("锁定"), LockApp),
                MenuItem::separator(),
                MenuItem::action(t!("退出 Mori"), Quit),
            ],
            disabled: false,
        },
        Menu {
            name: t!("笔记").into(),
            items: vec![
                MenuItem::action(t!("新建笔记"), NewMemo),
                MenuItem::action(t!("搜索"), FocusSearch),
                MenuItem::action(t!("回到全部笔记"), GoHome),
            ],
            disabled: false,
        },
        Menu {
            name: t!("编辑").into(),
            items: vec![
                MenuItem::os_action(t!("撤销"), Undo, OsAction::Undo),
                MenuItem::os_action(t!("重做"), Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action(t!("剪切"), Cut, OsAction::Cut),
                MenuItem::os_action(t!("复制"), Copy, OsAction::Copy),
                MenuItem::os_action(t!("粘贴"), Paste, OsAction::Paste),
                MenuItem::os_action(t!("全选"), SelectAll, OsAction::SelectAll),
            ],
            disabled: false,
        },
        Menu {
            name: t!("显示").into(),
            items: vec![
                MenuItem::action(t!("显示/隐藏侧边栏"), ToggleSidebar),
                MenuItem::action(t!("切换深色模式"), ToggleDarkMode),
            ],
            disabled: false,
        },
    ]);
}

fn main() {
    if let Some(code) = cli::try_run() {
        std::process::exit(code);
    }

    let store = match core::store::Store::open_default() {
        Ok(store) => store,
        Err(err) => {
            eprintln!("无法打开 Mori 数据库：{err:#}");
            std::process::exit(1);
        }
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let lang = store.settings().language;
            gpui_kit::component::set_locale(lang.gpui_code());
            ui::workspace::bind_keys(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            install_menus(cx);

            let settings = store.settings().clone();
            Theme::update(cx, |theme| theme.font_size = px(settings.font_size.clamp(12., 22.)));
            Library::init(store, cx);

            let bounds = Bounds::centered(None, size(px(1320.), px(880.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Mori".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.), px(16.))),
                }),
                window_min_size: Some(size(px(480.), px(560.))),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Workspace::new(window, cx))
            })
            .expect("无法打开主窗口");
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
