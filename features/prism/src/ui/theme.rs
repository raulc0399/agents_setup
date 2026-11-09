use ratatui::style::{Color, Modifier, Style};

/// Named terminal colors adapt to the user's light or dark palette.
pub const STEP: Color = Color::Cyan;
pub const FINDING: Color = Color::Red;
pub const QUESTION: Color = Color::Magenta;
pub const TEXT: Color = Color::Reset;
pub const LINK: Color = Color::Blue;
pub const GROUP: Color = Color::Yellow;
pub const ACCEPTED: Color = Color::Green;
pub const STALE: Color = Color::LightRed;
pub const CHANGED: Color = Color::Yellow;
pub const CONNECTED: Color = Color::Green;
pub const ERROR: Color = Color::Red;
pub const PRIMARY: Color = Color::Cyan;
pub const HINT: Color = Color::DarkGray;
pub const FOCUS_BACKGROUND: Color = Color::DarkGray;

pub fn hint() -> Style {
    Style::default().fg(HINT)
}

pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

pub fn colored(color: Color) -> Style {
    Style::default().fg(color)
}
