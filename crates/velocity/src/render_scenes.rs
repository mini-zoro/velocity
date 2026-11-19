//! Developer tool: renders real application states offscreen to PNG files.
//!
//! `velocity --render-scenes <out-dir> [--scale 1|2] [--appearance dark|light] [--only a,b]`
//!
//! Each scenario builds an actual [`App`] around a [`Workspace`] with synthetic files in a
//! temporary directory, drives it through the same command, panel and pointer paths as the
//! native host, and renders `App::scene` with the real Metal renderer into an offscreen
//! texture. No NSApplication, window, menu or Dock icon is ever created and the process is
//! never activated, so it is safe to run while someone is using the machine.
use super::*;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Parsed `--render-scenes` arguments.
struct Request {
    out: PathBuf,
    scales: Vec<f32>,
    dark: Vec<bool>,
    only: Option<Vec<String>>,
    /// `--bench-scene N`: measure N `App::scene` calls per scenario instead of rendering.
    bench: usize,
    /// `--first-frames N`: render the first N frames as the native loop produces them
    /// (one worker poll per frame, no settling), e.g. an overlay before ranking finishes.
    first_frames: usize,
}

impl Request {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut args = args.iter();
        let out = args
            .next()
            .filter(|a| !a.starts_with('-'))
            .ok_or("--render-scenes needs an output directory")?;
        let mut request = Self {
            out: PathBuf::from(out),
            scales: vec![2.],
            dark: vec![true, false],
            only: None,
            bench: 0,
            first_frames: 0,
        };
        while let Some(arg) = args.next() {
            let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
            match arg.as_str() {
                "--scale" => {
                    request.scales = value
                        .split(',')
                        .map(|s| match s {
                            "1" => Ok(1.),
                            "2" => Ok(2.),
                            "3" => Ok(3.),
                            _ => Err(format!("unsupported scale {s}")),
                        })
                        .collect::<Result<_, _>>()?;
                }
                "--appearance" => {
                    request.dark = match value.as_str() {
                        "dark" => vec![true],
                        "light" => vec![false],
                        "both" => vec![true, false],
                        _ => return Err("--appearance must be dark, light or both".into()),
                    };
                }
                "--only" => request.only = Some(value.split(',').map(str::to_owned).collect()),
                "--first-frames" => {
                    request.first_frames =
                        value.parse().map_err(|_| "invalid --first-frames count")?
                }
                "--bench-scene" => {
                    request.bench = value.parse().map_err(|_| "invalid --bench-scene count")?
                }
                _ => return Err(format!("unknown --render-scenes option {arg}")),
            }
        }
        Ok(request)
    }
}

/// Synthetic project on disk. Every name is generic; nothing comes from the user's machine.
struct Fixture {
    root: PathBuf,
    data: PathBuf,
    project: PathBuf,
    file: PathBuf,
}

const MAIN_RS: &str = r#"//! Tiny task runner used by the renderer's design scenes.
use std::collections::BTreeMap;
use std::fmt;

/// A unit of work with a priority and optional dependencies.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub name: String,
    pub priority: u8,
    pub depends_on: Vec<String>,
}

impl Task {
    pub fn new(name: &str, priority: u8) -> Self {
        Self {
            name: name.to_owned(),
            priority,
            depends_on: Vec::new(),
        }
    }

    pub fn after(mut self, other: &str) -> Self {
        self.depends_on.push(other.to_owned());
        self
    }
}

impl fmt::Display for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (p{})", self.name, self.priority)
    }
}

/// Orders tasks so that every dependency runs first.
pub fn schedule(tasks: &[Task]) -> Result<Vec<&Task>, String> {
    let by_name: BTreeMap<_, _> = tasks.iter().map(|t| (t.name.as_str(), t)).collect();
    let mut ordered = Vec::new();
    let mut visiting = Vec::new();
    for task in tasks {
        visit(task, &by_name, &mut visiting, &mut ordered)?;
    }
    Ok(ordered)
}

fn visit<'a>(
    task: &'a Task,
    by_name: &BTreeMap<&str, &'a Task>,
    visiting: &mut Vec<&'a str>,
    ordered: &mut Vec<&'a Task>,
) -> Result<(), String> {
    if ordered.iter().any(|t| t.name == task.name) {
        return Ok(());
    }
    if visiting.contains(&task.name.as_str()) {
        return Err(format!("cycle at {}", task.name));
    }
    visiting.push(&task.name);
    for name in &task.depends_on {
        let next = by_name.get(name.as_str()).ok_or("unknown dependency")?;
        visit(next, by_name, visiting, ordered)?;
    }
    visiting.pop();
    ordered.push(task);
    Ok(())
}

fn main() {
    let tasks = vec![
        Task::new("fetch", 2),
        Task::new("build", 1).after("fetch"),
        Task::new("test", 1).after("build"),
        Task::new("deploy", 0).after("test"),
    ];
    match schedule(&tasks) {
        Ok(order) => {
            for (step, task) in order.iter().enumerate() {
                println!("{step:>2}. {task}");
            }
        }
        Err(error) => eprintln!("error: {error}"),
    }
}
"#;

/// Invisible characters: zero-width space, no-break space, a right-to-left override and its
/// terminator, a bell control character.
const INVISIBLE_RS: &str = "// Characters that are easy to miss.\nlet joined = \"zero\u{200b}width\";\nlet spaced = \"no\u{a0}break\";\nlet flipped = \"\u{202e}reversed\u{202c}\";\nlet bell = \"ding\u{7}\";\nlet plain = \"nothing hidden\";\n";

