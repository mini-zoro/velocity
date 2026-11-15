//! Adapted from Velocity’s MIT OR Apache-2.0 P0 prototype.
//! Small, dependency-free command-line parser; modes never require human interaction.
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Interactive,
    Startup,
    Open,
    Scroll,
    Typing,
    Memory,
    Headless,
    HeadlessFeatures,
    HeadlessWorkbench,
    HeadlessReviewHistory,
    HeadlessTheme,
    ThemeUi,
    HeadlessParse,
    HeadlessIntelligence,
    Intelligence,
    Panels,
}
impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Startup => "startup",
            Self::Open => "open",
            Self::Scroll => "scroll",
            Self::Typing => "typing",
            Self::Memory => "memory",
            Self::Headless => "headless",
            Self::HeadlessFeatures => "headless-view-features",
            Self::HeadlessTheme => "headless-theme-ui",
            Self::ThemeUi => "theme-ui",
            Self::HeadlessReviewHistory => "headless-review-history",
            Self::HeadlessWorkbench => "headless-workbench",
            Self::HeadlessParse => "headless-parse-memory",
            Self::HeadlessIntelligence => "headless-intelligence",
            Self::Intelligence => "intelligence",
            Self::Panels => "panels",
        }
    }
    pub fn automatic(self) -> bool {
        self != Self::Interactive
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub mode: Mode,
    pub new_window: bool,
    pub add_to_window: bool,
    pub wait: bool,
    pub groups: usize,
    pub review_ui: Option<bool>,
    pub intel_words: usize,
    pub restore_session: bool,
    pub width: u32,
    pub height: u32,
    pub lines: usize,
    pub file: Option<PathBuf>,
    pub files: Vec<FileLocation>,
    pub carets: usize,
    pub data_dir: Option<PathBuf>,
    pub project: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub log_input: bool,
    pub seed: u64,
    pub timeout_seconds: u64,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::Interactive,
            new_window: false,
            add_to_window: false,
            wait: false,
            groups: 1,
            review_ui: None,
            intel_words: 100_000,
            restore_session: false,
            width: 1200,
            height: 800,
            lines: 0,
            file: None,
            files: Vec::new(),
            carets: 1,
            data_dir: None,
            project: None,
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
                options.add_file(&arg);

                continue;
            }
            let mode = match arg.as_str() {
                "--bench-startup" => Some(Mode::Startup),
                "--bench-open" => Some(Mode::Open),
                "--bench-scroll" => Some(Mode::Scroll),
                "--bench-typing" => Some(Mode::Typing),
                "--bench-memory" => Some(Mode::Memory),
                "--headless-smoke" => Some(Mode::Headless),
                "--headless-view-features" => Some(Mode::HeadlessFeatures),
                "--headless-theme-ui" => Some(Mode::HeadlessTheme),
                "--bench-theme-ui" => Some(Mode::ThemeUi),
                "--headless-review-history" => Some(Mode::HeadlessReviewHistory),
                "--headless-workbench" => Some(Mode::HeadlessWorkbench),
                "--headless-parse-memory" => Some(Mode::HeadlessParse),
                "--headless-intelligence" => Some(Mode::HeadlessIntelligence),
                "--bench-intelligence" => Some(Mode::Intelligence),
                "--bench-panels" => Some(Mode::Panels),
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
                "--restore-session" => options.restore_session = true,
                "-n" | "--new-window" => options.new_window = true,
                "-a" | "--add" => options.add_to_window = true,
                "-w" | "--wait" => options.wait = true,
                "--groups" => {
                    options.groups = args
                        .next()
                        .ok_or("--groups needs a count")?
                        .parse()
                        .map_err(|_| "invalid group count")?;
                    if !matches!(options.groups, 1..=4) {
                        return Err("--groups must be 1..4".into());
                    }
                }
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
                "--carets" => {
                    options.carets = args
                        .next()
                        .ok_or("--carets needs a count")?
                        .parse()
                        .map_err(|_| "invalid --carets")?;
                    if !matches!(options.carets, 1 | 1000) {
                        return Err("--carets must be 1 or 1000".into());
                    }
                }
                "--data-dir" => {
                    options.data_dir = Some(args.next().ok_or("--data-dir needs a path")?.into())
                }
                "--project" => {
                    options.project = Some(args.next().ok_or("--project needs a path")?.into())
                }
                "-" => options.add_file("-"),
                "--lines" => {
                    options.lines = args
                        .next()
                        .ok_or("--lines needs a value")?
                        .parse()
                        .map_err(|_| "invalid --lines")?
                }
                "--review-ui" => {
                    options.review_ui = Some(match args.next().as_deref() {
                        Some("on") => true,
                        Some("off") => false,
                        _ => return Err("--review-ui needs on or off".into()),
                    });
                }
                "--intel-words" => {
                    options.intel_words = args
                        .next()
                        .ok_or("--intel-words needs 0 or 100000")?
                        .parse()
                        .map_err(|_| "invalid --intel-words")?;
                    if !matches!(options.intel_words, 0 | 100_000) {
                        return Err("--intel-words must be 0 or 100000".into());
                    }
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
                    if args.next().as_deref() != Some("fifo") {
                        return Err("only Fifo is supported".into());
                    }
                }
                _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
                _ => {
                    options.add_file(&arg);
                }
            }
        }
        if options.mode.automatic() && options.files.len() > 1 {
            return Err("benchmarks accept one input file".into());
        }
        if options.mode == Mode::Open
            && (options.files.len() != 1 || options.files[0].path == std::path::Path::new("-"))
        {
            return Err("--bench-open requires one file path".into());
        }
        Ok(Some(options))
    }
    fn add_file(&mut self, arg: &str) {
        let location = FileLocation::parse(arg);
        if self.file.is_none() {
            self.file = Some(location.path.clone());
        }
        self.files.push(location);
    }
}
/// File path with one-based CLI row/scalar column suffixes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileLocation {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
}
impl FileLocation {
    pub fn parse(arg: &str) -> Self {
        let mut path = arg;
        let mut line = 1;
        let mut column = 1;
        if let Some((prefix, last)) = arg.rsplit_once(':')
            && let Ok(number) = last.parse::<usize>()
        {
            path = prefix;
            line = number.max(1);
            if let Some((prefix, previous)) = prefix.rsplit_once(':')
                && let Ok(number) = previous.parse::<usize>()
            {
                path = prefix;
                column = line;
                line = number.max(1);
            }
        }
        Self {
            path: path.into(),
            line,
            column,
        }
    }
}

