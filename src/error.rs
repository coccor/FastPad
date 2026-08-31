use crate::launch::LaunchError;
use std::fmt;

#[derive(Debug)]
pub enum FastPadError {
    Launch(LaunchError),
    Startup {
        stage: StartupStage,
        source: Box<FastPadError>,
    },
    Win32(u32),
    Io(std::io::Error),
    UnsupportedEncoding,
    Json(serde_json::Error),
    Ipc(&'static str),
    Invariant(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupStage {
    ScintillaLoad,
    WindowClassRegistration,
    EditorCreate,
}

impl StartupStage {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::ScintillaLoad => 10,
            Self::WindowClassRegistration => 11,
            Self::EditorCreate => 12,
        }
    }
}

impl fmt::Display for FastPadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Launch(LaunchError::UnknownFlag) => {
                formatter.write_str("unknown command-line flag")
            }
            Self::Launch(LaunchError::MultiplePaths) => {
                formatter.write_str("only one file path may be provided")
            }
            Self::Startup { stage, source } => {
                write!(formatter, "{}: {source}", stage.user_message())
            }
            Self::Win32(code) => write!(formatter, "Win32 error {code}"),
            Self::Io(error) => error.fmt(formatter),
            Self::UnsupportedEncoding => formatter.write_str("unsupported text encoding"),
            Self::Json(error) => error.fmt(formatter),
            Self::Ipc(message) => write!(formatter, "IPC error: {message}"),
            Self::Invariant(message) => write!(formatter, "invariant violated: {message}"),
        }
    }
}

impl std::error::Error for FastPadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Startup { source, .. } => Some(source),
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<LaunchError> for FastPadError {
    fn from(error: LaunchError) -> Self {
        Self::Launch(error)
    }
}

impl From<std::io::Error> for FastPadError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for FastPadError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl FastPadError {
    pub fn startup(stage: StartupStage, source: FastPadError) -> Self {
        Self::Startup {
            stage,
            source: Box::new(source),
        }
    }

    pub fn startup_stage(&self) -> Option<StartupStage> {
        match self {
            Self::Startup { stage, .. } => Some(*stage),
            _ => None,
        }
    }
}

impl StartupStage {
    fn user_message(self) -> &'static str {
        match self {
            Self::ScintillaLoad => "failed to load Scintilla.dll",
            Self::WindowClassRegistration => "failed to register the main window class",
            Self::EditorCreate => "failed to create the editor control",
        }
    }
}