impl Fixture {
    fn create() -> Result<Self, String> {
        let root = std::env::temp_dir().join("velocity-render-scenes");
        if root.exists() {
            std::fs::remove_dir_all(&root).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        // Workspace folders are canonical (/private/var/… on macOS).
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let project_dir = root.join("project");
        let data = root.join("data");
        let files: &[(&str, &str)] = &[
            ("src/main.rs", MAIN_RS),
            (
                "src/lib.rs",
                "pub mod config;\npub mod render;\n\npub fn version() -> &'static str {\n    \"0.1.0\"\n}\n",
            ),
            (
                "src/config.rs",
                "/// Settings loaded at start-up.\npub struct Config {\n    pub width: u32,\n    pub height: u32,\n}\n",
            ),
            (
                "src/render/mod.rs",
                "pub mod canvas;\n\npub fn clear(color: u32) -> u32 {\n    color & 0x00ff_ffff\n}\n",
            ),
            (
                "src/render/canvas.rs",
                "pub struct Canvas {\n    pub pixels: Vec<u32>,\n}\n",
            ),
            (
                "src/render/an_unusually_long_module_name_for_truncation_checks.rs",
                "// long name\n",
            ),
            (
                "tests/schedule.rs",
                "#[test]\nfn orders_dependencies() {\n    assert!(true);\n}\n",
            ),
            ("README.md", "# Sample project\n\nA small task runner.\n"),
            (
                "Cargo.toml",
                "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            (".gitignore", "/target\n"),
            ("notes.txt", "Remember to update the changelog.\n"),
        ];
        for (name, contents) in files {
            let path = project_dir.join(name);
            std::fs::create_dir_all(path.parent().expect("parent")).map_err(|e| e.to_string())?;
            std::fs::write(&path, contents).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir_all(project_dir.join("target/debug")).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(data.join("Packages/User")).map_err(|e| e.to_string())?;
        // Deterministic chrome: no tree/hover animations, no blinking.
        std::fs::write(
            data.join("Packages/User/Preferences.sublime-settings"),
            "{\n\t\"animation_enabled\": false,\n\t\"tree_animation_enabled\": false,\n\t\"caret_style\": \"solid\"\n}\n",
        )
        .map_err(|e| e.to_string())?;
        // Files outside the project folder (so the side bar and Goto stay unchanged): an image
        // with transparency, a damaged image and a text file with invisible characters.
        let scratch = root.join("scratch");
        std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
        let (width, height) = (360u32, 220u32);
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                // A soft disc over a transparent field, with a diagonal band.
                let (dx, dy) = (x as f32 - 130., y as f32 - 110.);
                let disc = (1. - ((dx * dx + dy * dy).sqrt() - 80.).clamp(0., 1.)) * 255.;
                let band = ((x + y) / 24) % 2 == 0 && x > 200;
                let (r, g, b, a) = if disc > 0. {
                    (58, 132, 196, disc as u8)
                } else if band {
                    (233, 160, 64, 210)
                } else {
                    (0, 0, 0, 0)
                };
                pixels.extend_from_slice(&[r, g, b, a]);
            }
        }
        write_png(&scratch.join("logo.png"), width, height, &pixels)?;
        std::fs::write(
            scratch.join("damaged.png"),
            b"\x89PNG\r\n\x1a\nnot really an image",
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(scratch.join("invisible.rs"), INVISIBLE_RS).map_err(|e| e.to_string())?;
        let project = root.join("sample.sublime-project");
        std::fs::write(&project, r#"{"folders":[{"path":"project"}]}"#)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            file: project_dir.join("src/main.rs"),
            root,
            data,
            project,
        })
    }
    fn path(&self, name: &str) -> PathBuf {
        self.root.join("project").join(name)
    }
    fn scratch(&self, name: &str) -> PathBuf {
        self.root.join("scratch").join(name)
    }
}

/// One scene: a name, a logical window size and a builder that returns a prepared app.
struct Scenario {
    name: &'static str,
    size: (f64, f64),
    build: fn(&Fixture, bool) -> Result<App, String>,
}

fn new_app(workspace: Workspace) -> Result<App, String> {
    let options = Options::default();
    let reporter = Reporter::for_options(&options, Instant::now())?;
    Ok(App::new(
        options,
        Clock::new(Instant::now(), std::time::SystemTime::now()),
        reporter,
        workspace,
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
    ))
}

fn project_app(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut workspace = Workspace::new(
        vel_text::Text::new(),
        Some(fixture.data.clone()),
        Some(fixture.project.clone()),
        false,
    )?;
    workspace.set_appearance(dark)?;
    let mut app = new_app(workspace)?;
    app.panels.side_visible = true;
    Ok(app)
}

fn open(app: &mut App, path: &Path) -> Result<(), String> {
    app.workspace.open(crate::cli::FileLocation {
        path: path.to_owned(),
        line: 1,
        column: 1,
    })?;
    // Opening into the initial untitled, unmodified buffer replaces it in Sublime;
    // here the scratch tab is closed explicitly so the tab bar shows real files only.
    if let Some(index) = app
        .workspace
        .tabs
        .iter()
        .position(|t| t.path.is_none() && !t.dirty())
        && app.workspace.tabs.len() > 1
    {
        app.workspace.close(index)?;
    }
    Ok(())
}

fn named(app: &mut App, name: &str, args: Value) {
    app.run_named(None, name, &args);
}

fn pointer(app: &mut App, kind: MouseKind, x: f64, y: f64) {
    app.handle_mouse_event(
        None,
        MouseEvent {
            button: 0,
            kind,
            position: Point { x, y },
            click_count: 1,
            modifiers: Modifiers::default(),
            timestamp: 0.,
        },
    );
}

fn select(app: &mut App, ranges: &[(usize, usize, usize, usize)]) {
    let editor = app.workspace.editor_mut();
    let regions = ranges.iter().map(|&(row, col, end_row, end_col)| {
        let a = editor.text.line_to_byte(row) + col;
        let b = editor.text.line_to_byte(end_row) + end_col;
        vel_buffer::Region::new(a, b)
    });
    editor.set_selection(vel_buffer::Selection::from_regions(regions));
}

fn editor_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    // Fold the Display impl body, bookmark a line, then a selection plus two carets.
    let fold_row = MAIN_RS
        .lines()
        .position(|l| l.starts_with("impl fmt::Display"))
        .unwrap_or(0);
    select(&mut app, &[(fold_row + 1, 4, fold_row + 1, 4)]);
    named(&mut app, "fold", json!({}));
    select(&mut app, &[(39, 0, 39, 0)]);
    named(&mut app, "toggle_bookmark", json!({}));
    // Unsaved edits against the file on disk give added, modified and deleted diff marks.
    select(&mut app, &[(33, 0, 33, 0)]);
    named(
        &mut app,
        "insert",
        json!({"characters": "// TODO: report cycles with their path\n"}),
    );
    let end = MAIN_RS.lines().nth(37).map_or(0, str::len);
    select(&mut app, &[(38, end, 38, end)]);
    named(
        &mut app,
        "insert",
        json!({"characters": " // depth-first order"}),
    );
    select(&mut app, &[(41, 0, 42, 0)]);
    named(&mut app, "left_delete", json!({}));
    settle(&mut app);
    select(
        &mut app,
        &[(44, 12, 44, 20), (45, 8, 45, 8), (46, 8, 46, 8)],
    );
    Ok(app)
}

