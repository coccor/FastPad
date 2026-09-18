//! Versioned crash-recovery snapshot files.
//!
//! Layout (all integers little-endian): `FPS1` magic, u8 version (1), u128 recovery ID,
//! u8 encoding, u64 path byte length + UTF-16LE path (0 = no path), u64 text byte length + UTF-8.

use crate::document::RecoveryId;
use crate::file::encoding::Encoding;
use crate::{FastPadError, Result};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"FPS1";
const VERSION: u8 = 1;
const EXTENSION: &str = "fps";
const QUARANTINE_SUFFIX: &str = ".invalid";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub recovery_id: RecoveryId,
    pub original_path: Option<PathBuf>,
    pub encoding: Encoding,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotCandidate {
    pub path: PathBuf,
    pub snapshot: Snapshot,
}

impl Snapshot {
    pub fn new(
        recovery_id: RecoveryId,
        original_path: Option<PathBuf>,
        encoding: Encoding,
        text: impl Into<String>,
    ) -> Self {
        Self {
            recovery_id,
            original_path,
            encoding,
            text: text.into(),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let path_units = match &self.original_path {
            Some(path) => path
                .to_str()
                .ok_or(malformed("snapshot path is not valid Unicode"))?
                .encode_utf16()
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };
        let path_bytes = path_units.len().checked_mul(2).ok_or(too_large())?;
        let capacity = (MAGIC.len() + 1 + 16 + 1 + 8 + 8)
            .checked_add(path_bytes)
            .and_then(|size| size.checked_add(self.text.len()))
            .filter(|&size| size <= isize::MAX as usize)
            .ok_or(too_large())?;
        let mut bytes = Vec::with_capacity(capacity);
        bytes.extend_from_slice(MAGIC);
        bytes.push(VERSION);
        bytes.extend_from_slice(&self.recovery_id.0.to_le_bytes());
        bytes.push(encoding_byte(self.encoding));
        bytes.extend_from_slice(&(path_bytes as u64).to_le_bytes());
        for unit in path_units {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&(self.text.len() as u64).to_le_bytes());
        bytes.extend_from_slice(self.text.as_bytes());
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > isize::MAX as usize {
            return Err(too_large());
        }
        let mut reader = Reader { rest: bytes };
        if reader.take(MAGIC.len())? != MAGIC {
            return Err(malformed("snapshot magic mismatch"));
        }
        if reader.take_array::<1>()?[0] != VERSION {
            return Err(malformed("unsupported snapshot version"));
        }
        let recovery_id = RecoveryId::from_u128(u128::from_le_bytes(reader.take_array()?));
        let encoding = encoding_from_byte(reader.take_array::<1>()?[0])?;
        let path_bytes = reader.take_length()?;
        if !path_bytes.len().is_multiple_of(2) {
            return Err(malformed("snapshot path length is odd"));
        }
        let original_path = if path_bytes.is_empty() {
            None
        } else {
            let (units, _) = path_bytes.as_chunks::<2>();
            let path = char::decode_utf16(units.iter().map(|unit| u16::from_le_bytes(*unit)))
                .collect::<core::result::Result<String, _>>()
                .map_err(|_| malformed("snapshot path is not valid UTF-16"))?;
            Some(PathBuf::from(path))
        };
        let text = std::str::from_utf8(reader.take_length()?)
            .map_err(|_| malformed("snapshot text is not valid UTF-8"))?
            .to_owned();
        if !reader.rest.is_empty() {
            return Err(malformed("snapshot has trailing bytes"));
        }
        Ok(Self {
            recovery_id,
            original_path,
            encoding,
            text,
        })
    }
}

pub fn snapshot_path(root: &Path, recovery_id: RecoveryId) -> PathBuf {
    root.join(format!("{:032x}.{EXTENSION}", recovery_id.0))
}

/// The recovery ID a snapshot file is named after, or `None` for any other file name.
pub fn snapshot_file_id(path: &Path) -> Option<RecoveryId> {
    if path.extension()? != EXTENSION {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    if stem.len() != 32 || !stem.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(stem, 16)
        .ok()
        .map(RecoveryId::from_u128)
}

pub fn write_snapshot(root: &Path, snapshot: &Snapshot) -> Result<PathBuf> {
    let bytes = snapshot.encode()?;
    std::fs::create_dir_all(root)?;
    let path = snapshot_path(root, snapshot.recovery_id);
    crate::file::saver::save_atomic(&path, &bytes)?;
    Ok(path)
}

pub fn discover_snapshots(root: &Path) -> Result<Vec<SnapshotCandidate>> {
    discover_snapshots_with(root, |_| false)
}

/// Like `discover_snapshots`, but never reads, returns, or quarantines files for IDs `skip` claims.
pub fn discover_snapshots_with(
    root: &Path,
    skip: impl Fn(RecoveryId) -> bool,
) -> Result<Vec<SnapshotCandidate>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_snapshot = path
            .extension()
            .is_some_and(|extension| extension == EXTENSION);
        if !is_snapshot || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let named_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| stem.len() == 32)
            .and_then(|stem| u128::from_str_radix(stem, 16).ok())
            .map(RecoveryId::from_u128);
        if named_id.is_some_and(&skip) {
            continue;
        }
        match read_snapshot(&path) {
            Ok(snapshot) if skip(snapshot.recovery_id) => {}
            Ok(snapshot) => candidates.push(SnapshotCandidate { path, snapshot }),
            Err(FastPadError::Io(_)) => {}
            Err(_) => quarantine(&path),
        }
    }
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(candidates)
}

