//! Adapted from Velocity’s MIT OR Apache-2.0 P0 prototype.
//! Small, dependency-free command-line parser; modes never require human interaction.
use std::path::PathBuf;

pub const PERF_HELP: &str = "\
  --headless-perf-startup [FILE]  main -> first App::scene, idle memory; no file or --lines = blank
  --headless-perf-startup-interactive [FILE]  interactive setup before AppKit (requires --data-dir)
  --headless-perf-file FILE  streamed open, first scene, settled memory, typing and scroll
  --headless-perf-typing FILE | --headless-perf-scroll FILE | --headless-perf-large FILE
                         replay native benchmark actions through App::scene, without a window
  --headless-perf-highlight FILE  full bundled syntax parse + color spans (first and warmed pass)
  --headless-perf-find DIR [DIR...]  literal project search through the production worker
  --headless-perf-goto    production ranking over 100k prepared paths
";

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
    HeadlessLargeFiles,
    HeadlessPerfStartup,
    HeadlessPerfStartupInteractive,
    HeadlessPerfFile,
    HeadlessPerfTyping,
    HeadlessPerfScroll,
    HeadlessPerfLarge,
    HeadlessPerfHighlight,
    HeadlessPerfFind,
    HeadlessPerfGoto,
    LargeFiles,
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
            Self::HeadlessLargeFiles => "headless-large-files",
            Self::HeadlessPerfStartup => "headless-perf-startup",
            Self::HeadlessPerfStartupInteractive => "headless-perf-startup-interactive",
            Self::HeadlessPerfFile => "headless-perf-file",
            Self::HeadlessPerfTyping => "headless-perf-typing",
            Self::HeadlessPerfScroll => "headless-perf-scroll",
            Self::HeadlessPerfLarge => "headless-perf-large",
            Self::HeadlessPerfHighlight => "headless-perf-highlight",
            Self::HeadlessPerfFind => "headless-perf-find",
            Self::HeadlessPerfGoto => "headless-perf-goto",
            Self::LargeFiles => "bench-large-files",
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
    pub background: bool,
    pub stay: bool,
    pub launch_or_new_window: bool,
    pub command: Option<Named>,
    pub version: bool,
    pub bench_help: bool,
    pub safe_mode: bool,
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
    /// Legacy LaunchServices process serial number, never a document path.
    pub launch_services_psn: bool,
    /// A secondary native launch must await and relay its Apple-event paths.
    pub forward_native_opens: bool,
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
            background: false,
            stay: false,
            launch_or_new_window: false,
            command: None,
            version: false,
            bench_help: false,
            safe_mode: false,
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
            launch_services_psn: false,
            forward_native_opens: false,
            seed: 0x7665_6c6f_6369_7479,
            timeout_seconds: 180,
        }
    }
}
impl Options {
    /// Keep LaunchServices' Apple-event recipient alive instead of spawning a GUI child.
    /// Modern macOS launches are adopted by launchd (parent 1); older launches supply
    /// `-psn_<high>_<low>`. Only a path-free interactive launch uses either heuristic:
    /// terminal invocations with files/stdin and measurement modes retain CLI behavior.
    pub fn launched_by_launch_services(&self, parent_pid: u32) -> bool {
        self.mode == Mode::Interactive
            && self.files.is_empty()
            && self.project.is_none()
            && self.command.is_none()
            && !self.wait
            && !self.background
            && !self.launch_or_new_window
            && (parent_pid == 1 || self.launch_services_psn)
    }
    /// Normal editing creates no telemetry; explicit measurements opt in.
    pub fn telemetry_enabled(&self) -> bool {
        self.mode.automatic() || self.log_input || self.output.is_some()
    }
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
                "--headless-large-files" => Some(Mode::HeadlessLargeFiles),
                "--headless-perf-startup" => Some(Mode::HeadlessPerfStartup),
                "--headless-perf-startup-interactive" => Some(Mode::HeadlessPerfStartupInteractive),
                "--headless-perf-file" => Some(Mode::HeadlessPerfFile),
                "--headless-perf-typing" => Some(Mode::HeadlessPerfTyping),
                "--headless-perf-scroll" => Some(Mode::HeadlessPerfScroll),
                "--headless-perf-large" => Some(Mode::HeadlessPerfLarge),
                "--headless-perf-highlight" => Some(Mode::HeadlessPerfHighlight),
                "--headless-perf-find" => Some(Mode::HeadlessPerfFind),
                "--headless-perf-goto" => Some(Mode::HeadlessPerfGoto),
                "--bench-large-files" => Some(Mode::LargeFiles),
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
                "--help-bench" => options.bench_help = true,
                "--version" | "-v" => options.version = true,
                "--safe-mode" => options.safe_mode = true,
                "--background" | "-b" => options.background = true,
                // https://www.sublimetext.com/docs/2/osx_command_line.html
                "--stay" | "-s" => options.stay = true,
                "-ws" | "-sw" => {
                    options.wait = true;
                    options.stay = true;
                }
                "--launch-or-new-window" => options.launch_or_new_window = true,
                "--command" => {
                    options.command = Some(Named::parse(
                        &args.next().ok_or("--command needs a command")?,
                    )?);
                }
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
                _ if is_process_serial_number(&arg) => options.launch_services_psn = true,
                _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
                _ => {
                    options.add_file(&arg);
                }
            }
        }
        if options.mode.automatic()
            && options.mode != Mode::HeadlessPerfFind
            && options.files.len() > 1
        {
            return Err("benchmarks accept one input file".into());
        }
        if matches!(
            options.mode,
            Mode::HeadlessPerfFile
                | Mode::HeadlessPerfTyping
                | Mode::HeadlessPerfScroll
                | Mode::HeadlessPerfLarge
                | Mode::HeadlessPerfHighlight
                | Mode::HeadlessPerfFind
        ) && options.file.is_none()
        {
            return Err(format!("--{} requires a path", options.mode.name()));
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
fn is_process_serial_number(arg: &str) -> bool {
    let Some(serial) = arg.strip_prefix("-psn_") else {
        return false;
    };
    let Some((high, low)) = serial.split_once('_') else {
        return false;
    };
    [high, low]
        .iter()
        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Named {
    pub name: String,
    pub args: serde_json::Value,
}
impl Named {
    fn parse(command: &str) -> Result<Self, String> {
        let (name, raw) = command
            .trim()
            .split_once(char::is_whitespace)
            .unwrap_or((command.trim(), ""));
        if name.is_empty() {
            return Err("--command needs a name".into());
        }
        let args = if raw.trim().is_empty() {
            serde_json::json!({})
        } else {
            let value: serde_json::Value =
                serde_json::from_str(raw).map_err(|e| format!("--command: {e}"))?;
            if !value.is_object() {
                return Err("--command arguments must be a JSON object".into());
            }
            value
        };
        Ok(Self {
            name: name.into(),
            args,
        })
    }
}

/// File path with optional one-based row/scalar column suffixes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileLocation {
    pub path: PathBuf,
    /// Zero denotes a path without a position, including on the single-instance wire.
    pub line: usize,
    pub column: usize,
}
impl FileLocation {
    pub fn path_only(path: PathBuf) -> Self {
        Self {
            path,
            line: 0,
            column: 1,
        }
    }

    pub fn has_position(&self) -> bool {
        self.line != 0
    }

    pub fn parse(arg: &str) -> Self {
        // Build 4180 supports literal colon filenames: https://www.sublimetext.com/download
        if arg.contains(':') && std::path::Path::new(arg).exists() {
            return Self::path_only(arg.into());
        }
        let mut path = arg;
        let mut line = 0;
        let mut column = 1;
        if let Some((prefix, last)) = arg.rsplit_once(':')
            && let Ok(number) = last.parse::<usize>()
        {
            path = prefix;
            line = number.max(1);
            // In "name:42:2", the 42 can be part of an existing filename.
            // Keep two-position syntax when its base exists or neither base exists.
            if let Some((base, previous)) = prefix.rsplit_once(':')
                && let Ok(number) = previous.parse::<usize>()
                && (!std::path::Path::new(prefix).exists() || std::path::Path::new(base).exists())
            {
                path = base;
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

pub const HELP: &str = r#"Usage: velocity [options] [file[:line[:column]] | directory ...]
       velocity [options] -- [files beginning with '-']
       velocity [options] - > output

  --project PATH         Open a project
  --command COMMAND      Run a named command with optional JSON arguments
  -n, --new-window       Request a separate window
  --launch-or-new-window Request a new window only when already running
  -a, --add              Add folders to the current window
  -w, --wait             Return after all requested tabs close (implied for stdin)
  -b, --background       Leave the current application active
  -s, --stay             Do not reactivate the terminal after waiting
  --safe-mode            Start with clean settings and no restored session
  --data-dir PATH        Use a separate settings and session profile
  --help-bench           Show developer measurement options
  -v, --version          Show the version and exit
  -h, --help             Show this help and exit

Regular or piped stdin is loaded when no paths are given. Edited text goes to stdout.
"#;

pub const BENCH_HELP: &str = "Theme UI: --headless-theme-ui (CPU), --bench-theme-ui (automatic native)\nvelocity [files… | file:line:col | -] [--bench-startup|--bench-open|--bench-scroll|--bench-typing|--bench-memory|--bench-panels|--headless-smoke|--headless-view-features] [file]\n  --width N --height N  logical window size (default 1200 x 800)\n  --large                3000 x 1800 points; actual physical size is logged\n  --lines N              benchmark first N lines; 0 = entire file; bundled sample = 5000\n  --bench-panels         automatic 100-action panel/100k-candidate GUI measurement\n  --headless-large-files [4GB-log]  editor prefix, 50MB line and 4GB hex CPU budgets\n  --bench-large-files FILE  automatic native huge/long-line/hex viewport exercise\n  --headless-view-features  CPU highlight/wrap/fold/minimap measurements, no GUI or GPU\n  --carets 1|1000        typing/headless benchmark selection count\n  -n / -a / -w           new window / add to window / wait for requested sheets to close\n  --groups 1|2|3|4        initial pane count, also applies to GUI typing/scroll modes\n  --headless-workbench   device-free sessions, restore and four-group scene budgets\n  --headless-parse-memory  ten-file parser CPU and footprint, no GUI or GPU\n  --headless-intelligence  100k-word popup typing and definition CPU budgets\n  --bench-intelligence   automatic native popup/index typing, scanout and memory\n  --headless-review-history  50 dirty documents, native full-history save/restore; requires --data-dir\n  --review-ui on|off     Git/diff/spell A/B fixture for intelligence modes\n  --intel-words 0|100000  extra synthetic project words (default 100000); 0 measures the real ten-file vocabulary\n  --data-dir PATH        Packages and Installed Packages location\n  --project PATH         project JSON settings\n  --present fifo         Fifo is the only supported presentation mode\n  --log-input            enable native input/latency JSONL (off for normal editing)\n  --output PATH          enable JSONL at PATH; existing results are never overwritten\n  Default logs: $VELOCITY_LOG_DIR, else ~/Library/Logs/Velocity/\n  --seed N               reproducible decimal u64 typing seed\n  --timeout SECONDS      independent automatic-run watchdog (default 180)\n  --render-scenes DIR [--scale 1|2] [--appearance dark|light|both] [--only a,b] [--bench-scene N] [--first-frames N]\n                         developer tool: offscreen PNGs of UI states (or App::scene cost); never opens a window\n  --help                 print help without native initialization\n";

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(s: &[&str]) -> Result<Option<Options>, String> {
        Options::parse(s.iter().map(|s| s.to_string()))
    }
    #[test]
    fn parse_window_flags_command_arguments_and_safe_mode() {
        let o = parse(&[
            "-b",
            "-ws",
            "--launch-or-new-window",
            "--safe-mode",
            "--command",
            r#"insert {"characters":"fixture"}"#,
            "--",
            "-dash.txt",
        ])
        .unwrap()
        .unwrap();
        assert!(o.background && o.wait && o.stay && o.safe_mode && o.launch_or_new_window);
        assert!(!o.new_window);
        assert_eq!(o.files[0].path, PathBuf::from("-dash.txt"));
        let command = o.command.unwrap();
        assert_eq!(command.name, "insert");
        assert_eq!(command.args["characters"], "fixture");
    }
    #[test]
    fn launch_services_keeps_only_path_free_interactive_launches_in_process() {
        let normal = parse(&[]).unwrap().unwrap();
        assert!(normal.launched_by_launch_services(1));
        assert!(!normal.launched_by_launch_services(200));
        let legacy = parse(&["-psn_0_12345"]).unwrap().unwrap();
        assert!(legacy.launch_services_psn);
        assert!(legacy.files.is_empty());
        assert!(legacy.launched_by_launch_services(200));
        for args in [
            vec!["document.txt"],
            vec!["-"],
            vec!["-psn_0_12345", "document.txt"],
            vec!["--bench-scroll"],
            vec!["-psn_0_12345", "--headless-smoke"],
            vec!["--", "-psn_0_12345"],
        ] {
            assert!(
                !parse(&args)
                    .unwrap()
                    .unwrap()
                    .launched_by_launch_services(1),
                "{args:?}"
            );
        }
        for arg in ["-psn_", "-psn_0_", "-psn_0_a", "-psn_0_1_2"] {
            assert!(parse(&[arg]).is_err(), "{arg}");
        }
    }
    #[test]
    fn telemetry_requires_an_explicit_measurement() {
        assert!(!parse(&[]).unwrap().unwrap().telemetry_enabled());
        assert!(!parse(&["file.txt"]).unwrap().unwrap().telemetry_enabled());
        for args in [
            vec!["--log-input"],
            vec!["--output", "run.jsonl"],
            vec!["--headless-smoke"],
        ] {
            assert!(
                parse(&args).unwrap().unwrap().telemetry_enabled(),
                "{args:?}"
            );
        }
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
        assert!(!options.files[2].has_position());
        assert!(!FileLocation::parse("file").has_position());
        assert!(FileLocation::parse("file:1").has_position());
        assert!(FileLocation::parse("file:1:1").has_position());
        assert!(FileLocation::parse("file:0:0").has_position());
        assert_eq!(
            FileLocation::parse("colon:name").path,
            PathBuf::from("colon:name")
        );
    }
}