fn minimap_scrollbar_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = editor_scene(fixture, dark)?;
    select(&mut app, &[(60, 0, 60, 0)]);
    Ok(app)
}

fn editor_hover_scrollbar(app: &mut App) {
    // Rest the pointer on the overlay scroll bar (left of the minimap).
    let editor = app.workspace.editor();
    let x = app.content.x + app.view.right(editor) - 4.;
    let y = app.content.y + app.content.height / 2.;
    pointer(app, MouseKind::Move, x, y);
}

fn sidebar_focused_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = sidebar_scene(fixture, dark)?;
    named(&mut app, "focus_side_bar", json!({}));
    Ok(app)
}

fn editor_hover_gutter(app: &mut App) {
    let y = app.content.y + 200.;
    pointer(app, MouseKind::Move, app.content.x + 20., y);
}

fn tabs_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    for name in [
        "Cargo.toml",
        "README.md",
        "src/lib.rs",
        "src/config.rs",
        "src/render/mod.rs",
        "src/render/canvas.rs",
        "tests/schedule.rs",
        "notes.txt",
        "src/main.rs",
    ] {
        open(&mut app, &fixture.path(name))?;
    }
    settle(&mut app);
    // Dirty tab.
    named(&mut app, "insert", json!({"characters": "// edited\n"}));
    // Preview (transient) tab.
    app.workspace.preview(crate::cli::FileLocation {
        path: fixture.path(".gitignore"),
        line: 1,
        column: 1,
    })?;
    settle(&mut app);
    // Make the dirty file active again so the preview shows as an inactive transient tab.
    if let Some(index) = app
        .workspace
        .tabs
        .iter()
        .position(|t| t.path.as_deref() == Some(fixture.file.as_path()))
    {
        app.workspace.select(index);
    }
    Ok(app)
}

fn split_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    for name in ["src/lib.rs", "src/main.rs", "src/config.rs"] {
        open(&mut app, &fixture.path(name))?;
    }
    settle(&mut app);
    // Split View: Cmd-click a second tab.
    let lib = app
        .workspace
        .tabs
        .iter()
        .position(|t| t.path.as_deref() == Some(fixture.path("src/lib.rs").as_path()))
        .ok_or("lib tab")?;
    app.workspace.click_sheet(lib, true, false);
    Ok(app)
}

fn groups_scene(fixture: &Fixture, dark: bool, preset: &str) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    for name in ["src/main.rs", "src/lib.rs", "src/config.rs", "README.md"] {
        open(&mut app, &fixture.path(name))?;
    }
    named(&mut app, "set_layout", json!({"preset": preset}));
    let groups = app.workspace.workbench.groups.len();
    for (group, name) in ["src/render/mod.rs", "Cargo.toml", "tests/schedule.rs"]
        .into_iter()
        .enumerate()
        .take(groups.saturating_sub(1))
    {
        named(&mut app, "focus_group", json!({"group": group + 1}));
        open(&mut app, &fixture.path(name))?;
    }
    named(&mut app, "focus_group", json!({"group": 1}));
    Ok(app)
}

fn sidebar_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    for name in ["src/main.rs", "README.md"] {
        open(&mut app, &fixture.path(name))?;
    }
    named(
        &mut app,
        "insert",
        json!({"characters": "<!-- draft -->\n"}),
    );
    app.workspace.preview(crate::cli::FileLocation {
        path: fixture.path("notes.txt"),
        line: 1,
        column: 1,
    })?;
    settle(&mut app);
    // Expands the project, src and src/render through the reveal path a click would use.
    app.panels
        .reveal_sidebar_path(&mut app.workspace, fixture.path("src/render/canvas.rs"))?;
    settle(&mut app);
    inject_git(&mut app, fixture);
    Ok(app)
}

/// A synthetic, in-memory Git publication; no repository is created on disk.
fn inject_git(app: &mut App, fixture: &Fixture) {
    use vel_git::{Badge, FileStatus, Head, StatusSnapshot};
    let root = fixture.root.join("project");
    let mut files = std::collections::BTreeMap::new();
    let mut folders = std::collections::BTreeMap::new();
    for (path, badge) in [
        ("src/main.rs", Badge::Modified),
        ("README.md", Badge::StagedModification),
        ("notes.txt", Badge::Untracked),
        ("src/render/canvas.rs", Badge::StagedAddition),
        ("target", Badge::Ignored),
    ] {
        files.insert(
            PathBuf::from(path),
            FileStatus {
                statuses: [badge].into_iter().collect(),
            },
        );
    }
    folders.insert(PathBuf::from("src"), Badge::Modified);
    folders.insert(PathBuf::from("src/render"), Badge::StagedAddition);
    folders.insert(PathBuf::from("target"), Badge::Ignored);
    folders.insert(PathBuf::new(), Badge::Modified);
    app.workspace.git.insert_snapshot(StatusSnapshot {
        root,
        generation: 1,
        head: Head::Branch {
            name: "main".into(),
            unborn: false,
        },
        ahead_behind: None,
        files,
        folders,
    });
}

fn find_scene(fixture: &Fixture, dark: bool, panel: &str, query: &str) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    select(&mut app, &[(40, 0, 40, 0)]);
    named(&mut app, "show_panel", json!({"panel": panel}));
    if let Some(input) = app.panels.input_mut() {
        input.set(query);
    }
    let viewport = app.viewport();
    app.panels.edited(&mut app.workspace, viewport);
    if panel == "replace" {
        app.panels.replace.set("job");
    }
    if panel == "find_in_files" {
        app.panels.where_input.set("<project>, *.rs");
        app.panels.replace.set("");
    }
    Ok(app)
}

