//! UI color/border themes (the four acclient-ts themes):
//!
//! * `custom` — ASCII corners (`+ - |`), classic colors.
//! * `utf8` — Unicode corners (`╭─╮│╰╯`), classic colors.
//! * `mono` — ASCII corners, bold/inverse only (no color; renders on any
//!   terminal).
//! * `green` — Unicode corners, all green (classic terminal look).
//!
//! Applied live to every rendered span; persisted to the profile.

use ratatui::style::{Color, Style};
use ratatui::symbols::border::{Set as BorderSet, ROUNDED as ROUND};

/// ASCII border set (ratatui 0.29 dropped the old `border::ASCII` const).
const ASCII: BorderSet = BorderSet {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// One of the four rendering themes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Custom,
    Utf8,
    Mono,
    Green,
}

impl Theme {
    pub const ALL: [Theme; 4] = [Theme::Custom, Theme::Utf8, Theme::Mono, Theme::Green];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Theme::Custom => "custom",
            Theme::Utf8 => "utf8",
            Theme::Mono => "mono",
            Theme::Green => "green",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Theme> {
        Some(match s {
            "custom" => Theme::Custom,
            "utf8" => Theme::Utf8,
            "mono" => Theme::Mono,
            "green" => Theme::Green,
            _ => return None,
        })
    }

    /// Corner/edge glyphs: ASCII vs Unicode.
    #[must_use]
    pub fn border_set(self) -> BorderSet {
        match self {
            Theme::Utf8 | Theme::Green => ROUND,
            Theme::Custom | Theme::Mono => ASCII,
        }
    }

    /// Remap a style to the theme. `green` forces green fg; `mono` strips all
    /// color (keeping bold/italic/dim/inverse) so it renders on any terminal;
    /// the classic themes pass through unchanged.
    #[must_use]
    pub fn style(self, s: Style) -> Style {
        match self {
            Theme::Green => s.fg(Color::Green).bg(Color::Reset),
            Theme::Mono => s.fg(Color::Reset).bg(Color::Reset),
            _ => s,
        }
    }
}
