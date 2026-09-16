use crate::{FastPadError, Result};
use std::path::PathBuf;

/// FastPad's per-user data directory: `%LocalAppData%\FastPad`. Nothing under this directory is
/// created by this helper — callers create whatever files/subdirectories they need (settings,
/// crash-recovery snapshots, IPC session state, ...) on demand.
///
/// Resolved via the `LOCALAPPDATA` environment variable rather than `SHGetKnownFolderPath`: Windows
/// sets it for every interactive user session, and — unlike a `KNOWNFOLDERID` lookup — a spawned
/// child process can trivially be pointed at an isolated scratch directory for tests by overriding
/// this one environment variable, without touching the real user profile.
#[cfg(windows)]
pub fn fastpad_data_dir() -> Result<PathBuf> {
    let value = std::env::var_os("LOCALAPPDATA").ok_or(FastPadError::Invariant(
        "the LOCALAPPDATA environment variable was not set",
    ))?;
    if value.is_empty() {
        return Err(FastPadError::Invariant(
            "the LOCALAPPDATA environment variable was empty",
        ));
    }
    Ok(PathBuf::from(value).join("FastPad"))
}

#[cfg(not(windows))]
pub fn fastpad_data_dir() -> Result<PathBuf> {
    Err(FastPadError::Invariant(
        "FastPad's per-user data directory is only defined on Windows",
    ))
}

#[cfg(test)]
mod tests {
    use super::fastpad_data_dir;

    #[cfg(windows)]
    #[test]
    fn resolves_a_fastpad_subdirectory_under_local_app_data() {
        // Break caught: resolving to LOCALAPPDATA itself (or some other path) instead of FastPad's
        // own subdirectory would let settings/recovery/IPC state collide with unrelated files.
        let local_app_data = std::env::var("LOCALAPPDATA").expect(
            "LOCALAPPDATA is set for every interactive Windows session this test suite runs under",
        );
        let resolved = fastpad_data_dir().unwrap();
        assert_eq!(
            resolved,
            std::path::Path::new(&local_app_data).join("FastPad")
        );
    }
}