fn overlay_scene(fixture: &Fixture, dark: bool, overlay: &str, text: &str) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    named(
        &mut app,
        "show_overlay",
        json!({"overlay": overlay, "text": text}),
    );
    Ok(app)
}

fn picker_scene(fixture: &Fixture, dark: bool, command: &str) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    named(&mut app, command, json!({}));
    Ok(app)
}

fn completion_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    let row = MAIN_RS
        .lines()
        .position(|l| l.contains("Ok(ordered)"))
        .unwrap_or(40);
    let editor = app.workspace.editor_mut();
    let at = editor.text.line_to_byte(row);
    editor.set_caret(at, false);
    named(&mut app, "insert", json!({"characters": "    vi"}));
    named(&mut app, "auto_complete", json!({}));
    Ok(app)
}

fn set_view(app: &mut App, key: &str, value: Value) {
    app.workspace
        .editor_mut()
        .settings
        .set(vel_config::settings::Layer::View, key, value);
}

/// A completion popup in a short window: the caret sits low in a 280-point window.
fn completion_short_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    let editor = app.workspace.editor_mut();
    let at = editor.text.line_to_byte(41);
    editor.set_caret(at, false);
    named(&mut app, "insert", json!({"characters": "t"}));
    named(&mut app, "auto_complete", json!({}));
    Ok(app)
}

fn completion_ui2_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = completion_scene(fixture, dark)?;
    set_view(&mut app, "ui_scale", json!(2.));
    Ok(app)
}

fn image_scene(fixture: &Fixture, dark: bool, name: &str) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    open(&mut app, &fixture.scratch(name))?;
    settle(&mut app);
    Ok(app)
}

/// Invisible characters drawn as boxed hex labels: on the caret line, in a selection and
/// plain; the minimap shows the same lines.
fn invisibles_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.scratch("invisible.rs"))?;
    for (key, value) in [
        ("draw_unicode_white_space", json!("all")),
        ("draw_unicode_bidi", json!(true)),
        ("draw_control_characters", json!(true)),
    ] {
        set_view(&mut app, key, value);
    }
    settle(&mut app);
    // The caret on the zero-width-space line; the bidi line selected.
    let text = app.workspace.editor().text.to_string();
    let line = |needle: &str| text.lines().position(|l| l.contains(needle)).unwrap_or(0);
    let (caret, selected) = (line("zero"), line("flipped"));
    let len = text.lines().nth(selected).map_or(0, str::len);
    select(&mut app, &[(selected, 0, selected, len)]);
    let editor = app.workspace.editor_mut();
    let at = editor.text.line_to_byte(caret) + 4;
    let mut regions = editor.regions();
    regions.push(vel_buffer::Region::caret(at));
    editor.set_selection(vel_buffer::Selection::from_regions(regions));
    Ok(app)
}

/// `show_scope_name` deep inside main(): a long scope stack.
fn scope_popup_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    let row = MAIN_RS
        .lines()
        .position(|l| l.contains("{step:>2}"))
        .unwrap_or(0);
    let column = MAIN_RS
        .lines()
        .nth(row)
        .and_then(|l| l.find("step:"))
        .unwrap_or(0);
    select(&mut app, &[(row, column + 2, row, column + 2)]);
    settle(&mut app);
    named(&mut app, "show_scope_name", json!({}));
    Ok(app)
}

/// Many fitted tabs with two-letter names, some dirty, tab scrolling off.
fn narrow_tabs_scene(fixture: &Fixture, dark: bool, ui: f64) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    for name in ["src/lib.rs", "src/config.rs", "README.md", "notes.txt"] {
        open(&mut app, &fixture.path(name))?;
    }
    for (i, name) in ["ab", "db", "ok", "x", "go", "id", "ui", "to", "of", "up"]
        .into_iter()
        .enumerate()
    {
        named(&mut app, "new_file", json!({}));
        // Every third one stays empty ("Untitled", clean); the others are dirty.
        if i % 3 != 0 {
            named(&mut app, "insert", json!({"characters": name}));
        }
    }
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    for (key, value) in [
        ("enable_tab_scrolling", json!(false)),
        ("ui_scale", json!(ui)),
    ] {
        named(
            &mut app,
            "set_setting",
            json!({"setting": key, "value": value}),
        );
    }
    Ok(app)
}

/// Benchmark: one 12 KB line with a no-break space and 3,000 selections, no wrap.
fn selections_3000_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    named(&mut app, "new_file", json!({}));
    let mut line = "word ".repeat(2400);
    line.replace_range(6000..6001, "\u{a0}");
    named(&mut app, "insert", json!({"characters": line}));
    for (key, value) in [
        ("word_wrap", json!(false)),
        ("draw_unicode_white_space", json!("all")),
    ] {
        set_view(&mut app, key, value);
    }
    settle(&mut app);
    let regions = (0..3000).map(|i| vel_buffer::Region::new(i * 4, i * 4 + 2));
    app.workspace
        .editor_mut()
        .set_selection(vel_buffer::Selection::from_regions(regions));
    Ok(app)
}

/// Benchmark: 300 rows with four selections each and one inline phantom.
fn phantom_selections_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    named(&mut app, "new_file", json!({}));
    let text = "let value = compute(alpha, beta, gamma);\n".repeat(300);
    named(&mut app, "insert", json!({"characters": text}));
    settle(&mut app);
    let editor = app.workspace.editor_mut();
    let _ = editor.run(
        "add_phantoms",
        &json!({"key":"hint","items":[{"at":20,"layout":"inline","content":"<b>x:</b>"}]}),
    );
    let regions = (0..300).flat_map(|row| {
        let start = row * 41;
        [4, 12, 20, 33].map(|at| vel_buffer::Region::new(start + at, start + at + 3))
    });
    editor.set_selection(vel_buffer::Selection::from_regions(regions));
    Ok(app)
}

fn sidebar_folders_only_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = sidebar_scene(fixture, dark)?;
    named(&mut app, "toggle_show_open_files", json!({}));
    Ok(app)
}

fn hover_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    Ok(app)
}

