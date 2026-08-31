pub mod error;
pub mod file;
pub mod launch;

pub use error::FastPadError;
pub use launch::{LaunchOptions, LaunchRequest};

pub type Result<T> = std::result::Result<T, FastPadError>;

pub mod bootstrap {
    use crate::{LaunchOptions, Result};

    pub fn run(_options: LaunchOptions) -> Result<i32> {
        Ok(0)
    }
}
