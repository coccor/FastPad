//! A persistent, non-modal notification surface.
//!
//! Tasks 11/13/14 all report errors via a real, blocking `MessageBoxW` in production
//! (`show_save_error`/`show_language_error`/`show_json_issue`). A blocking dialog raised from inside
//! the deferred startup chain would defeat the entire startup-latency-first design that chain exists
//! to protect, so corrupt/invalid `fastpad.ini` keys (and, per this module's general-purpose shape,
//! any future non-fatal background problem — e.g. Task 17's IPC-bind-failure) are surfaced here
//! instead: pushed onto an in-memory queue that a repaint (`InvalidateRect`) makes visible without
//! ever blocking the message loop. No new child HWND is required; today the queue simply exists as
//! state on `App` and is repainted for, matching the brief's "no status child HWND necessary".

/// One non-modal notification: a plain user-facing message. Deliberately just a `String` today —
/// callers that need more structure later (severity, a source tag, ...) can extend this without
/// disturbing `NotificationCenter`'s API shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notification {
    pub message: String,
}

/// An ordered queue of not-yet-dismissed notifications. Oldest first; `push` appends, `dismiss_all`
/// clears everything (the only dismissal policy FastPad needs today — a single indicator that clears
/// once acknowledged, rather than per-item dismissal UI).
#[derive(Debug, Default)]
pub struct NotificationCenter {
    pending: Vec<Notification>,
}

impl NotificationCenter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues `message` as a new notification. Never blocks, never touches the window: the caller is
    /// responsible for requesting a repaint if it wants the change to become visible promptly.
    pub fn push(&mut self, message: impl Into<String>) {
        self.pending.push(Notification {
            message: message.into(),
        });
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn pending(&self) -> &[Notification] {
        &self.pending
    }

    pub fn dismiss_all(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::NotificationCenter;

    #[test]
    fn pushed_notifications_accumulate_in_order_until_dismissed() {
        // Break caught: a notification queue that drops earlier entries when a new one arrives, or
        // that fails to clear on dismiss_all, would either lose real warnings or leave stale ones
        // visible forever.
        let mut center = NotificationCenter::new();
        assert!(center.is_empty());

        center.push("first");
        center.push("second".to_owned());
        assert_eq!(center.len(), 2);
        assert_eq!(center.pending()[0].message, "first");
        assert_eq!(center.pending()[1].message, "second");

        center.dismiss_all();
        assert!(center.is_empty());
    }
}
