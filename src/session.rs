//! The session manifest: which tabs the primary window had open when it closed, so the next
//! launch can reopen them. Unsaved text is never stored here. It stays in the Recovery snapshots
//! the manifest names, so a lost or corrupt manifest still leaves crash recovery to bring it back.
//!
//! Format (UTF-8, one `key=value` per line): `version=1`, `active=<zero-based entry index>`, then
//! one `file=<caret>|<anchor>|<first visible line>|<path>` or
//! `snapshot=<caret>|<anchor>|<first visible line>|<recovery id as 32 hex digits>` per tab, in tab
//! order. The target is always the last field, so a path containing `|` still parses.

use crate::Result;
use crate::document::{DocumentId, RecoveryId};
use std::path::{Path, PathBuf};

const VERSION: &str = "1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionSource {
    /// A clean file, reopened from disk.
    File(PathBuf),
    /// An unsaved tab, reopened from the Recovery snapshot with this ID.
    Snapshot(RecoveryId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionEntry {
    pub source: SessionSource,
    pub caret: usize,
    pub anchor: usize,
    pub first_line: usize,
}

impl SessionEntry {
    pub fn new(source: SessionSource) -> Self {
        Self {
            source,
            caret: 0,
            anchor: 0,
            first_line: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Session {
    pub active: usize,
    pub entries: Vec<SessionEntry>,
}

impl Session {
    pub fn encode(&self) -> String {
        let mut output = format!("version={VERSION}\r\nactive={}\r\n", self.active);
        for entry in &self.entries {
            let (key, target) = match &entry.source {
                SessionSource::File(path) => ("file", path.to_string_lossy().into_owned()),
                SessionSource::Snapshot(id) => ("snapshot", format!("{:032x}", id.0)),
            };
            output.push_str(&format!(
                "{key}={}|{}|{}|{target}\r\n",
                entry.caret, entry.anchor, entry.first_line
            ));
        }
        output
    }

    /// `None` for anything but a version-1 manifest. Malformed entry lines and unknown keys are
    /// skipped, and an out-of-range active index falls back to the first entry.
    pub fn parse(source: &str) -> Option<Self> {
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);
        let mut version = None;
        let mut active = 0;
        let mut entries = Vec::new();
        for line in source.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "version" => version = Some(value),
                "active" => active = value.parse().unwrap_or(0),
                "file" | "snapshot" => entries.extend(parse_entry(key, value)),
                _ => {}
            }
        }
        if version != Some(VERSION) {
            return None;
        }
        if active >= entries.len() {
            active = 0;
        }
        Some(Self { active, entries })
    }
}

fn parse_entry(key: &str, value: &str) -> Option<SessionEntry> {
    let mut fields = value.splitn(4, '|');
    let caret = fields.next()?.parse().ok()?;
    let anchor = fields.next()?.parse().ok()?;
    let first_line = fields.next()?.parse().ok()?;
    let target = fields.next()?;
    let source = if key == "file" {
        if target.is_empty() {
            return None;
        }
        SessionSource::File(PathBuf::from(target))
    } else {
        if target.len() != 32 || !target.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        SessionSource::Snapshot(RecoveryId::from_u128(
            u128::from_str_radix(target, 16).ok()?,
        ))
    };
    Some(SessionEntry {
        source,
        caret,
        anchor,
        first_line,
    })
}

/// `%LocalAppData%\FastPad\session.ini`, next to `fastpad.ini`.
pub fn session_file_path() -> Result<PathBuf> {
    Ok(crate::platform::paths::fastpad_data_dir()?.join("session.ini"))
}

pub fn write(path: &Path, session: &Session) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::file::saver::save_atomic(path, session.encode().as_bytes())
}

pub fn read(path: &Path) -> Option<Session> {
    Session::parse(&std::fs::read_to_string(path).ok()?)
}

pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Progress through a manifest being reopened, one entry per `WM_FASTPAD_RESTORE_SESSION`.
#[derive(Debug)]
pub struct SessionRestore {
    pub session: Session,
    pub next: usize,
    pub failed: usize,
    /// The tab each processed entry became, by entry index. `None` when the entry failed.
    pub restored: Vec<Option<DocumentId>>,
    /// The empty startup tab, closed at the end once something else was restored.
    pub placeholder: Option<DocumentId>,
}

impl SessionRestore {
    pub fn new(session: Session, placeholder: Option<DocumentId>) -> Self {
        Self {
            session,
            next: 0,
            failed: 0,
            restored: Vec::new(),
            placeholder,
        }
    }

    pub fn next_entry(&self) -> Option<&SessionEntry> {
        self.session.entries.get(self.next)
    }