fn read_snapshot(path: &Path) -> Result<Snapshot> {
    if std::fs::metadata(path)?.len() > isize::MAX as u64 {
        return Err(too_large());
    }
    Snapshot::decode(&std::fs::read(path)?)
}

fn quarantine(path: &Path) {
    let mut target = path.as_os_str().to_owned();
    target.push(QUARANTINE_SUFFIX);
    let _ = std::fs::rename(path, target);
}

struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let Some((taken, rest)) = self.rest.split_at_checked(length) else {
            return Err(malformed("snapshot is truncated"));
        };
        self.rest = rest;
        Ok(taken)
    }

    fn take_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self.take(N)?;
        Ok(bytes.try_into().expect("take returned exactly N bytes"))
    }

    fn take_length(&mut self) -> Result<&'a [u8]> {
        let length = u64::from_le_bytes(self.take_array()?);
        let length = usize::try_from(length).map_err(|_| malformed("snapshot is truncated"))?;
        self.take(length)
    }
}

fn encoding_byte(encoding: Encoding) -> u8 {
    match encoding {
        Encoding::Utf8 => 0,
        Encoding::Utf8Bom => 1,
        Encoding::Utf16Le => 2,
        Encoding::Utf16Be => 3,
    }
}

fn encoding_from_byte(byte: u8) -> Result<Encoding> {
    match byte {
        0 => Ok(Encoding::Utf8),
        1 => Ok(Encoding::Utf8Bom),
        2 => Ok(Encoding::Utf16Le),
        3 => Ok(Encoding::Utf16Be),
        _ => Err(malformed("unknown snapshot encoding")),
    }
}

fn malformed(message: &'static str) -> FastPadError {
    FastPadError::Invariant(message)
}

fn too_large() -> FastPadError {
    malformed("snapshot exceeds the maximum supported size")
}

#[cfg(test)]
mod tests {
    use super::{Snapshot, discover_snapshots, snapshot_file_id, snapshot_path, write_snapshot};
    use crate::document::RecoveryId;
    use crate::file::encoding::Encoding;
    use std::path::{Path, PathBuf};

    #[test]
    fn snapshot_files_name_their_recovery_id() {
        let root = Path::new(r"C:\Recovery");
        let id = RecoveryId::from_u128(0x1234);
        assert_eq!(snapshot_file_id(&snapshot_path(root, id)), Some(id));
        assert_eq!(snapshot_file_id(Path::new(r"C:\Recovery\1234.fps")), None);
        assert_eq!(
            snapshot_file_id(&root.join(format!("{:032x}.txt", 0x1234))),
            None
        );
    }

    #[test]
    fn snapshot_round_trip_preserves_metadata_and_text() {
        let snapshot = Snapshot::new(
            RecoveryId::from_u128(7),
            Some(PathBuf::from("a.txt")),
            Encoding::Utf16Le,
            "unsaved",
        );
        let bytes = snapshot.encode().unwrap();
        assert_eq!(Snapshot::decode(&bytes).unwrap(), snapshot);
    }

    #[test]
    fn corrupt_snapshot_is_rejected_without_panicking() {
        assert!(Snapshot::decode(b"FPS1\xFF").is_err());
    }

    #[test]
    fn every_truncated_prefix_of_a_valid_snapshot_is_rejected() {
        // Break caught: a length field trusted without bounds checks panics on a torn write.
        let snapshot = Snapshot::new(
            RecoveryId::from_u128(u128::MAX - 3),
            Some(PathBuf::from(r"C:\docs\zăpadă.txt")),
            Encoding::Utf8Bom,
            "text 🦀",
        );
        let bytes = snapshot.encode().unwrap();
        for length in 0..bytes.len() {
            assert!(
                Snapshot::decode(&bytes[..length]).is_err(),
                "prefix {length}"
            );
        }
    }

