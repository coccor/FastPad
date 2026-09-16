use std::ffi::OsString;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LaunchOptions {
    pub new_window: bool,
    pub diagnostic: bool,
    pub request: LaunchRequest,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum LaunchRequest {
    #[default]
    New,
    Open(OsString),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchError {
    UnknownFlag,
    MultiplePaths,
}

pub fn parse(
    args: impl IntoIterator<Item = OsString>,
) -> core::result::Result<LaunchOptions, LaunchError> {
    let mut options = LaunchOptions::default();
    let mut path = None;

    for arg in args {
        match arg.to_str() {
            Some("--new-window") => options.new_window = true,
            Some("--diagnostic") => options.diagnostic = true,
            Some(value) if value.starts_with('-') => return Err(LaunchError::UnknownFlag),
            _ if path.is_some() => return Err(LaunchError::MultiplePaths),
            _ => path = Some(arg),
        }
    }

    if let Some(path) = path {
        options.request = LaunchRequest::Open(path);
    }

    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn os(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn empty_launch_creates_new_document() {
        assert_eq!(parse(os(&[])), Ok(LaunchOptions::default()));
    }

    #[test]
    fn file_and_flags_are_order_independent() {
        assert_eq!(
            parse(os(&["notes.md", "--diagnostic", "--new-window"])),
            Ok(LaunchOptions {
                new_window: true,
                diagnostic: true,
                request: LaunchRequest::Open(OsString::from("notes.md")),
            })
        );
    }

    #[test]
    fn two_paths_are_rejected() {
        assert_eq!(
            parse(os(&["a.txt", "b.txt"])),
            Err(LaunchError::MultiplePaths)
        );
    }
}
