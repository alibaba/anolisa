//! Theme system for cosh-tui.

use ratatui::style::Color;

/// A color theme for the TUI.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Theme {
    pub name: &'static str,
    pub prompt: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub info: Color,
    pub border: Color,
    pub highlight: Color,
    pub muted: Color,
}

pub const DARK: Theme = Theme {
    name: "dark",
    prompt: Color::Green,
    success: Color::Green,
    error: Color::Red,
    warning: Color::Yellow,
    info: Color::Cyan,
    border: Color::DarkGray,
    highlight: Color::White,
    muted: Color::DarkGray,
};

pub const LIGHT: Theme = Theme {
    name: "light",
    prompt: Color::Blue,
    success: Color::Green,
    error: Color::Red,
    warning: Color::Yellow,
    info: Color::Blue,
    border: Color::Gray,
    highlight: Color::Black,
    muted: Color::Gray,
};

pub const MINIMAL: Theme = Theme {
    name: "minimal",
    prompt: Color::White,
    success: Color::White,
    error: Color::White,
    warning: Color::White,
    info: Color::White,
    border: Color::White,
    highlight: Color::White,
    muted: Color::DarkGray,
};

/// Look up a theme by name. Returns `None` if the name is not recognized.
pub fn get_theme(name: &str) -> Option<&'static Theme> {
    match name {
        "dark" => Some(&DARK),
        "light" => Some(&LIGHT),
        "minimal" => Some(&MINIMAL),
        _ => None,
    }
}

/// Return the list of available theme names.
pub fn available_themes() -> Vec<&'static str> {
    vec!["dark", "light", "minimal"]
}
