//! Velocity release verification and replacement, independent of the editor and renderer.
mod install;
mod release;
mod verify;

pub use install::{Install, RECOVERY, STARTUP, Stage, apply_update};
pub use release::{CHECK_INTERVAL, Client, Offer, Outcome, State};

use std::{fmt, io};

pub const RELEASES: &str = "https://github.com/mini-zoro/velocity/releases";
pub const HELPER: &str = "--apply-update";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Mac
        }
    }

    fn asset(self, version: &semver::Version) -> String {
        let suffix = match self {
            Self::Mac => "universal.app.zip",
            Self::Windows => "windows-x64.zip",
        };
        format!("Velocity-{version}-{suffix}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Network,
    Release,
    MissingSignature,
    SigningKey,
    Signature,
    Checksum,
    NotWritable,
    ManualInstall,
    Staging,
    State,
}

#[derive(Debug)]
pub struct Error {
    pub reason: Reason,
    detail: String,
}

impl Error {
    fn new(reason: Reason, detail: impl fmt::Display) -> Self {
        Self {
            reason,
            detail: detail.to_string(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.reason, self.detail)
    }
}

impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::new(Reason::Staging, error)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
