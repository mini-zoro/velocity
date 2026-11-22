//! Internal first-launch mode: vel.exe sends the initial request over the pipe.
pub(crate) const SERVE: &str = "--internal-windows-serve";

#[derive(Debug)]
pub(crate) struct Startup {
    pub(crate) serve: bool,
}

impl Startup {
    pub(crate) fn take() -> Self {
        Self {
            serve: std::env::args_os().nth(1).is_some_and(|arg| arg == SERVE),
        }
    }
}