fn hover_definition(app: &mut App) {
    // Point at the `visit` call inside schedule(); its definition is in the same file.
    let row = MAIN_RS
        .lines()
        .position(|l| l.contains("visit(task, &by_name"))
        .unwrap_or(0);
    let column = MAIN_RS
        .lines()
        .nth(row)
        .and_then(|l| l.find("visit"))
        .unwrap_or(0);
    let editor = app.workspace.editor();
    let byte = editor.text.line_to_byte(row) + column + 2;
    let rect = app.view.rect_at(editor, byte);
    let point = Point {
        x: app.content.x + rect.x + 2.,
        y: app.content.y + rect.y + rect.height / 2.,
    };
    pointer(app, MouseKind::Move, point.x, point.y);
    // Past the 250 ms hover delay, before the 2 s give-up window.
    app.panels.hover_since = Instant::now() - Duration::from_millis(600);
}

fn phantoms_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    let line = |needle: &str| {
        MAIN_RS
            .lines()
            .position(|l| l.contains(needle))
            .unwrap_or(0)
    };
    let editor = app.workspace.editor_mut();
    let task_new = editor.text.line_to_byte(line("Task::new(\"fetch\""));
    let visit = editor.text.line_to_byte(line("fn visit<'a>"));
    editor.run("add_phantoms", &json!({"key":"design","items":[
        {"at": task_new + 18, "layout":"inline", "content":"<span style='color:var(--foreground);background-color:color(var(--foreground) alpha(0.08));padding:0 4px;border-radius:3px'>priority:</span>"},
        {"at": visit, "layout":"below", "content":"<div style='padding:4px 8px;border-left:2px solid var(--bluish)'>Recursive helper · <a href='subl:hide_popup'>Show callers</a></div>"}
    ]}))?;
    editor.run(
        "add_annotations",
        &json!({"key":"design","annotation_color":"var(--yellowish)","items":[
            {"at": editor.text.line_to_byte(line("visiting.pop()")), "content":"<small><b>Note</b> · pop after recursion</small>"}
        ]}),
    )?;
    let path = fixture.file.clone();
    let error_row = line("let next = by_name");
    app.builds.diagnostics.push(vel_build::Diagnostic {
        path,
        line: error_row + 1,
        column: 20,
        message: "mismatched types: expected `&Task`, found `Option<&&Task>`".into(),
        output_line: 0,
    });
    let editor = app.workspace.editor_mut();
    let at = editor.text.line_to_byte(line("println!(\"{step"));
    editor.set_caret(at + 16, false);
    editor.run("show_popup", &json!({"max_width": 420, "content": "<h3>println!</h3><p>Prints to the standard output, with a newline.</p><p><code>println!(\"{}\", value)</code> · <a href='subl:hide_popup'>Close</a></p>"}))?;
    Ok(app)
}

fn status_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    for (key, value) in [("show_encoding", true), ("show_line_endings", true)] {
        app.workspace.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            key,
            json!(value),
        );
    }
    select(&mut app, &[(44, 12, 44, 20), (45, 8, 45, 8)]);
    inject_git(&mut app, fixture);
    Ok(app)
}

fn output_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    app.panels.configure_output(&app.workspace, None, false);
    app.panels.output = "   Compiling sample v0.1.0 (/tmp/project)\nerror[E0308]: mismatched types\n  --> src/main.rs:55:20\n   |\n55 |         let next = by_name.get(name.as_str()).ok_or(\"unknown dependency\")?;\n   |                    ^^^^^^^ expected `&Task`, found `Option<&&Task>`\n\nerror: could not compile `sample` (bin \"sample\") due to 1 previous error\n[Finished in 0.4s with exit code 101]\n".into();
    app.panels.panel = Some(crate::panels::Panel::Output);
    Ok(app)
}

fn settings_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    named(
        &mut app,
        "edit_settings",
        json!({"base_file": "${packages}/Default/Preferences.sublime-settings", "default": "{\n\t$0\n}\n"}),
    );
    let mut settings = app
        .opened_settings
        .pop()
        .ok_or_else(|| format!("settings window not opened: {}", app.panels.message))?;
    settings.workspace.set_appearance(dark)?;
    Ok(settings)
}

fn distraction_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    named(&mut app, "toggle_distraction_free", json!({}));
    Ok(app)
}

fn empty_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut workspace = Workspace::new(
        vel_text::Text::new(),
        Some(fixture.data.clone()),
        None,
        false,
    )?;
    workspace.set_appearance(dark)?;
    new_app(workspace)
}

