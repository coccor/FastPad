/// Returns true when `code` is a C0 control character (0x00-0x1F) or DEL (0x7F) that an unbound
/// Ctrl combination left for `TranslateMessage` to turn into a `WM_CHAR`, which Scintilla would
/// otherwise insert verbatim. Tab/CR/LF are exempt only without Ctrl, since real Tab/Enter
/// keystrokes produce those codes too and must keep inserting.
pub fn should_ignore_char(code: u16, ctrl_down: bool) -> bool {
    let is_c0_or_del = code <= 0x1F || code == 0x7F;
    if !is_c0_or_del {
        return false;
    }
    let is_plain_whitespace = matches!(code, 0x09 | 0x0D | 0x0A) && !ctrl_down;
    !is_plain_whitespace
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbound_control_characters_are_ignored_but_plain_whitespace_is_not() {
        assert!(should_ignore_char(0x11, true)); // Ctrl+Q -> DC1
        assert!(should_ignore_char(0x01, true)); // Ctrl+A -> SOH
        assert!(should_ignore_char(0x00, true)); // Ctrl+2 -> NUL
        assert!(should_ignore_char(0x1B, true)); // Ctrl+[ -> ESC
        assert!(should_ignore_char(0x7F, false)); // DEL
        assert!(should_ignore_char(0x0A, true)); // Ctrl+Enter -> LF
        assert!(should_ignore_char(0x09, true)); // Ctrl+I -> TAB
        assert!(!should_ignore_char(0x09, false));
        assert!(!should_ignore_char(0x0D, false));
        assert!(!should_ignore_char(0x0A, false));
        assert!(!should_ignore_char(u16::from(b'q'), true));
        assert!(!should_ignore_char(0x00E9, true)); // AltGr/Ctrl+Alt printable output stays
    }
}
