//! Centered confirmation dialogs and colored notices.

use std::time::{Duration, Instant};

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::Style,
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::theme::Theme;

const SUCCESS_SECONDS: u64 = 4;
const ERROR_SECONDS: u64 = 12;
const INFO_SECONDS: u64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Success,
    Error,
    Info,
}

/// Short-lived, highlighted outcome of an action, such as a save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
    expires_at: Instant,
}

impl Notice {
    pub fn new(kind: NoticeKind, text: impl Into<String>) -> Self {
        let seconds = match kind {
            NoticeKind::Success => SUCCESS_SECONDS,
            NoticeKind::Error => ERROR_SECONDS,
            NoticeKind::Info => INFO_SECONDS,
        };
        Self {
            kind,
            text: text.into(),
            expires_at: Instant::now() + Duration::from_secs(seconds),
        }
    }

    pub fn is_active(&self) -> bool {
        Instant::now() < self.expires_at
    }

    /// Banner text with a symbol that does not depend on color.
    pub fn label(&self) -> String {
        let symbol = match self.kind {
            NoticeKind::Success => "✔",
            NoticeKind::Error => "✖",
            NoticeKind::Info => "ℹ",
        };
        format!(" {symbol} {}", self.text)
    }

    pub fn style(&self, theme: &Theme) -> Style {
        match self.kind {
            NoticeKind::Success => theme.success_banner(),
            NoticeKind::Error => theme.error_banner(),
            NoticeKind::Info => theme.focus(),
        }
    }

    /// Screen lines the banner needs at `width`, from 1 to 3.
    pub fn height(&self, width: u16) -> u16 {
        let width = usize::from(width.max(1));
        let lines = self.label().chars().count().div_ceil(width);
        u16::try_from(lines.clamp(1, 3)).unwrap_or(1)
    }
}

/// Draws a bordered dialog centered in `area`, sized to its lines.
pub fn render_dialog(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    title: &str,
    lines: Vec<Line<'_>>,
    border: Style,
) {
    let content_width = lines.iter().map(Line::width).max().unwrap_or(0);
    let width = u16::try_from(content_width + 4)
        .unwrap_or(u16::MAX)
        .max(u16::try_from(title.chars().count() + 4).unwrap_or(u16::MAX))
        .min(area.width);
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border)
                    .style(theme.surface())
                    .title(format!(" {title} ")),
            ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::{Notice, NoticeKind};

    #[test]
    fn notices_are_labelled_and_wrap_up_to_three_lines() {
        let notice = Notice::new(NoticeKind::Success, "Saved 2 row(s).");
        assert!(notice.is_active());
        assert_eq!(notice.label(), " ✔ Saved 2 row(s).");
        assert_eq!(notice.height(80), 1);
        let long = Notice::new(NoticeKind::Error, "x".repeat(500));
        assert!(long.label().starts_with(" ✖ "));
        assert_eq!(long.height(100), 3);
    }
}
