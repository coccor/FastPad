pub mod app;
pub mod bootstrap;
pub mod editor;
pub mod error;
pub mod file;
pub mod launch;
pub mod perf;
pub mod platform;
pub mod window;

pub use error::FastPadError;
pub use error::StartupStage;
pub use launch::{LaunchOptions, LaunchRequest};

pub type Result<T> = std::result::Result<T, FastPadError>;
