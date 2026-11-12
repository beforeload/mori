//! The Mori look: flomo-style green accent, mode-aware surfaces and memo typography.

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
use gpui_kit::{App, Hsla, Window, WindowAppearance, hsla, px, relative};

use crate::core::model::{Appearance, BodyFont, LineSpacing, Settings};

/// The app's own surfaces, derived from the active theme so custom themes still apply.
#[derive(Clone, Copy)]
pub struct Surfaces {
    pub page: Hsla,
    pub sidebar: Hsla,
    pub card: Hsla,
    pub card_hover: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub heat: [Hsla; 6],
}

pub fn surfaces(cx: &App) -> Surfaces {
    let theme = cx.theme();
    let dark = theme.is_dark();
    let accent = theme.primary;
    let empty_cell = if dark { theme.secondary } else { theme.border.opacity(0.7) };
    let heat = [
        empty_cell,
        accent.opacity(0.35),
        accent.opacity(0.5),
        accent.opacity(0.65),
        accent.opacity(0.82),
        accent,
    ];
    if dark {
        Surfaces {
            page: theme.background,
            sidebar: theme.background,
            card: theme.secondary,
            card_hover: theme.secondary_hover,
            accent,
            accent_soft: accent.opacity(0.18),
            heat,
        }
    } else {
        Surfaces {
            page: theme.secondary,
            sidebar: theme.secondary,
            card: theme.background,
            card_hover: theme.background,
            accent,
            accent_soft: accent.opacity(0.12),
            heat,
        }
    }
}

fn resolve_mode(appearance: Appearance, window: &Window) -> ThemeMode {
    match appearance {
        Appearance::Light => ThemeMode::Light,
        Appearance::Dark => ThemeMode::Dark,
        Appearance::System => match window.appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
            _ => ThemeMode::Light,
        },
    }
}

/// Switches light/dark according to the setting and re-applies the brand palette.
pub fn apply(settings: &Settings, window: &mut Window, cx: &mut App) {
    let mode = resolve_mode(settings.appearance, window);
    Theme::change(mode, Some(window), cx);
    apply_brand(cx);
}

fn apply_brand(cx: &mut App) {
    Theme::update(cx, |theme| {
        let dark = theme.mode.is_dark();
        let green = if dark { hsla(148. / 360., 0.34, 0.38, 1.) } else { hsla(148. / 360., 0.55, 0.43, 1.) };
        let green_hover = if dark { hsla(148. / 360., 0.34, 0.43, 1.) } else { hsla(148. / 360., 0.55, 0.38, 1.) };
        let white = hsla(0., 0., 1., 1.);
        let c = &mut theme.colors;
        c.primary = green;
        c.primary_hover = green_hover;
        c.primary_active = green_hover;
        c.primary_foreground = white;
        c.button_primary = green;
        c.button_primary_hover = green_hover;
        c.button_primary_active = green_hover;
        c.button_primary_foreground = white;
        c.ring = green.opacity(0.6);
        c.switch = if dark { hsla(0., 0., 0.3, 1.) } else { hsla(0., 0., 0.82, 1.) };
        c.switch_thumb = hsla(0., 0., 1., 1.);
        c.slider_bar = green;
        c.slider_thumb = white;
        c.progress_bar = green;
        c.sidebar_primary = green;
        c.sidebar_primary_foreground = white;
        c.link = hsla(222. / 360., 0.86, 0.66, 1.);
        c.link_hover = c.link;
        c.link_active = c.link;
        if dark {
            c.background = hsla(0., 0., 0.11, 1.);
            c.sidebar = c.background;
            c.secondary = hsla(0., 0., 0.155, 1.);
            c.secondary_hover = hsla(0., 0., 0.18, 1.);
            c.secondary_active = hsla(0., 0., 0.2, 1.);
            c.popover = hsla(0., 0., 0.15, 1.);
            c.border = hsla(0., 0., 0.21, 1.);
            c.input = hsla(0., 0., 0.24, 1.);
            c.muted = hsla(0., 0., 0.17, 1.);
            c.accent = hsla(0., 0., 0.19, 1.);
            c.list_hover = c.accent;
            c.sidebar_accent = hsla(0., 0., 0.17, 1.);
            c.sidebar_border = c.border;
            c.title_bar = c.background;
            c.title_bar_border = c.background;
        } else {
            c.secondary = hsla(0., 0., 0.962, 1.);
            c.secondary_hover = hsla(0., 0., 0.94, 1.);
            c.sidebar = c.secondary;
            c.sidebar_accent = hsla(0., 0., 0.92, 1.);
            c.sidebar_border = c.secondary;
            c.title_bar = c.secondary;
            c.title_bar_border = c.secondary;
        }
    });
}

/// Font, size and line height used for memo bodies.
#[derive(Clone, Copy)]
pub struct Typography {
    pub family: Option<&'static str>,
    pub size: f32,
    pub line_height: f32,
}

impl Typography {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            family: settings.body_font.family(),
            size: settings.body_font_size.clamp(12., 22.),
            line_height: match settings.line_spacing {
                LineSpacing::Compact => 1.5,
                LineSpacing::Default => 1.7,
                LineSpacing::Loose => 1.95,
            },
        }
    }

    pub fn apply<E: gpui_kit::Styled>(self, element: E) -> E {
        let element = element.text_size(px(self.size)).line_height(relative(self.line_height));
        match self.family {
            Some(family) => element.font_family(family),
            None => element,
        }
    }
}

impl BodyFont {
    pub fn family(self) -> Option<&'static str> {
        match self {
            BodyFont::System => None,
            BodyFont::PingFang => Some("PingFang SC"),
            BodyFont::Songti => Some("Songti SC"),
            BodyFont::Kaiti => Some("Kaiti SC"),
            BodyFont::Mono => Some("Menlo"),
        }
    }
}