fn narrow_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = find_scene(fixture, dark, "replace", "task")?;
    for name in ["src/lib.rs", "README.md", "Cargo.toml"] {
        open(&mut app, &fixture.path(name))?;
    }
    open(&mut app, &fixture.file)?;
    Ok(app)
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "editor",
        size: (1200., 800.),
        build: editor_scene,
    },
    Scenario {
        name: "minimap_scrollbar",
        size: (1200., 800.),
        build: minimap_scrollbar_scene,
    },
    Scenario {
        name: "minimap_hover",
        size: (1200., 800.),
        build: minimap_scrollbar_scene,
    },
    Scenario {
        name: "tabs",
        size: (1200., 800.),
        build: tabs_scene,
    },
    Scenario {
        name: "tabs_split_view",
        size: (1200., 800.),
        build: split_scene,
    },
    Scenario {
        name: "groups_columns",
        size: (1200., 800.),
        build: |f, d| groups_scene(f, d, "columns2"),
    },
    Scenario {
        name: "groups_grid",
        size: (1200., 800.),
        build: |f, d| groups_scene(f, d, "grid4"),
    },
    Scenario {
        name: "sidebar",
        size: (1200., 800.),
        build: sidebar_scene,
    },
    Scenario {
        name: "sidebar_focused",
        size: (1200., 800.),
        build: sidebar_focused_scene,
    },
    Scenario {
        name: "find",
        size: (1200., 800.),
        build: |f, d| find_scene(f, d, "find", "task"),
    },
    Scenario {
        name: "find_regex_error",
        size: (1200., 800.),
        build: |f, d| {
            let mut app = find_scene(f, d, "find", "(task")?;
            app.panels.options.regex = true;
            let viewport = app.viewport();
            app.panels.edited(&mut app.workspace, viewport);
            Ok(app)
        },
    },
    Scenario {
        name: "find_options_on",
        size: (1200., 800.),
        build: |f, d| {
            // A multi-line selection makes "In selection" relevant; four restricting
            // options are on through the same command the shortcuts use.
            let mut app = project_app(f, d)?;
            open(&mut app, &f.file)?;
            settle(&mut app);
            select(&mut app, &[(34, 0, 43, 1)]);
            named(
                &mut app,
                "show_panel",
                json!({"panel": "find", "text": "task"}),
            );
            for option in ["regex", "case_sensitive", "whole_word", "in_selection"] {
                named(&mut app, "toggle_find_option", json!({"option": option}));
            }
            Ok(app)
        },
    },
    Scenario {
        name: "incremental_find",
        size: (1200., 800.),
        build: |f, d| find_scene(f, d, "incremental_find", "visit"),
    },
    Scenario {
        name: "replace",
        size: (1200., 800.),
        build: |f, d| {
            let mut app = find_scene(f, d, "replace", "task")?;
            named(
                &mut app,
                "toggle_find_option",
                json!({"option": "preserve_case"}),
            );
            Ok(app)
        },
    },
    Scenario {
        name: "find_in_files",
        size: (1200., 800.),
        build: |f, d| find_scene(f, d, "find_in_files", "schedule"),
    },
    Scenario {
        name: "find_in_files_short",
        size: (1000., 200.),
        build: |f, d| find_scene(f, d, "find_in_files", "schedule"),
    },
    Scenario {
        name: "replace_scaled_narrow",
        size: (760., 560.),
        build: |f, d| {
            let mut app = find_scene(f, d, "replace", "task")?;
            app.workspace.editor_mut().settings.set(
                vel_config::settings::Layer::View,
                "ui_scale",
                json!(1.5),
            );
            Ok(app)
        },
    },
    Scenario {
        name: "replace_300_ui2",
        size: (300., 420.),
        build: |f, d| {
            let mut app = find_scene(f, d, "replace", "task")?;
            app.workspace.editor_mut().settings.set(
                vel_config::settings::Layer::View,
                "ui_scale",
                json!(2.),
            );
            Ok(app)
        },
    },
    Scenario {
        name: "command_palette",
        size: (1200., 800.),
        build: |f, d| overlay_scene(f, d, "command_palette", "toggle"),
    },
    Scenario {
        name: "goto_anything",
        size: (1200., 800.),
        build: |f, d| overlay_scene(f, d, "goto", "rs"),
    },
    Scenario {
        name: "goto_symbol",
        size: (1200., 800.),
        build: |f, d| overlay_scene(f, d, "goto", "@"),
    },
    Scenario {
        name: "scheme_picker",
        size: (1200., 800.),
        build: |f, d| picker_scene(f, d, "select_color_scheme"),
    },
    Scenario {
        name: "theme_picker",
        size: (1200., 800.),
        build: |f, d| picker_scene(f, d, "select_theme"),
    },
    Scenario {
        name: "completion",
        size: (1200., 800.),
        build: completion_scene,
    },
    Scenario {
        name: "hover_definition",
        size: (1200., 800.),
        build: hover_scene,
    },
    Scenario {
        name: "phantoms_popup_build_errors",
        size: (1200., 800.),
        build: phantoms_scene,
    },
    Scenario {
        name: "status_bar",
        size: (1200., 800.),
        build: status_scene,
    },
    Scenario {
        name: "status_narrow",
        size: (520., 340.),
        build: status_scene,
    },
    Scenario {
        name: "quick_panel_many",
        size: (1200., 800.),
        build: |f, d| overlay_scene(f, d, "command_palette", ""),
    },
    Scenario {
        name: "overlay_small",
        size: (420., 300.),
        build: |f, d| overlay_scene(f, d, "goto", "rs"),
    },
    Scenario {
        name: "completion_short",
        size: (1000., 280.),
        build: completion_short_scene,
    },
    Scenario {
        name: "completion_ui2",
        size: (1200., 800.),
        build: completion_ui2_scene,
    },
    Scenario {
        name: "image_view",
        size: (1000., 640.),
        build: |f, d| image_scene(f, d, "logo.png"),
    },
    Scenario {
        name: "image_fallback",
        size: (1000., 640.),
        build: |f, d| image_scene(f, d, "damaged.png"),
    },
    Scenario {
        name: "invisibles",
        size: (1000., 480.),
        build: invisibles_scene,
    },
    Scenario {
        name: "scope_popup",
        size: (1200., 800.),
        build: scope_popup_scene,
    },
    Scenario {
        name: "narrow_tabs",
        size: (640., 400.),
        build: |f, d| narrow_tabs_scene(f, d, 1.),
    },
    Scenario {
        name: "narrow_tabs_ui2",
        size: (1200., 500.),
        build: |f, d| narrow_tabs_scene(f, d, 2.),
    },
    Scenario {
        name: "sidebar_narrow",
        size: (320., 520.),
        build: sidebar_scene,
    },
    Scenario {
        name: "sidebar_folders_only",
        size: (1000., 560.),
        build: sidebar_folders_only_scene,
    },
    Scenario {
        name: "selections_3000",
        size: (1200., 800.),
        build: selections_3000_scene,
    },
    Scenario {
        name: "phantom_selections",
        size: (1200., 800.),
        build: phantom_selections_scene,
    },
    Scenario {
        name: "build_output",
        size: (1200., 800.),
        build: output_scene,
    },
    Scenario {
        name: "settings_window",
        size: (1200., 800.),
        build: settings_scene,
    },
    Scenario {
        name: "distraction_free",
        size: (1200., 800.),
        build: distraction_scene,
    },
    Scenario {
        name: "empty_window",
        size: (1200., 800.),
        build: empty_scene,
    },
    Scenario {
        name: "narrow_640x480",
        size: (640., 480.),
        build: narrow_scene,
    },
    Scenario {
        name: "large_1920x1200",
        size: (1920., 1200.),
        build: |f, d| {
            let mut app = tabs_scene(f, d)?;
            app.panels.side_visible = true;
            Ok(app)
        },
    },
];

