//! Small, dependency-free command-line parser; modes never require human interaction.
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Interactive,
    Startup,
    Scroll,
    Typing,
    Memory,
    Metal,
}
impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Startup => "startup",
            Self::Scroll => "scroll",
            Self::Typing => "typing",
            Self::Memory => "memory",
            Self::Metal => "metal",
        }
    }
    pub fn automatic(self) -> bool {
        self != Self::Interactive
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Present {
    Fifo,
    Mailbox,
    Immediate,
}
impl Present {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fifo => "fifo",
            Self::Mailbox => "mailbox",
            Self::Immediate => "immediate",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub mode: Mode,
    pub present: Present,
    pub width: u32,
    pub height: u32,
    pub lines: usize,
    pub file: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub log_input: bool,
    pub seed: u64,
    pub timeout_seconds: u64,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::Interactive,
            present: Present::Fifo,
            width: 1200,
            height: 800,
            lines: 5000,
            file: None,
            output: None,
            log_input: false,
            seed: 0x7665_6c6f_6369_7479,
            timeout_seconds: 180,
        }
    }
}
impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        let mut mode_seen = false;
        let mut positional_only = false;
        while let Some(arg) = args.next() {
            if positional_only {
                if options.file.replace(arg.into()).is_some() {
                    return Err("only one file may be supplied".into());
                }
                continue;
            }
            let mode = match arg.as_str() {
                "--bench-startup" => Some(Mode::Startup),
                "--bench-scroll" => Some(Mode::Scroll),
                "--bench-typing" => Some(Mode::Typing),
                "--bench-memory" => Some(Mode::Memory),
                "--bench-metal" => Some(Mode::Metal),
                _ => None,
            };
            if let Some(mode) = mode {
                if mode_seen {
                    return Err("select exactly one benchmark mode".into());
                }
                mode_seen = true;
                options.mode = mode;
                continue;
            }
            match arg.as_str() {
                "--help" | "-h" => return Ok(None),
                "--" => positional_only = true,
                "--log-input" => options.log_input = true,
                "--large" => {
                    options.width = 3000;
                    options.height = 1800;
                }
                "--width" | "--height" => {
                    let value: u32 = args
                        .next()
                        .ok_or_else(|| format!("{arg} needs a value"))?
                        .parse()
                        .map_err(|_| format!("invalid {arg}"))?;
                    if !(64..=8192).contains(&value) {
                        return Err(format!("{arg} must be in 64..=8192 logical points"));
                    }
                    if arg == "--width" {
                        options.width = value;
                    } else {
                        options.height = value;
                    }
                }
                "--lines" => {
                    options.lines = args
                        .next()
                        .ok_or("--lines needs a value")?
                        .parse()
                        .map_err(|_| "invalid --lines")?
                }
                "--seed" => {
                    options.seed = args
                        .next()
                        .ok_or("--seed needs a value")?
                        .parse()
                        .map_err(|_| "invalid --seed (use decimal)")?
                }
                "--timeout" => {
                    options.timeout_seconds = args
                        .next()
                        .ok_or("--timeout needs seconds")?
                        .parse()
                        .map_err(|_| "invalid --timeout")?;
                    if options.timeout_seconds == 0 {
                        return Err("--timeout must be positive".into());
                    }
                }
                "--output" => {
                    options.output = Some(args.next().ok_or("--output needs a JSONL path")?.into())
                }
                "--present" => {
                    options.present = match args.next().as_deref() {
                        Some("fifo") => Present::Fifo,
                        Some("mailbox") => Present::Mailbox,
                        Some("immediate") => Present::Immediate,
                        _ => return Err("--present must be fifo, mailbox, or immediate".into()),
                    }
                }
                _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
                _ => {
                    if options.file.replace(arg.into()).is_some() {
                        return Err("only one file may be supplied".into());
                    }
                }
            }
        }
        Ok(Some(options))
    }
}

pub const HELP: &str = "vel-shell [--bench-startup|--bench-scroll|--bench-typing|--bench-memory|--bench-metal] [file]\n  --large                   3000 x 1800 logical points (actual size logged)\n  --width N --height N      default 1200 x 800 logical points\n  --present MODE            fifo (default), mailbox, immediate; unsupported = error\n  --lines N                 load first N lines; 0 = entire file; default 5000\n  --log-input               timestamp every keyboard/IME event and real input latency\n  --output PATH             unique JSONL file (default output/MODE-TIME-PID.jsonl)\n  --seed N                  deterministic typing jitter seed (decimal u64)\n  --timeout SECONDS         automatic-run watchdog; default 180\n  --help                    show this help; no GUI initialization\n";

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(s: &[&str]) -> Result<Option<Options>, String> {
        Options::parse(s.iter().map(|s| s.to_string()))
    }
    #[test]
    fn defaults_and_large() {
        let o = parse(&["--bench-scroll", "--large", "--lines", "0"])
            .unwrap()
            .unwrap();
        assert_eq!(o.mode, Mode::Scroll);
        assert_eq!((o.width, o.height, o.lines), (3000, 1800, 0));
    }
    #[test]
    fn rejects_ambiguous_or_unbounded_arguments() {
        for s in [
            &["--bench-scroll", "--bench-typing"][..],
            &["--width", "0"],
            &["--timeout", "0"],
            &["--present", "fast"],
            &["a", "b"],
            &["--seed"],
            &["--oops"],
        ] {
            assert!(parse(s).is_err(), "{s:?}");
        }
    }
    #[test]
    fn dash_filename_and_help() {
        assert_eq!(
            parse(&["--", "-hello"]).unwrap().unwrap().file,
            Some(PathBuf::from("-hello"))
        );
        assert!(parse(&["--help"]).unwrap().is_none());
    }
}
