//! Pure JSON validation and formatting: parses/serializes with `serde_json` only, never touches
//! Win32 or the editor. `main_window.rs`'s `validate_active_json`/`format_active_json` are the
//! thin wiring layer that reads the active document's text, calls into these, and applies the
//! result back to Scintilla inside one undo group (see Task 12's `Editor::replace_all`
//! precedent for that grouping).

/// One JSON parse failure, as a one-based line/column plus `serde_json`'s own message.
/// `line`/`column` are both `0` only for `JsonIssue::invariant()`'s defensive UTF-8 failure, which
/// should be unreachable in practice: `serde_json::to_writer_pretty` only ever emits UTF-8 for a
/// `Value` that was itself parsed from a `&str`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsonIssue {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl JsonIssue {
    fn invariant() -> Self {
        Self {
            line: 0,
            column: 0,
            message: "JSON output was not UTF-8".into(),
        }
    }
}

impl From<serde_json::Error> for JsonIssue {
    fn from(error: serde_json::Error) -> Self {
        Self {
            line: error.line(),
            column: error.column(),
            message: error.to_string(),
        }
    }
}

/// Parses `source` as JSON without producing any output. `Ok(())` means it is valid; `Err` carries
/// the one-based line/column of the first parse failure.
pub fn validate_json(source: &str) -> core::result::Result<(), JsonIssue> {
    serde_json::from_str::<serde_json::Value>(source)
        .map(|_| ())
        .map_err(JsonIssue::from)
}

/// Parses `source` and re-serializes it with `serde_json`'s default two-space pretty printer.
/// `to_writer_pretty` never emits a trailing newline itself, so one is appended only when `source`
/// ended in `\n` (which also covers `\r\n`, since that ends in `\n` too) — formatting a file that
/// had no final newline must not introduce one.
pub fn format_json(source: &str) -> core::result::Result<String, JsonIssue> {
    let value: serde_json::Value = serde_json::from_str(source).map_err(JsonIssue::from)?;
    let mut bytes = Vec::new();
    serde_json::to_writer_pretty(&mut bytes, &value).map_err(JsonIssue::from)?;
    let mut output = String::from_utf8(bytes).map_err(|_| JsonIssue::invariant())?;
    if source.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{format_json, validate_json};

    #[test]
    fn format_uses_two_spaces_and_preserves_final_newline() {
        assert_eq!(
            format_json("{\"a\":[1,2]}\n").unwrap(),
            "{\n  \"a\": [\n    1,\n    2\n  ]\n}\n"
        );
        assert!(!format_json("{\"a\":1}").unwrap().ends_with('\n'));
    }

    #[test]
    fn validation_returns_line_and_column() {
        let issue = validate_json("{\n  bad\n}").unwrap_err();
        assert_eq!((issue.line, issue.column), (2, 3));
    }

    #[test]
    fn format_of_invalid_json_reports_the_same_line_and_column_as_validate() {
        // Break caught: format_json and validate_json disagreeing about where a parse failure is,
        // e.g. by wrapping the error differently on the format path.
        let format_issue = format_json("{\n  bad\n}").unwrap_err();
        let validate_issue = validate_json("{\n  bad\n}").unwrap_err();
        assert_eq!(format_issue, validate_issue);
    }

    #[test]
    fn validate_accepts_already_valid_json() {
        assert_eq!(validate_json("{\"a\":1}"), Ok(()));
    }

    #[test]
    fn format_preserves_a_trailing_crlf_as_a_bare_lf() {
        // `source.ends_with('\n')` is true for both "\n" and "\r\n"; only a single '\n' is ever
        // appended, never the original "\r\n" verbatim.
        assert_eq!(format_json("{\"a\":1}\r\n").unwrap(), "{\n  \"a\": 1\n}\n");
    }
}
