use crate::Result;
use crate::file::encoding::{Encoding, decode};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct LoadedFile {
    pub path: PathBuf,
    pub text: String,
    pub encoding: Encoding,
}

pub fn load(path: &Path) -> Result<LoadedFile> {
    let bytes = std::fs::read(path)?;
    let decoded = decode(&bytes)?;
    Ok(LoadedFile {
        path: path.to_path_buf(),
        text: decoded.text,
        encoding: decoded.encoding,
    })
}

#[cfg(test)]
mod tests {
    use super::load;
    use crate::FastPadError;
    use crate::file::encoding::Encoding;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempFixture {
        directory: PathBuf,
        path: PathBuf,
    }

    impl TempFixture {
        fn new(name: &str, bytes: &[u8]) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "fastpad-task10-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join(name);
            std::fs::write(&path, bytes).unwrap();
            Self { directory, path }
        }
        fn path(&self) -> &Path {
            &self.path
        }
    }
    impl Drop for TempFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn load_preserves_path_encoding_and_text() {
        // Break caught: discarding the source path or BOM encoding on decode.
        let file = TempFixture::new("config.json", b"\xEF\xBB\xBF{\"ok\":true}");
        let loaded = load(file.path()).unwrap();
        assert_eq!(loaded.text, "{\"ok\":true}");
        assert_eq!(loaded.encoding, Encoding::Utf8Bom);
        assert_eq!(loaded.path, file.path());
    }

    #[test]
    fn invalid_encoding_does_not_return_partial_text() {
        // Break caught: lossy decoding yields partial text instead of a load error.
        let file = TempFixture::new("bad.txt", &[0x80]);
        assert!(matches!(
            load(file.path()),
            Err(FastPadError::UnsupportedEncoding)
        ));
    }

    #[test]
    fn missing_file_returns_io_error() {
        let file = TempFixture::new("gone.txt", b"gone");
        std::fs::remove_file(file.path()).unwrap();
        assert!(matches!(load(file.path()), Err(FastPadError::Io(_))));
    }
}
