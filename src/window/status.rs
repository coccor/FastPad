//! Painted status line: no child HWND. It is only shown while notifications are pending, so the
//! editor keeps its full height in the common case.

use crate::platform::theme::SystemTheme;
use crate::window::notification::NotificationCenter;

const STATUS_HEIGHT_AT_96_DPI: i64 = 22;

pub fn status_height(dpi: u32) -> i32 {
    ((STATUS_HEIGHT_AT_96_DPI * i64::from(dpi.max(96)) + 48) / 96) as i32
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatusModel {
    pub theme: SystemTheme,
}

impl StatusModel {
    pub fn new(theme: SystemTheme) -> Self {
        Self { theme }
    }
}

pub fn status_text(notifications: &NotificationCenter) -> Option<String> {
    let (first, rest) = notifications.pending().split_first()?;
    Some(if rest.is_empty() {
        format!("{} (click to dismiss)", first.message)
    } else {
        format!("{} (+{} more, click to dismiss)", first.message, rest.len())
    })
}

#[cfg(test)]
mod tests {
    use super::status_text;
    use crate::window::notification::NotificationCenter;

    #[test]
    fn status_text_is_hidden_without_notifications_and_counts_extras() {
        // Break caught: an always-visible status line steals editor height, and dropping the
        // extra count hides every warning after the first.
        let mut center = NotificationCenter::new();
        assert_eq!(status_text(&center), None);
        center.push("a");
        assert_eq!(
            status_text(&center).as_deref(),
            Some("a (click to dismiss)")
        );
        center.push("b");
        center.push("c");
        assert_eq!(
            status_text(&center).as_deref(),
            Some("a (+2 more, click to dismiss)")
        );
    }
}
