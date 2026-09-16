//! What a preview link or image reference points at. Classification never touches the network;
//! local targets resolve against the document's folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkAction {
    External(String),
    Anchor(String),
    LocalFile(PathBuf),
    Ignored,
}

pub fn classify_link(dest: &str, document_dir: Option<&Path>) -> LinkAction {
    let dest = dest.trim();
    let lower = dest.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
        return LinkAction::External(dest.to_owned());
    }
    if let Some(anchor) = dest.strip_prefix('#') {
        return LinkAction::Anchor(percent_decode(anchor).to_lowercase());
    }
    match local_path(dest, document_dir) {
        Some(path) if path.is_file() => LinkAction::LocalFile(path),
        _ => LinkAction::Ignored,
    }
}

pub fn resolve_image_path(dest: &str, document_dir: Option<&Path>) -> Option<PathBuf> {
    local_path(dest.trim(), document_dir)
}

fn local_path(dest: &str, document_dir: Option<&Path>) -> Option<PathBuf> {
    if dest.is_empty() || dest.starts_with('#') || dest.starts_with("//") || has_scheme(dest) {
        return None;
    }
    let without_suffix = dest.split(['#', '?']).next().unwrap_or_default();
    if without_suffix.is_empty() {
        return None;
    }
    let normalized = percent_decode(without_suffix).replace('/', "\\");
    // Reject UNC (`\\host\share`) and device paths (`\\?\`, `\\.\`) after decoding and slash
    // normalization: an untrusted document must never reach the network through a share path.
    if normalized.starts_with("\\\\") {
        return None;
    }
    let path = PathBuf::from(normalized);
    if path.is_absolute() {
        Some(path)
    } else {
        Some(document_dir?.join(path))
    }
}

/// `C:\x` and `C:/x` are drive paths, not a one-letter scheme.
fn has_scheme(dest: &str) -> bool {
    let Some(colon) = dest.find(':') else {
        return false;
    };
    let scheme = &dest[..colon];
    let is_drive = scheme.len() == 1 && scheme.as_bytes()[0].is_ascii_alphabetic();
    !is_drive
        && scheme.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
}

fn percent_decode(value: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        (byte as char).to_digit(16).map(|digit| digit as u8)
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_owned())
}

/// GitHub's heading anchor: lowercase, spaces become hyphens, other punctuation is dropped.
pub fn slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter_map(|character| match character {
            ' ' => Some('-'),
            '-' | '_' => Some(character),
            _ if character.is_alphanumeric() => Some(character),
            _ => None,
        })
        .collect()
}

#[derive(Debug, Default)]
pub struct SlugSet {
    seen: HashSet<String>,
}

impl SlugSet {
    /// Repeated headings get `-1`, `-2`, ... suffixes, as on GitHub; a heading that collides with
    /// a suffix already emitted for an earlier heading (e.g. an explicit "Intro-1") keeps
    /// climbing until it finds a slug nothing has used yet.
    pub fn unique(&mut self, text: &str) -> String {
        let base = slug(text);
        let mut candidate = base.clone();
        let mut suffix = 1_usize;
        while self.seen.contains(&candidate) {
            candidate = format!("{base}-{suffix}");
            suffix += 1;
        }
        self.seen.insert(candidate.clone());
        candidate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("fastpad-links-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn web_and_mail_links_open_externally() {
        for dest in ["https://x.dev/a", "HTTP://X.DEV", "mailto:me@x.dev"] {
            assert_eq!(classify_link(dest, None), LinkAction::External(dest.to_owned()));
        }
    }

    #[test]
    fn fragment_links_are_anchors() {
        assert_eq!(
            classify_link("#Getting-Started", None),
            LinkAction::Anchor("getting-started".into())
        );
    }

    #[test]
    fn existing_local_files_open_in_fastpad() {
        let dir = ScratchDir::new("local");
        std::fs::write(dir.0.join("notes.md"), "x").unwrap();
        let expected = LinkAction::LocalFile(dir.0.join("notes.md"));
        assert_eq!(classify_link("notes.md", Some(&dir.0)), expected);
        assert_eq!(classify_link("./notes.md#part", Some(&dir.0)), LinkAction::LocalFile(dir.0.join(".\\notes.md")));
        assert_eq!(classify_link("missing.md", Some(&dir.0)), LinkAction::Ignored);
        assert_eq!(classify_link("notes.md", None), LinkAction::Ignored);
        let absolute = dir.0.join("notes.md");
        assert_eq!(
            classify_link(absolute.to_str().unwrap(), None),
            LinkAction::LocalFile(absolute)
        );
    }

    #[test]
    fn other_schemes_are_ignored() {
        for dest in ["ftp://x.dev", "javascript:alert(1)", "file:///C:/x", "//host/share", ""] {
            assert_eq!(classify_link(dest, None), LinkAction::Ignored, "{dest}");
        }
    }

    #[test]
    fn image_paths_resolve_locally_and_never_remotely() {
        let dir = PathBuf::from(r"C:\docs");
        assert_eq!(
            resolve_image_path("img/a%20b.png", Some(&dir)),
            Some(PathBuf::from(r"C:\docs\img\a b.png"))
        );
        assert_eq!(resolve_image_path("https://x.dev/a.png", Some(&dir)), None);
        assert_eq!(resolve_image_path("data:image/png;base64,AAAA", Some(&dir)), None);
        assert_eq!(resolve_image_path("a.png", None), None);
    }

    #[test]
    fn unc_and_device_paths_never_resolve() {
        let dir = PathBuf::from(r"C:\docs");
        for dest in [r"/\host/share/a.png", "%2F%2Fhost/share/a.png", r"\\host\share\a.png", r"\\?\C:\x.png"]
        {
            assert_eq!(resolve_image_path(dest, Some(&dir)), None, "{dest}");
            assert_eq!(classify_link(dest, Some(&dir)), LinkAction::Ignored, "{dest}");
        }
    }

    #[test]
    fn slugs_follow_github_rules() {
        assert_eq!(slug("Getting Started!"), "getting-started");
        assert_eq!(slug("C++ & Rust"), "c--rust");
        assert_eq!(slug("Ünïcode_ok-1"), "ünïcode_ok-1");
        let mut slugs = SlugSet::default();
        assert_eq!(slugs.unique("Intro"), "intro");
        assert_eq!(slugs.unique("Intro"), "intro-1");
        assert_eq!(slugs.unique("Intro"), "intro-2");
    }

    #[test]
    fn slug_set_skips_suffixes_already_taken_by_an_explicit_heading() {
        let mut slugs = SlugSet::default();
        assert_eq!(slugs.unique("Intro-1"), "intro-1");
        assert_eq!(slugs.unique("Intro"), "intro");
        assert_eq!(slugs.unique("Intro"), "intro-2");
    }
}