/// Pointer-driven states are applied after layout because hit testing needs real geometry.
fn after_layout(name: &str, app: &mut App) {
    match name {
        "editor" => editor_hover_gutter(app),
        "minimap_scrollbar" => editor_hover_scrollbar(app),
        "minimap_hover" => {
            // Rest the pointer on the minimap so its viewport box shows.
            let editor = app.workspace.editor();
            let x = app.content.x + app.view.right(editor) + app.view.minimap_width(editor) / 2.;
            pointer(app, MouseKind::Move, x, app.content.y + 60.);
        }
        "status_bar" | "status_narrow" => {
            // Hover the syntax item (the rightmost one): status items are buttons and
            // must look clickable.
            // Their widgets come from the previous frame's measured layout.
            let _ = app.scene();
            let size = app.window_size;
            pointer(app, MouseKind::Move, size.width - 24., size.height - 12.);
        }
        "tabs" | "large_1920x1200" => {
            // Hover an inactive tab, then rest the pointer on another tab's close button.
            let hits: Vec<_> = app.tab_hits.clone();
            if let Some((_, rect)) = hits.get(1) {
                pointer(app, MouseKind::Move, rect.x + rect.width / 2., rect.y + 10.);
            }
        }
        "sidebar" => {
            let rows: Vec<_> = app
                .panels
                .tree
                .nodes
                .values()
                .filter(|n| n.visible && n.class == "tree_row")
                .map(|n| n.rect)
                .collect();
            if let Some(rect) = rows.get(4) {
                pointer(
                    app,
                    MouseKind::Move,
                    rect.x + 60.,
                    rect.y + rect.height / 2.,
                );
            }
        }
        // Hover one control per find panel and show its tool tip.
        "find" | "find_options_on" | "replace" | "find_in_files" | "incremental_find" => {
            let id = match name {
                "find" => 31,
                "find_options_on" => 50,
                "replace" => 52,
                "find_in_files" => 30,
                _ => 32,
            };
            let rect = app.panels.tree.nodes.get(&id).map(|n| n.rect);
            if let Some(rect) = rect {
                pointer(
                    app,
                    MouseKind::Move,
                    rect.x + rect.width / 2.,
                    rect.y + rect.height / 2.,
                );
            }
            if let Some(tooltip) = &mut app.panels.review.tooltip {
                tooltip.2 = Instant::now() - Duration::from_secs(1);
            }
        }
        "hover_definition" => hover_definition(app),
        _ => {}
    }
}

/// Polls workers until language, panels, completion and annotation state is published.
/// Only the visible sheets must be highlighted; background tabs may stay lazy.
fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut quiet = 0;
    loop {
        // A dirty document reports a change on every poll; rely on explicit busy flags.
        let mut reasons = Vec::new();
        app.workspace.poll();
        let viewport = app.viewport();
        app.panels.poll(&mut app.workspace, viewport);
        if app.workspace.themes.ui.borrow().metrics.poll_images() {
            reasons.push("images changed".into());
        }
        app.sync_active_view();
        if app.workspace.resources_busy() {
            reasons.push("resources".into());
        }
        // An open overlay always reports busy; only its ranking must have published.
        let overlay = app.panels.overlay.take();
        let panels_busy = app.panels.busy();
        app.panels.overlay = overlay;
        if panels_busy || app.panels.ranking_pending() {
            reasons.push("panels busy".into());
        }
        if app.workspace.themes.ui.borrow().metrics.images_pending() {
            reasons.push("images pending".into());
        }
        let visible: Vec<u64> = app
            .workspace
            .workbench
            .groups
            .iter()
            .flat_map(|g| g.selected.iter().copied())
            .collect();
        for tab in &app.workspace.tabs {
            if !visible.contains(&tab.sheet) {
                continue;
            }
            let editor = &tab.editor;
            if tab.loading.is_some() || tab.binary_loading.is_some() {
                reasons.push(format!("{} loading", tab.label()));
            }
            if editor.completion.busy() {
                reasons.push("completion".into());
            }
            if editor.annotations.borrow().pending > 0 {
                reasons.push("annotations".into());
            }
            if editor
                .language
                .borrow()
                .as_ref()
                .is_some_and(|l| l.busy() || !(l.ready() && l.highlight_complete()))
            {
                reasons.push(format!("{} highlighting", tab.label()));
            }
        }
        if app.view.image_preview.busy() {
            reasons.push("image decode".into());
        }
        if reasons.is_empty() {
            quiet += 1;
            if quiet >= 3 {
                return;
            }
        } else {
            quiet = 0;
        }
        if Instant::now() > deadline {
            eprintln!("velocity: render-scenes: still busy after 20 s: {reasons:?}");
            return;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// Mirrors `on_start` for a window of `size` at `scale`, without creating the window.
fn attach(app: &mut App, fonts: Fonts, size: Size, scale: f64) -> Result<(), String> {
    app.view = View::new(size, fonts.cell as f64, fonts.ascent as f64);
    app.view.set_font_metrics(fonts.metrics, scale);
    app.window_size = size;
    app.workspace.themes.ui.borrow_mut().density = scale as f32;
    app.view.appearance = if app.workspace.dark {
        Appearance::Dark
    } else {
        Appearance::Light
    };
    app.workspace
        .themes
        .ui
        .borrow_mut()
        .metrics
        .enable_native()?;
    app.font_face = app.workspace.editor().preferences.font_face.clone();
    app.fonts = Some(fonts);
    app.initialize_view_metrics();
    app.view_buffer = None;
    app.view.reveal(app.workspace.editor());
    Ok(())
}

fn render(
    app: &mut App,
    renderer: &mut Renderer,
    scale: f32,
) -> Result<(Vec<u8>, vel_render::scene::Scene), String> {
    // Theme transitions are disabled by the fixture's settings, but a hover may still be
    // mid-flight for themes that force animation; let it settle.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut scene = app.scene();
    while app.workspace.themes.ui.borrow().animating() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        scene = app.scene();
    }
    let fonts = app.fonts.as_mut().ok_or("fonts missing")?;
    let mut pixels = renderer.render_rgba(&scene, fonts, scale)?;
    // Fallback faces (CJK, emoji, symbols) load asynchronously; re-render once they land.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let fonts = app.fonts.as_mut().ok_or("fonts missing")?;
        if fonts.poll_fallbacks() {
            renderer.invalidate_layouts();
            pixels = renderer.render_rgba(&scene, fonts, scale)?;
            continue;
        }
        if !fonts.fallback_pending() || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok((pixels, scene))
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())
}