    pub fn record(&mut self, restored: Option<DocumentId>) {
        if restored.is_none() {
            self.failed += 1;
        }
        self.restored.push(restored);
        self.next += 1;
    }

    /// The tab the saved active entry became, if that entry was restored.
    pub fn saved_active_restored(&self) -> Option<DocumentId> {
        self.restored.get(self.session.active).copied().flatten()
    }

    /// The tab to show at the end: the saved active one, else the last one restored.
    pub fn active_tab(&self) -> Option<DocumentId> {
        self.saved_active_restored()
            .or_else(|| self.restored.iter().rev().find_map(|id| *id))
    }
}

pub fn restore_failure_notice(count: usize) -> String {
    if count == 1 {
        "1 item from the last session could not be reopened.".to_owned()
    } else {
        format!("{count} items from the last session could not be reopened.")
    }
}

pub fn toggle_notice(enabled: bool) -> &'static str {
    if enabled {
        "Session restore is on. Open tabs will reopen on the next launch."
    } else {
        "Session restore is off. Closing FastPad will ask about unsaved changes."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Session {
        Session {
            active: 1,
            entries: vec![
                SessionEntry::new(SessionSource::File(PathBuf::from(r"C:\notes\a b|c.txt"))),
                SessionEntry {
                    source: SessionSource::Snapshot(RecoveryId::from_u128(0xabc)),
                    caret: 12,
                    anchor: 4,
                    first_line: 3,
                },
            ],
        }
    }

    #[test]
    fn a_manifest_round_trips_through_its_text_form() {
        // Break caught: a path with spaces or `|`, or a snapshot ID, that does not survive a
        // write and read, so the next launch reopens the wrong file or none.
        let session = sample();
        let text = session.encode();
        assert!(text.contains("snapshot=12|4|3|00000000000000000000000000000abc"));
        assert_eq!(Session::parse(&text), Some(session));
    }

    #[test]
    fn only_version_one_manifests_are_accepted() {
        // Break caught: a future or hand-damaged manifest restoring garbage instead of nothing.
        assert_eq!(Session::parse("active=0\r\nfile=0|0|0|C:\\a.txt\r\n"), None);
        assert_eq!(
            Session::parse("version=2\r\nfile=0|0|0|C:\\a.txt\r\n"),
            None
        );
        assert!(Session::parse("\u{feff}version=1\r\n").is_some());
    }

    #[test]
    fn malformed_entries_are_skipped_and_a_bad_active_index_falls_back() {
        // Break caught: one damaged line discarding the whole session, or an active index that
        // points past the entries.
        let parsed = Session::parse(
            "version=1\nactive=9\nfile=x|0|0|C:\\bad.txt\nsnapshot=0|0|0|xyz\nfile=0|0|0|\n\
             bogus=1\nfile=1|2|3|C:\\good.txt\n",
        )
        .unwrap();
        assert_eq!(parsed.active, 0);
        assert_eq!(
            parsed.entries,
            vec![SessionEntry {
                source: SessionSource::File(PathBuf::from(r"C:\good.txt")),
                caret: 1,
                anchor: 2,
                first_line: 3,
            }]
        );
    }

    #[test]
    fn write_then_read_returns_the_session_and_a_missing_file_reads_as_none() {
        let dir = std::env::temp_dir().join(format!("fastpad-session-io-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("FastPad").join("session.ini");
        assert_eq!(read(&path), None);
        write(&path, &sample()).unwrap();
        assert_eq!(read(&path), Some(sample()));
        remove(&path);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_progress_activates_the_saved_tab_or_the_last_restored_one() {
        // Break caught: a failed active entry leaving no tab activated, or counting successes as
        // failures in the notice.
        let mut restore = SessionRestore::new(sample(), None);
        assert_eq!(restore.next_entry(), Some(&sample().entries[0]));
        restore.record(Some(DocumentId(7)));
        restore.record(None);
        assert_eq!(restore.next_entry(), None);
        assert_eq!(restore.failed, 1);
        assert_eq!(restore.saved_active_restored(), None);
        assert_eq!(restore.active_tab(), Some(DocumentId(7)));

        let mut restore = SessionRestore::new(sample(), None);
        restore.record(Some(DocumentId(7)));
        restore.record(Some(DocumentId(8)));
        assert_eq!(restore.active_tab(), Some(DocumentId(8)));
        assert_eq!(
            restore_failure_notice(1),
            "1 item from the last session could not be reopened."
        );
        assert_eq!(
            restore_failure_notice(2),
            "2 items from the last session could not be reopened."
        );
    }
}
