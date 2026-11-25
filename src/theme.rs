//! Minimalist color palette. It uses no opaque background fills, so the
//! terminal keeps its native background and opacity.

use ratatui::style::{Color, Modifier, Style};

pub const TEXT_PRIMARY: Color = Color::Rgb(205, 214, 244);
pub const TEXT_SECONDARY: Color = Color::Rgb(147, 153, 178);
pub const ACCENT: Color = Color::Rgb(137, 180, 250);
pub const WAVE_PEAK: Color = Color::Rgb(180, 190, 254);

pub const TITLE: Style = Style::new().fg(TEXT_PRIMARY).add_modifier(Modifier::BOLD);
pub const HINT: Style = Style::new()
    .fg(TEXT_SECONDARY)
    .add_modifier(Modifier::ITALIC);
pub const SELECTED: Style = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