    #[test]
    fn snapshot_without_a_path_round_trips() {
        let snapshot = Snapshot::new(RecoveryId::from_u128(1), None, Encoding::Utf8, "");
        let bytes = snapshot.encode().unwrap();
        assert_eq!(Snapshot::decode(&bytes).unwrap(), snapshot);
    }

    #[test]
    fn malformed_layouts_are_rejected() {
        let valid = Snapshot::new(
            RecoveryId::from_u128(9),
            Some(PathBuf::from("ab")),
            Encoding::Utf16Be,
            "hi",
        )
        .encode()
        .unwrap();
        // magic(4) version(1) id(16) encoding(1) = 22; path length u64 at 22..30.
        let mut trailing = valid.clone();
        trailing.push(0);
        assert!(Snapshot::decode(&trailing).is_err(), "trailing garbage");

        let mut version = valid.clone();
        version[4] = 2;
        assert!(Snapshot::decode(&version).is_err(), "unknown version");

        let mut encoding = valid.clone();
        encoding[21] = 9;
        assert!(Snapshot::decode(&encoding).is_err(), "unknown encoding");

        let mut odd = valid.clone();
        odd[22] = 3;
        assert!(Snapshot::decode(&odd).is_err(), "odd path length");

        let mut oversized = valid.clone();
        oversized[22..30].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(
            Snapshot::decode(&oversized).is_err(),
            "path length overflow"
        );

        let mut lone_surrogate = valid.clone();
        lone_surrogate[30..32].copy_from_slice(&0xD800_u16.to_le_bytes());
        assert!(Snapshot::decode(&lone_surrogate).is_err(), "invalid UTF-16");

        let mut invalid_text = valid.clone();
        let text_start = invalid_text.len() - 2;
        invalid_text[text_start] = 0xFF;
        assert!(Snapshot::decode(&invalid_text).is_err(), "invalid UTF-8");

        let mut magic = valid;
        magic[0] = b'X';
        assert!(Snapshot::decode(&magic).is_err(), "bad magic");
    }

    #[test]
    fn discovery_returns_valid_snapshots_and_quarantines_malformed_files() {
        // Break caught: a malformed snapshot deleted (data loss) or retried forever.
        let root = ScratchRoot::new("discover");
        let snapshot = Snapshot::new(
            RecoveryId::from_u128(0xABCD),
            Some(PathBuf::from("notes.md")),
            Encoding::Utf8,
            "recovered text",
        );
        let written = write_snapshot(root.path(), &snapshot).unwrap();
        let malformed = root.path().join("broken.fps");
        std::fs::write(&malformed, b"FPS1\x01garbage").unwrap();
        let already = root.path().join("old.fps.invalid");
        std::fs::write(&already, b"junk").unwrap();
        let unrelated = root.path().join("readme.txt");
        std::fs::write(&unrelated, b"not a snapshot").unwrap();

        let candidates = discover_snapshots(root.path()).unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].path, written);
        assert_eq!(candidates[0].snapshot, snapshot);
        assert!(!malformed.exists());
        assert_eq!(
            std::fs::read(root.path().join("broken.fps.invalid")).unwrap(),
            b"FPS1\x01garbage"
        );
        assert_eq!(std::fs::read(&already).unwrap(), b"junk");
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"not a snapshot");
    }

    #[test]
    fn discovery_of_a_missing_directory_finds_nothing() {
        let root = ScratchRoot::new("missing");
        assert!(
            discover_snapshots(&root.path().join("absent"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn writing_replaces_the_previous_snapshot_for_the_same_recovery_id() {
        let root = ScratchRoot::new("replace");
        let id = RecoveryId::from_u128(42);
        let first =
            write_snapshot(root.path(), &Snapshot::new(id, None, Encoding::Utf8, "one")).unwrap();
        let second = Snapshot::new(id, None, Encoding::Utf8, "two");
        assert_eq!(write_snapshot(root.path(), &second).unwrap(), first);
        assert_eq!(
            first.file_name().unwrap().to_str().unwrap(),
            "0000000000000000000000000000002a.fps"
        );
        let candidates = discover_snapshots(root.path()).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].snapshot, second);
    }

    struct ScratchRoot(PathBuf);

    impl ScratchRoot {
        fn new(label: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "fastpad-recovery-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for ScratchRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
