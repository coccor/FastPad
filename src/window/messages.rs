use windows_sys::Win32::UI::WindowsAndMessaging::WM_APP;

use crate::perf::Milestone;

pub const WM_FASTPAD_LOAD_SETTINGS: u32 = WM_APP + 1;
pub const WM_FASTPAD_OPEN_REQUEST: u32 = WM_APP + 2;
pub const WM_FASTPAD_APPLY_LANGUAGE: u32 = WM_APP + 3;
pub const WM_FASTPAD_RECOVERY: u32 = WM_APP + 4;
pub const WM_FASTPAD_START_IPC: u32 = WM_APP + 5;
pub const WM_FASTPAD_BUILD_CHROME: u32 = WM_APP + 6;
// Not part of the deferred chain: it only drains requests already queued on App.
pub const WM_FASTPAD_IPC_REQUEST: u32 = WM_APP + 7;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeferredAction {
    RepostSelf(u32),
    PostNext(u32),
    RecordFullyReady,
}

pub fn deferred_start_message() -> u32 {
    WM_FASTPAD_LOAD_SETTINGS
}

pub fn classify_deferred_message(message: u32, input_pending: bool) -> Option<DeferredAction> {
    let action = match message {
        WM_FASTPAD_LOAD_SETTINGS => next_action(message, WM_FASTPAD_OPEN_REQUEST, input_pending),
        WM_FASTPAD_OPEN_REQUEST => next_action(message, WM_FASTPAD_APPLY_LANGUAGE, input_pending),
        WM_FASTPAD_APPLY_LANGUAGE => next_action(message, WM_FASTPAD_RECOVERY, input_pending),
        WM_FASTPAD_RECOVERY => next_action(message, WM_FASTPAD_START_IPC, input_pending),
        WM_FASTPAD_START_IPC => next_action(message, WM_FASTPAD_BUILD_CHROME, input_pending),
        WM_FASTPAD_BUILD_CHROME => {
            if input_pending {
                DeferredAction::RepostSelf(message)
            } else {
                DeferredAction::RecordFullyReady
            }
        }
        _ => return None,
    };
    Some(action)
}

pub(crate) fn completed_milestone(action: DeferredAction) -> Option<Milestone> {
    match action {
        DeferredAction::PostNext(WM_FASTPAD_OPEN_REQUEST) => Some(Milestone::SettingsLoaded),
        DeferredAction::PostNext(WM_FASTPAD_APPLY_LANGUAGE) => Some(Milestone::FileLoaded),
        DeferredAction::RecordFullyReady => Some(Milestone::FullyReady),
        _ => None,
    }
}

fn next_action(message: u32, next: u32, input_pending: bool) -> DeferredAction {
    if input_pending {
        DeferredAction::RepostSelf(message)
    } else {
        DeferredAction::PostNext(next)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeferredAction, WM_FASTPAD_APPLY_LANGUAGE, WM_FASTPAD_BUILD_CHROME,
        WM_FASTPAD_LOAD_SETTINGS, WM_FASTPAD_OPEN_REQUEST, WM_FASTPAD_RECOVERY,
        WM_FASTPAD_START_IPC, classify_deferred_message, completed_milestone,
    };
    use crate::perf::Milestone;

    #[test]
    fn deferred_messages_follow_the_required_startup_order() {
        // Break caught: reordering deferred startup units changes bootstrap sequencing after the
        // first paint and invalidates the startup allowlist.
        assert_eq!(
            classify_deferred_message(WM_FASTPAD_LOAD_SETTINGS, false),
            Some(DeferredAction::PostNext(WM_FASTPAD_OPEN_REQUEST))
        );
        assert_eq!(
            classify_deferred_message(WM_FASTPAD_OPEN_REQUEST, false),
            Some(DeferredAction::PostNext(WM_FASTPAD_APPLY_LANGUAGE))
        );
        assert_eq!(
            classify_deferred_message(WM_FASTPAD_APPLY_LANGUAGE, false),
            Some(DeferredAction::PostNext(WM_FASTPAD_RECOVERY))
        );
        assert_eq!(
            classify_deferred_message(WM_FASTPAD_RECOVERY, false),
            Some(DeferredAction::PostNext(WM_FASTPAD_START_IPC))
        );
        assert_eq!(
            classify_deferred_message(WM_FASTPAD_START_IPC, false),
            Some(DeferredAction::PostNext(WM_FASTPAD_BUILD_CHROME))
        );
        assert_eq!(
            classify_deferred_message(WM_FASTPAD_BUILD_CHROME, false),
            Some(DeferredAction::RecordFullyReady)
        );
    }

    #[test]
    fn deferred_message_reposts_itself_when_input_is_pending() {
        // Break caught: advancing deferred startup work ahead of queued keyboard or mouse input
        // steals responsiveness from the first interactive frame.
        assert_eq!(
            classify_deferred_message(WM_FASTPAD_LOAD_SETTINGS, true),
            Some(DeferredAction::RepostSelf(WM_FASTPAD_LOAD_SETTINGS))
        );
        assert_eq!(
            classify_deferred_message(WM_FASTPAD_BUILD_CHROME, true),
            Some(DeferredAction::RepostSelf(WM_FASTPAD_BUILD_CHROME))
        );
    }

    #[test]
    fn deferred_transitions_report_settings_and_file_completion() {
        // Break caught: leaving placeholder settings/file units unrecorded produces zero fields in
        // otherwise valid benchmark frames.
        assert_eq!(
            completed_milestone(DeferredAction::PostNext(WM_FASTPAD_OPEN_REQUEST)),
            Some(Milestone::SettingsLoaded)
        );
        assert_eq!(
            completed_milestone(DeferredAction::PostNext(WM_FASTPAD_APPLY_LANGUAGE)),
            Some(Milestone::FileLoaded)
        );
        assert_eq!(
            completed_milestone(DeferredAction::RecordFullyReady),
            Some(Milestone::FullyReady)
        );
    }
}