pub const HELP: &str = "Theme UI: --headless-theme-ui (CPU), --bench-theme-ui (automatic native)\nvelocity [files… | file:line:col | -] [--bench-startup|--bench-open|--bench-scroll|--bench-typing|--bench-memory|--bench-panels|--headless-smoke|--headless-view-features] [file]\n  --width N --height N  logical window size (default 1200 x 800)\n  --large                3000 x 1800 points; actual physical size is logged\n  --lines N              benchmark first N lines; 0 = entire file; bundled sample = 5000\n  --bench-panels         automatic 100-action panel/100k-candidate GUI measurement\n  --headless-view-features  CPU highlight/wrap/fold/minimap measurements, no GUI or GPU\n  --carets 1|1000        typing/headless benchmark selection count\n  -n / -a / -w           new window / add to window / wait for requested sheets to close\n  --groups 1|2|3|4        initial pane count, also applies to GUI typing/scroll modes\n  --headless-workbench   device-free sessions, restore and four-group scene budgets\n  --headless-parse-memory  ten-file parser CPU and footprint, no GUI or GPU\n  --headless-intelligence  100k-word popup typing and definition CPU budgets\n  --bench-intelligence   automatic native popup/index typing, scanout and memory\n  --headless-review-history  50 dirty documents, native full-history save/restore; requires --data-dir\n  --review-ui on|off     Git/diff/spell A/B fixture for intelligence modes\n  --intel-words 0|100000  extra synthetic project words (default 100000); 0 measures the real ten-file vocabulary\n  --data-dir PATH        Packages and Installed Packages location\n  --project PATH         project JSON settings\n  --present fifo         Fifo is the only supported presentation mode\n  --log-input            log native keys, selectors, IME ranges and input latency\n  --output PATH          new JSONL file; existing results are never overwritten\n  --seed N               reproducible decimal u64 typing seed\n  --timeout SECONDS      independent automatic-run watchdog (default 180)\n  --help                 print help without native initialization\n";

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
            &["--bench-startup", "a", "b"],
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

#[cfg(test)]
mod location_tests {
    use super::*;
    #[test]
    fn multiple_files_stdin_and_one_based_unicode_locations() {
        let options = Options::parse(["file:2:3", "other:4", "-"].map(str::to_string))
            .unwrap()
            .unwrap();
        assert_eq!(options.files.len(), 3);
        assert_eq!(
            options.files[0],
            FileLocation {
                path: "file".into(),
                line: 2,
                column: 3
            }
        );
        assert_eq!(options.files[1].line, 4);
        assert_eq!(options.files[1].column, 1);
        assert_eq!(options.files[2].path, PathBuf::from("-"));
        assert_eq!(
            FileLocation::parse("colon:name").path,
            PathBuf::from("colon:name")
        );
    }
}