/// Instructions retired by this process so far (all threads).
fn instructions() -> u64 {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: proc_pid_rusage writes one rusage_info_v4 for RUSAGE_INFO_V4 into a buffer of
    // exactly that size; the pointer cast matches its documented `rusage_info_t *` parameter.
    let status = unsafe {
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast::<libc::rusage_info_t>(),
        )
    };
    if status != 0 {
        return 0;
    }
    // SAFETY: the call succeeded and initialized the structure.
    unsafe { info.assume_init() }.ri_instructions
}

/// Steady-state cost of `App::scene` (CPU scene construction, no GPU): warms caches, then
/// reports mean instructions and wall time per call over `count` calls.
fn bench_scene(app: &mut App, name: &str, dark: bool, scale: f32, count: usize) {
    for _ in 0..20 {
        std::hint::black_box(app.scene());
    }
    let (start, at) = (instructions(), Instant::now());
    for _ in 0..count {
        std::hint::black_box(app.scene());
    }
    let (spent, elapsed) = (instructions() - start, at.elapsed());
    println!(
        "{{\"kind\":\"scene_bench\",\"scenario\":\"{name}\",\"appearance\":\"{}\",\"scale\":{scale},\"calls\":{count},\"instructions_per_scene\":{},\"us_per_scene\":{:.1}}}",
        if dark { "dark" } else { "light" },
        spent / count as u64,
        elapsed.as_secs_f64() * 1e6 / count as f64
    );
}

/// Entry point for `velocity --render-scenes`.
pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let request = Request::parse(args)?;
    std::fs::create_dir_all(&request.out).map_err(|e| e.to_string())?;
    let fixture = Fixture::create()?;
    let mut renderer: Option<Renderer> = None;
    let mut written = 0;
    for scenario in SCENARIOS {
        if request
            .only
            .as_ref()
            .is_some_and(|only| !only.iter().any(|n| n == scenario.name))
        {
            continue;
        }
        for &dark in &request.dark {
            for &scale in &request.scales {
                let mut app = (scenario.build)(&fixture, dark)?;
                let size = Size {
                    width: scenario.size.0,
                    height: scenario.size.1,
                };
                let fonts = Fonts::load_named(&app.workspace.editor().settings.editor().font_face)?;
                attach(&mut app, fonts, size, f64::from(scale))?;
                if request.first_frames > 0 {
                    let width = (size.width * f64::from(scale)).round() as u32;
                    let height = (size.height * f64::from(scale)).round() as u32;
                    let renderer = match renderer.as_mut() {
                        Some(r) => {
                            r.reset_fonts();
                            r.resize(width, height);
                            r
                        }
                        None => renderer.insert(Renderer::offscreen(width, height)?),
                    };
                    for frame in 0..request.first_frames {
                        if frame > 0 {
                            app.workspace.poll();
                            let viewport = app.viewport();
                            app.panels.poll(&mut app.workspace, viewport);
                        }
                        let (pixels, _) = render(&mut app, renderer, scale)?;
                        let file = request.out.join(format!(
                            "{}-{}@{}x-frame{}.png",
                            scenario.name,
                            if dark { "dark" } else { "light" },
                            scale,
                            frame + 1
                        ));
                        write_png(&file, width, height, &pixels)?;
                        written += 1;
                        println!("{}", file.display());
                        std::thread::sleep(Duration::from_millis(16));
                    }
                    continue;
                }
                settle(&mut app);
                let _ = app.scene();
                after_layout(scenario.name, &mut app);
                settle(&mut app);
                if scenario.name == "hover_definition" {
                    hover_definition(&mut app);
                    settle(&mut app);
                }
                if request.bench > 0 {
                    bench_scene(&mut app, scenario.name, dark, scale, request.bench);
                    continue;
                }
                let width = (size.width * f64::from(scale)).round() as u32;
                let height = (size.height * f64::from(scale)).round() as u32;
                let renderer = match renderer.as_mut() {
                    Some(r) => {
                        // Glyph and layout caches are keyed by this scene's font faces.
                        r.reset_fonts();
                        r.resize(width, height);
                        r
                    }
                    None => renderer.insert(Renderer::offscreen(width, height)?),
                };
                let (pixels, scene) = render(&mut app, renderer, scale)?;
                // VELOCITY_RENDER_DEBUG=1 lists every painted icon and text run for geometry checks.
                if std::env::var_os("VELOCITY_RENDER_DEBUG").is_some() {
                    for (i, layer) in scene.layers.iter().enumerate() {
                        for surface in &layer.surfaces {
                            if let vel_render::scene::Surface::Image(q) = surface {
                                eprintln!(
                                    "layer {i} image {:?} clip {:?} color {:?} src {:?}",
                                    q.rect, q.clip, q.color, q.source
                                );
                            }
                        }
                    }
                    for (i, layer) in scene.layers.iter().enumerate() {
                        for run in &layer.text {
                            eprintln!(
                                "layer {i} text {:?} at ({}, {}) clip {:?} color {:?}",
                                run.text, run.x, run.baseline, run.clip, run.color
                            );
                        }
                    }
                }
                let file = request.out.join(format!(
                    "{}-{}@{}x.png",
                    scenario.name,
                    if dark { "dark" } else { "light" },
                    scale
                ));
                write_png(&file, width, height, &pixels)?;
                if !app.workspace.messages.is_empty() || !app.panels.message.is_empty() {
                    eprintln!(
                        "{}: messages {:?} / {:?}",
                        scenario.name, app.workspace.messages, app.panels.message
                    );
                }
                written += 1;
                println!("{}", file.display());
            }
        }
    }
    if let Err(e) = std::fs::remove_dir_all(&fixture.root) {
        eprintln!("velocity: could not remove {}: {e}", fixture.root.display());
    }
    eprintln!("velocity: rendered {written} scenes");
    Ok(())
}
