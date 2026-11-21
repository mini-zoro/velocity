//! Developer tool: renders real application states offscreen to PNG files.
//!
//! `velocity --render-scenes <out-dir> [--scale 1|2] [--appearance dark|light] [--only a,b]
//! [--language <tag>] [--catalog <file>] [--audit-text]`
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
#[path = "render_scene_text.rs"]
mod text_audit;

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
    /// `--language <tag>`: the `ui_language` of the fixture's User preferences, so every
    /// scene starts in that language exactly as a user's setting would make it.
    language: Option<String>,
    catalogs: Vec<PathBuf>,
    audit_text: bool,
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
            language: None,
            catalogs: Vec::new(),
            audit_text: false,
        };
        while let Some(arg) = args.next() {
            if arg == "--audit-text" {
                request.audit_text = true;
                continue;
            }
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
                "--language" => {
                    vel_i18n::Language::parse(value).ok_or("--language needs a locale tag")?;
                    request.language = Some(value.clone());
                }
                "--catalog" => request.catalogs.push(PathBuf::from(value)),
                _ => return Err(format!("unknown --render-scenes option {arg}")),
            }
        }
        if request.audit_text
            && (request.language.is_none() || request.first_frames > 0 || request.bench > 0)
        {
            return Err(
                "--audit-text requires --language <tag> without a frame or bench mode".into(),
            );
        }
        Ok(request)
    }
}

/// Synthetic project on disk. Every name is generic; nothing comes from the user's machine.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    language: Option<String>,
    project: PathBuf,
    file: PathBuf,
    /// Round 5: the sample folder plus a folder of documents with Chinese names (`文档`).
    r5_project: PathBuf,
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

/// Documents with Chinese names for the round-5 scenes, in their own project folder (`文档`)
/// so the side bar and Goto results of the other scenes stay as they were.
const R5_DOCS: &[(&str, &str)] = &[
    (
        "说明.md",
        "# 示例项目\n\n一个小型任务调度器，用于渲染界面场景。\n",
    ),
    ("更新日志.md", "## 0.1.0\n\n- 第一个版本。\n"),
    ("配置.toml", "[editor]\nfont_size = 13\n"),
    ("设计/界面草图说明（第二版）.md", "# 界面草图\n"),
];

impl Fixture {
    fn create(language: Option<&str>, catalogs: &[PathBuf]) -> Result<Self, String> {
        let dir = tempfile::Builder::new()
            .prefix("velocity-render-scenes-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        // Workspace folders are canonical (/private/var/… on macOS).
        let root = dir.path().canonicalize().map_err(|e| e.to_string())?;
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
        for file in catalogs {
            let name = file
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("--catalog needs a UTF-8 filename")?;
            let tag = name
                .strip_suffix(".sublime-localization")
                .and_then(vel_i18n::Language::parse)
                .filter(|tag| !tag.is_english())
                .ok_or("--catalog needs a non-English <tag>.sublime-localization file")?;
            let dest = data.join("Packages/User/Localization");
            std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            std::fs::copy(
                file,
                dest.join(format!("{}.sublime-localization", tag.as_str())),
            )
            .map_err(|e| format!("{}: {e}", file.display()))?;
        }
        // Deterministic chrome: no tree/hover animations, no blinking; the UI language as a
        // user's preference would set it.
        let language_setting = json!(language.unwrap_or("en"));
        std::fs::write(
            data.join("Packages/User/Preferences.sublime-settings"),
            format!(
                "{{\n\t\"animation_enabled\": false,\n\t\"tree_animation_enabled\": false,\n\t\"caret_style\": \"solid\",\n\t\"ui_language\": {language_setting}\n}}\n"
            ),
        )
        .map_err(|e| e.to_string())?;
        for (name, contents) in R5_DOCS {
            let path = root.join("文档").join(name);
            std::fs::create_dir_all(path.parent().expect("parent")).map_err(|e| e.to_string())?;
            std::fs::write(&path, contents).map_err(|e| e.to_string())?;
        }
        let r5_project = root.join("r5.sublime-project");
        std::fs::write(
            &r5_project,
            r#"{"folders":[{"path":"project"},{"path":"文档"}]}"#,
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
            scratch.join("preview.webp"),
            crate::image_decode::WEBP_FIXTURE,
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(
            scratch.join("damaged.png"),
            b"\x89PNG\r\n\x1a\nnot really an image",
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(scratch.join("invisible.rs"), INVISIBLE_RS).map_err(|e| e.to_string())?;
        let project = root.join("sample.sublime-project");
        std::fs::write(&project, r#"{"folders":[{"path":"project"}]}"#)
            .map_err(|e| e.to_string())?;
        let inventory = data.join("Packages/ScenePackage");
        std::fs::create_dir_all(&inventory).map_err(|e| e.to_string())?;
        std::fs::write(inventory.join("Example.sublime-menu"), "[]").map_err(|e| e.to_string())?;
        std::fs::write(inventory.join("plugin.py"), "# Not executed\n")
            .map_err(|e| e.to_string())?;
        Ok(Self {
            _dir: dir,
            file: project_dir.join("src/main.rs"),
            root,
            data,
            language: language.map(str::to_owned),
            project,
            r5_project,
        })
    }
    fn path(&self, name: &str) -> PathBuf {
        self.root.join("project").join(name)
    }
    fn docs(&self, name: &str) -> PathBuf {
        self.root.join("文档").join(name)
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

fn chinese_scene(fixture: &Fixture, dark: bool, scale: f64, query: &str) -> Result<App, String> {
    let mut app = find_scene(fixture, dark, "find_in_files", "task")?;
    // Keep the original Chinese screenshots when no language was requested.
    if fixture.language.is_none() {
        app.workspace.settings.set(
            vel_config::settings::Layer::User,
            "ui_language",
            json!("zh-Hans"),
        );
        app.workspace.sync_ui_language();
    }
    app.workspace.editor_mut().settings.set(
        vel_config::settings::Layer::View,
        "ui_scale",
        json!(scale),
    );
    named(
        &mut app,
        "show_overlay",
        json!({"overlay":"command_palette","text":query}),
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

fn package_inventory_scene(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = project_app(fixture, dark)?;
    let items = crate::release::inventory_scene(&app.workspace)?;
    app.panels.choices(&mut app.workspace, items);
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
    if name == "preview.webp" && app.view.image_preview.image.is_none() {
        return Err("image_webp: the preview did not decode".into());
    }
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
    app.panels.output = "   Compiling sample v0.1.0 (/tmp/project)\nerror[E0308]: mismatched types\n  --> src/main.rs:55:20\n   |\n55 |         let next = by_name.get(name.as_str()).ok_or(\"unknown dependency\")?;\n   |                    ^^^^^^^ expected `&Task`, found `Option<&&Task>`\n\nerror: could not compile `sample` (bin \"sample\") due to 1 previous error\n".into();
    app.panels
        .append_output_ui(crate::build::finish_line_with_signal(
            Some(101),
            None,
            false,
            Duration::from_millis(400),
            false,
        ));
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

fn settings_ui_scene(fixture: &Fixture, dark: bool, variant: &str) -> Result<App, String> {
    // The broken-file scene must not affect the render fixture for any other scene.
    let support = fixture.root.join("Library/Application Support");
    std::fs::create_dir_all(&support).map_err(|e| e.to_string())?;
    let data = tempfile::Builder::new()
        .prefix("Velocity-")
        .tempdir_in(&support)
        .map_err(|e| e.to_string())?
        .keep();
    let path = crate::workbench::preferences_path(&data);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let original = crate::workbench::user_preferences_source(&crate::workbench::preferences_path(
        &fixture.data,
    ))?;
    let mut settings = vel_config::json::Document::parse(&original)?.value;
    settings["tab_size"] = json!(8);
    settings["translate_tabs_to_spaces"] = json!(true);
    settings["word_separators"] = json!(":/.");
    if variant == "array" {
        settings["file_exclude_patterns"] = json!([
            "*.log", "*.bak", "*.o", "*.pyc", "*.swp", "*.cache", "*.tmp"
        ]);
    } else if variant == "dependent" {
        settings["word_wrap"] = json!(true);
        settings["wrap_width"] = json!(96);
    }
    std::fs::write(&path, settings.to_string()).map_err(|e| e.to_string())?;
    let mut workspace = Workspace::new(vel_text::Text::new(), Some(data), None, false)?;
    workspace.set_appearance(dark)?;
    let mut app = new_app(workspace)?;
    if variant == "broken" {
        std::fs::write(&path, "{\n  \"tab_size\": ,\n}\n").map_err(|e| e.to_string())?;
    }
    named(&mut app, "show_settings", json!({}));
    let mut page = app
        .workspace
        .settings_page
        .take()
        .ok_or("Settings sheet not opened")?;
    page.use_fixture_home(fixture.root.clone());
    match variant {
        "modified" => page.show_modified(),
        "search" => page.search_for(
            if app.workspace.translations.matched_language().as_str() == "zh-Hans" {
                "换行"
            } else {
                "wrap"
            },
        ),
        "enum" | "array" | "guides" | "dependent" => {
            page.search_for(match variant {
                "enum" => "word_wrap",
                "array" => "file_exclude_patterns",
                "guides" => "indent_guide_options",
                _ => "wrap_width",
            });
            page.command(
                &mut app.workspace,
                vel_platform::Command::Newline,
                Instant::now(),
            );
            if variant != "dependent" {
                page.command(
                    &mut app.workspace,
                    vel_platform::Command::Newline,
                    Instant::now(),
                );
            }
        }
        _ => {}
    }
    app.workspace.settings_page = Some(page);
    Ok(app)
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

/// Round 5: the sample folder and the Chinese-named documents in one project.
fn r5_app(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut workspace = Workspace::new(
        vel_text::Text::new(),
        Some(fixture.data.clone()),
        Some(fixture.r5_project.clone()),
        false,
    )?;
    workspace.set_appearance(dark)?;
    let mut app = new_app(workspace)?;
    app.panels.side_visible = true;
    Ok(app)
}

/// Two Chinese-named documents and `main.rs` in front, caret on line 41.
fn r5_editor(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = r5_app(fixture, dark)?;
    for name in ["说明.md", "更新日志.md"] {
        open(&mut app, &fixture.docs(name))?;
    }
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    select(&mut app, &[(40, 0, 40, 0)]);
    Ok(app)
}

fn r5_overlay(fixture: &Fixture, dark: bool, overlay: &str, text: &str) -> Result<App, String> {
    let mut app = r5_editor(fixture, dark)?;
    named(
        &mut app,
        "show_overlay",
        json!({"overlay": overlay, "text": text}),
    );
    Ok(app)
}

fn r5_find(fixture: &Fixture, dark: bool, panel: &str) -> Result<App, String> {
    let mut app = r5_editor(fixture, dark)?;
    named(&mut app, "show_panel", json!({"panel": panel}));
    if let Some(input) = app.panels.input_mut() {
        input.set("task");
    }
    let viewport = app.viewport();
    app.panels.edited(&mut app.workspace, viewport);
    if panel == "find_in_files" {
        app.panels.where_input.set("<project>, *.rs");
    }
    Ok(app)
}

/// Side bar with the Chinese documents expanded, an unsaved Chinese document and the
/// status bar.
fn r5_sidebar(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = r5_editor(fixture, dark)?;
    open(&mut app, &fixture.docs("说明.md"))?;
    named(&mut app, "insert", json!({"characters": "草稿\n"}));
    settle(&mut app);
    app.panels.reveal_sidebar_path(
        &mut app.workspace,
        fixture.docs("设计/界面草图说明（第二版）.md"),
    )?;
    settle(&mut app);
    Ok(app)
}

/// The tab strip under `file_tab_style` and `show_tab_close_buttons_on_left`, with a
/// modified tab so the dirty marker's side shows too.
fn r26_tabs(fixture: &Fixture, dark: bool, rounded: bool, left: bool) -> Result<App, String> {
    let mut app = r5_editor(fixture, dark)?;
    open(&mut app, &fixture.docs("说明.md"))?;
    named(&mut app, "insert", json!({"characters": "草稿\n"}));
    open(&mut app, &fixture.file)?;
    for (key, value) in [
        (
            "file_tab_style",
            json!(if rounded { "rounded" } else { "square" }),
        ),
        ("show_tab_close_buttons_on_left", json!(left)),
    ] {
        let e = app.workspace.editor_mut();
        e.settings
            .set(vel_config::settings::Layer::View, key, value);
        e.sync();
    }
    settle(&mut app);
    Ok(app)
}

fn r25_status_selection(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = r5_editor(fixture, dark)?;
    let editor = app.workspace.editor_mut();
    let end = editor.text.char_to_byte(6);
    editor.set_selection(vel_buffer::Selection::from_regions([
        vel_buffer::Region::new(0, end),
    ]));
    Ok(app)
}

fn r25_build_output(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = r5_editor(fixture, dark)?;
    // The output view exists once a build configures it, as `build::start` does.
    app.panels.configure_output(&app.workspace, None, false);
    app.panels.output = "Compiling demo.rs\nwarning: unused variable: sample\n".into();
    app.panels
        .append_output_ui(crate::build::finish_line_with_signal(
            Some(0),
            None,
            false,
            Duration::from_millis(1200),
            false,
        ));
    app.panels.panel = Some(crate::panels::Panel::Output);
    Ok(app)
}

fn r25_ui2(mut app: App) -> Result<App, String> {
    set_view(&mut app, "ui_scale", json!(2.));
    Ok(app)
}

fn r6_cjk(fixture: &Fixture, dark: bool) -> Result<App, String> {
    let mut app = r5_app(fixture, dark)?;
    named(&mut app, "new_file", json!({}));
    let paragraph = "小雨已经停了，街角的书店亮起了灯。路过的人推开门，在窗前读完了这一页。";
    let text = format!(
        "雨后的书店\n\n{paragraph}\n\n读者 reader 说：東京、서울，明天见！\n书架\t索引  index\n新书 📚、咖啡 ☕、散步 👩‍💻\n\n    {paragraph}{paragraph}{paragraph}\n"
    );
    named(&mut app, "insert", json!({"characters": text}));
    for (key, value) in [
        ("word_wrap", json!(true)),
        ("wrap_width", json!(72)),
        ("indent_subsequent_lines", json!(true)),
        ("draw_white_space", json!("all")),
        ("rulers", json!([72])),
    ] {
        set_view(&mut app, key, value);
    }
    let a = text.find("東京").expect("fixture");
    let b = text.find("明天").expect("fixture");
    app.workspace
        .editor_mut()
        .set_selection(vel_buffer::Selection::from_regions([
            vel_buffer::Region::new(a, b),
        ]));
    settle(&mut app);
    Ok(app)
}

/// The input panel (Side Bar › Rename…) for a Chinese file name, through the side bar's own
/// command (its caption and initial text); the scene has no window for the native parts.
fn r5_input_panel(fixture: &Fixture, dark: bool) -> Result<App, String> {
    struct NoPlatform;
    impl crate::context_menu::SidebarPlatform for NoPlatform {
        fn trash(&mut self, _: &Path) -> Result<(), String> {
            Err("no trash in scenes".into())
        }
        fn copy_text(&mut self, _: &str) -> Result<(), String> {
            Err("no clipboard in scenes".into())
        }
        fn reveal(&mut self, _: &Path) -> Result<(), String> {
            Err("no file browser in scenes".into())
        }
    }
    let mut app = r5_editor(fixture, dark)?;
    let path = fixture.docs("说明.md");
    let roots: Vec<PathBuf> = app
        .workspace
        .folders
        .iter()
        .map(|folder| folder.path.clone())
        .collect();
    let effect = crate::context_menu::sidebar_command(
        "side_bar_rename",
        &json!({"paths": [path.display().to_string()]}),
        &roots,
        &mut NoPlatform,
    )?;
    let Some(crate::context_menu::SidebarEffect::Input(invocation, caption)) = effect else {
        return Err("side_bar_rename did not ask for a name".into());
    };
    let viewport = app.viewport();
    app.panels.show_input_caption(
        &mut app.workspace,
        &invocation.command,
        &invocation.args,
        caption,
        viewport,
    )?;
    Ok(app)
}

/// A 440 × 360 window: eight tabs (four Chinese names), the command palette on long rows.
fn r5_narrow(fixture: &Fixture, dark: bool, query: &str) -> Result<App, String> {
    let mut app = r5_app(fixture, dark)?;
    for name in ["src/lib.rs", "README.md", "Cargo.toml"] {
        open(&mut app, &fixture.path(name))?;
    }
    for (name, _) in R5_DOCS {
        open(&mut app, &fixture.docs(name))?;
    }
    open(&mut app, &fixture.file)?;
    settle(&mut app);
    named(
        &mut app,
        "show_overlay",
        json!({"overlay": "command_palette", "text": query}),
    );
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
        name: "find_results_tab",
        size: (1200., 800.),
        build: |f, d| {
            let mut app = find_scene(f, d, "find_in_files", "schedule")?;
            named(&mut app, "find_in_files_start", json!({}));
            settle(&mut app);
            if !app.panels.file_complete || !app.workspace.tabs[app.workspace.active].find_results {
                return Err("Find Results tab did not open".into());
            }
            Ok(app)
        },
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
        name: "zh_hans",
        size: (1200., 800.),
        build: |f, d| chinese_scene(f, d, 1., "保存"),
    },
    Scenario {
        name: "zh_hans_ui2",
        size: (1200., 800.),
        build: |f, d| chinese_scene(f, d, 2., "保存"),
    },
    Scenario {
        name: "zh_hans_mixed",
        size: (1200., 800.),
        build: |f, d| chinese_scene(f, d, 2., "UTF-8"),
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
        name: "package_inventory",
        size: (1200., 800.),
        build: package_inventory_scene,
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
        name: "status_switcher",
        size: (920., 520.),
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
        name: "image_webp",
        size: (1000., 640.),
        build: |f, d| image_scene(f, d, "preview.webp"),
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
        name: "r4_multiline_replace",
        size: (1200., 800.),
        build: |f, d| {
            // Six pasted lines in the field: four rows, the rest behind a scroll cue.
            let mut app = find_scene(f, d, "replace", "task")?;
            if let Some(input) = app.panels.input_mut() {
                input.set("task\nvisit\nrender\ncanvas\nparse\nwrite");
            }
            let viewport = app.viewport();
            app.panels.edited(&mut app.workspace, viewport);
            Ok(app)
        },
    },
    Scenario {
        name: "r4_multiline_replace_ui2",
        size: (1400., 900.),
        build: |f, d| {
            let mut app = find_scene(f, d, "replace", "task")?;
            named(
                &mut app,
                "set_setting",
                json!({"setting": "ui_scale", "value": 2}),
            );
            if let Some(input) = app.panels.input_mut() {
                input.set("task\nvisit\nrender\ncanvas\nparse\nwrite");
            }
            let viewport = app.viewport();
            app.panels.edited(&mut app.workspace, viewport);
            Ok(app)
        },
    },
    Scenario {
        name: "r4_region_focus",
        size: (1200., 800.),
        build: |f, d| {
            let mut app = sidebar_scene(f, d)?;
            app.panels.side_visible = true;
            app.scene();
            let sheet = app.workspace.tabs[app.workspace.active].sheet;
            app.set_region_focus(None, 1000 + sheet * 2);
            Ok(app)
        },
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
        name: "settings_ui",
        size: (900., 800.),
        build: |f, d| settings_ui_scene(f, d, "default"),
    },
    Scenario {
        name: "settings_ui_search",
        size: (900., 650.),
        build: |f, d| settings_ui_scene(f, d, "search"),
    },
    Scenario {
        name: "settings_ui_modified",
        size: (900., 650.),
        build: |f, d| settings_ui_scene(f, d, "modified"),
    },
    Scenario {
        name: "settings_ui_broken",
        size: (900., 650.),
        build: |f, d| settings_ui_scene(f, d, "broken"),
    },
    Scenario {
        name: "settings_ui_enum",
        size: (900., 650.),
        build: |f, d| settings_ui_scene(f, d, "enum"),
    },
    Scenario {
        name: "settings_ui_array",
        size: (900., 650.),
        build: |f, d| settings_ui_scene(f, d, "array"),
    },
    Scenario {
        name: "settings_ui_guides",
        size: (900., 650.),
        build: |f, d| settings_ui_scene(f, d, "guides"),
    },
    Scenario {
        name: "settings_ui_dependent",
        size: (900., 650.),
        build: |f, d| settings_ui_scene(f, d, "dependent"),
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
    Scenario {
        name: "r5_palette",
        size: (1200., 800.),
        build: |f, d| r5_overlay(f, d, "command_palette", "save"),
    },
    Scenario {
        name: "r5_palette_mixed",
        size: (1200., 800.),
        build: |f, d| r5_overlay(f, d, "command_palette", "rust"),
    },
    Scenario {
        name: "r5_palette_all",
        size: (1200., 800.),
        build: |f, d| r5_overlay(f, d, "command_palette", ""),
    },
    Scenario {
        name: "r5_find",
        size: (1200., 800.),
        build: |f, d| r5_find(f, d, "find"),
    },
    Scenario {
        name: "r5_find_in_files",
        size: (1200., 800.),
        build: |f, d| r5_find(f, d, "find_in_files"),
    },
    Scenario {
        name: "r5_goto_anything",
        size: (1200., 800.),
        build: |f, d| r5_overlay(f, d, "goto", "说明"),
    },
    Scenario {
        name: "r5_goto_symbol",
        size: (1200., 800.),
        build: |f, d| r5_overlay(f, d, "goto", "@"),
    },
    Scenario {
        name: "r5_sidebar",
        size: (1200., 800.),
        build: r5_sidebar,
    },
    Scenario {
        name: "r5_input_panel",
        size: (1200., 800.),
        build: r5_input_panel,
    },
    Scenario {
        name: "r25_find_ui2",
        size: (1200., 800.),
        build: |f, d| r25_ui2(r5_find(f, d, "find")?),
    },
    Scenario {
        name: "r25_find_in_files_ui2",
        size: (1200., 800.),
        build: |f, d| r25_ui2(r5_find(f, d, "find_in_files")?),
    },
    Scenario {
        name: "r25_status_selection",
        size: (1200., 800.),
        build: r25_status_selection,
    },
    Scenario {
        name: "r25_build_output",
        size: (1200., 800.),
        build: r25_build_output,
    },
    Scenario {
        name: "r5_narrow",
        size: (440., 360.),
        build: |f, d| r5_narrow(f, d, "neighbor"),
    },
    Scenario {
        name: "r6-cjk-zh",
        size: (800., 360.),
        build: r6_cjk,
    },
    Scenario {
        name: "r26-tabs-square",
        size: (800., 200.),
        build: |f, d| r26_tabs(f, d, false, false),
    },
    Scenario {
        name: "r26-tabs-rounded-left",
        size: (800., 200.),
        build: |f, d| r26_tabs(f, d, true, true),
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
                reasons.push(format!("{} loading", tab.ui_label(&app.workspace.ui)));
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
                reasons.push(format!("{} highlighting", tab.ui_label(&app.workspace.ui)));
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

fn capture_chinese_layout(
    app: &mut App,
    out: &Path,
    variant: &str,
    dark: bool,
    scale: f32,
    audit_root: Option<&Path>,
) -> Result<(), String> {
    use vel_minihtml::FontMetrics;
    fn model(rows: &[vel_platform::MenuEntry]) -> Vec<vel_config::Value> {
        rows.iter()
            .map(|row| {
                json!({
                    "id":row.id, "caption":row.caption, "command":row.command,
                    "shortcut":row.shortcut, "children":model(&row.children)
                })
            })
            .collect()
    }
    let menus = model(&app.application_menu());
    let tag = app.workspace.translations.matched_language().as_str();
    let menu_file = if tag == "zh-Hans" {
        "zh_hans-menu.json".into()
    } else {
        format!("{tag}-menu.json")
    };
    std::fs::write(
        out.join(menu_file),
        serde_json::to_vec_pretty(&menus).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let scene = app.scene();
    let theme = app.workspace.themes.ui.borrow();
    let mut layout = Vec::new();
    let mut mixed_checked = false;
    for run in scene
        .layers
        .iter()
        .flat_map(|layer| &layer.text)
        .filter(|run| {
            run.text
                .chars()
                .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
        })
    {
        let style = vel_minihtml::TextStyle {
            font_family: run.family.clone().unwrap_or_else(|| Arc::from("monospace")),
            font_size: run.size,
            font_weight: if run.style.bold { 700 } else { 400 },
            italic: run.style.italic,
            underline: false,
            color: vel_minihtml::Color::rgb(0, 0, 0),
        };
        let metrics = theme.metrics.measure(&run.text, &style);
        let bounds = [
            run.x,
            run.baseline - metrics.ascent,
            run.x + metrics.advance,
            run.baseline + metrics.descent,
        ];
        let clip = [
            run.clip.x,
            run.clip.y,
            run.clip.x + run.clip.width,
            run.clip.y + run.clip.height,
        ];
        // The quick list prepares an extra row just outside the scrolling
        // viewport. A completely invisible run is not a clipped caption.
        if f64::from(bounds[1]) >= clip[3] || f64::from(bounds[3]) <= clip[1] {
            continue;
        }
        let tolerance = 1. / f64::from(scale);
        let fits = f64::from(bounds[0]) + tolerance >= clip[0]
            && f64::from(bounds[1]) + tolerance >= clip[1]
            && f64::from(bounds[2]) <= clip[2] + tolerance
            && f64::from(bounds[3]) <= clip[3] + tolerance;
        let mut record = json!({"text":run.text, "bounds":bounds, "clip":clip, "fits":fits});
        if variant == "zh_hans_mixed"
            && run.text == "文件：以指定编码保存：UTF-8"
            && let Some(highlight) = &run.highlight
        {
            let spans: Vec<_> = highlight
                .ranges
                .iter()
                .map(|range| &run.text[range.clone()])
                .collect();
            if spans != ["UTF-8"] {
                return Err(format!("unexpected mixed-caption highlights: {spans:?}"));
            }
            let fonts = app.fonts.as_mut().ok_or("fonts missing")?;
            let ink = caption_ink(fonts, run, scale)?;
            // Raster bounds, not just the scissored pixels or measured advance.
            record["ink_bounds"] = json!(ink);
            record["fits"] = json!(
                fits && (0..2).all(|i| {
                    ink[i] >= clip[i] - tolerance && ink[i + 2] <= clip[i + 2] + tolerance
                })
            );
            mixed_checked = true;
        }
        layout.push(record);
    }
    let appearance = if dark { "dark" } else { "light" };
    let report = out.join(format!("{variant}-{appearance}@{scale}x-layout.json"));
    let payload = if let Some(root) = audit_root {
        json!({"cjk_runs":layout, "ascii_runs":text_audit::runs(variant, app, root, &scene)})
    } else {
        json!(layout)
    };
    std::fs::write(
        &report,
        serde_json::to_vec_pretty(&payload).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if audit_root.is_some() {
        return Ok(());
    }
    if variant.starts_with("r5_") {
        // Round-5 scenes under `--language zh-Hans` only report: a scrolled tab or a list row
        // half out of view is clipped on purpose.
        for row in layout.iter().filter(|row| row["fits"] != true) {
            eprintln!(
                "velocity: {variant}: Chinese text outside its clip: {}",
                row["text"]
            );
        }
        return Ok(());
    }
    if layout.is_empty()
        || (variant == "zh_hans_mixed" && !mixed_checked)
        || layout.iter().any(|row| row["fits"] != true)
    {
        return Err(format!(
            "Chinese scene has missing or clipped text: {}",
            report.display()
        ));
    }
    Ok(())
}

fn caption_ink(
    fonts: &mut Fonts,
    run: &vel_render::scene::TextRun,
    scale: f32,
) -> Result<[f64; 4], String> {
    let family = run.family.as_deref().ok_or("caption has no UI font")?;
    let px = (run.size * scale * 64.).round() / 64.;
    let mut shaped = fonts.shape_ui(&run.text, family, px, run.style);
    let deadline = Instant::now() + Duration::from_secs(10);
    while fonts.fallback_pending() {
        if Instant::now() >= deadline {
            return Err("caption font fallback did not settle".into());
        }
        std::thread::sleep(Duration::from_millis(2));
        if fonts.poll_fallbacks() {
            shaped = fonts.shape_ui(&run.text, family, px, run.style);
        }
    }
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for glyph in shaped.glyphs {
        let (x, bucket) = vel_render::atlas::subpixel_x(run.x * scale + glyph.x);
        if let Some(raster) = fonts.rasterize(glyph.face, glyph.id, px, bucket) {
            let placement = raster.placement;
            let left = x + placement.left as f32;
            let top = (run.baseline * scale).round() - placement.top as f32 - glyph.y;
            bounds[0] = bounds[0].min(left);
            bounds[1] = bounds[1].min(top);
            bounds[2] = bounds[2].max(left + placement.width as f32);
            bounds[3] = bounds[3].max(top + placement.height as f32);
        }
    }
    if !bounds.iter().all(|b| b.is_finite()) {
        return Err("mixed caption rendered no ink".into());
    }
    Ok(bounds.map(|b| f64::from(b / scale)))
}

/// Entry point for `velocity --render-scenes`.
pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let request = Request::parse(args)?;
    std::fs::create_dir_all(&request.out).map_err(|e| e.to_string())?;
    let fixture = Fixture::create(request.language.as_deref(), &request.catalogs)?;
    let mut renderer: Option<Renderer> = None;
    let mut written = 0;
    for scenario in SCENARIOS {
        if request.only.as_ref().is_some_and(|only| {
            !only.iter().any(|n| {
                n == scenario.name
                    || (n == "settings_ui" && scenario.name.starts_with("settings_ui_"))
            })
        }) {
            continue;
        }
        for &dark in &request.dark {
            for &scale in &request.scales {
                let mut app = (scenario.build)(&fixture, dark)?;
                if let Some(tag) = &request.language
                    && app.workspace.translations.resolve(tag).is_none()
                {
                    return Err(format!("no available UI language matches {tag:?}"));
                }
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
                finish_layout(&mut app, scenario.name);
                let chinese_layout = app.workspace.translations.matched_language().as_str()
                    == "zh-Hans"
                    && (matches!(scenario.name, "zh_hans" | "zh_hans_ui2" | "zh_hans_mixed")
                        || scenario.name.starts_with("r5_")
                        || scenario.name.starts_with("r25_"));
                if request.audit_text || chinese_layout {
                    capture_chinese_layout(
                        &mut app,
                        &request.out,
                        scenario.name,
                        dark,
                        scale,
                        request.audit_text.then_some(&fixture.root),
                    )?;
                    if request.audit_text {
                        written += 1;
                        continue;
                    }
                }
                if request.bench > 0 {
                    bench_scene(&mut app, scenario.name, dark, scale, request.bench);
                    continue;
                }
                let width = (size.width * f64::from(scale)).round() as u32;
                let height = (size.height * f64::from(scale)).round() as u32;
                let available = match renderer.as_mut() {
                    Some(r) => {
                        // Glyph and layout caches are keyed by this scene's font faces.
                        r.reset_fonts();
                        r.resize(width, height);
                        Some(r)
                    }
                    None => match Renderer::offscreen(width, height) {
                        Ok(r) => Some(renderer.insert(r)),
                        Err(error)
                            if matches!(scenario.name, "r6-cjk-zh" | "r5_sidebar")
                                || scenario.name.starts_with("settings_ui") =>
                        {
                            eprintln!("velocity: {error}; using flat software text snapshot");
                            None
                        }
                        Err(error) => return Err(error),
                    },
                };
                let (pixels, scene) = if let Some(renderer) = available {
                    render(&mut app, renderer, scale)?
                } else {
                    let scene = app.scene();
                    let fonts = app.fonts.as_mut().ok_or("fonts missing")?;
                    let pixels =
                        vel_render::snapshot::text_rgba(&scene, fonts, width, height, scale)?;
                    (pixels, scene)
                };
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
    eprintln!("velocity: rendered {written} scenes");
    Ok(())
}

fn finish_layout(app: &mut App, name: &str) {
    settle(app);
    if matches!(name, "status_bar" | "status_narrow" | "status_switcher") {
        // Review the status fields rather than the fixture's package diagnostics.
        app.panels.message.clear();
    }
    let _ = app.scene();
    after_layout(name, app);
    settle(app);
    if name == "hover_definition" {
        hover_definition(app);
        settle(app);
    }
}

#[cfg(test)]
#[test]
fn settings_ui_scene_matrix_draws_each_state_in_both_languages_appearances_and_scales() {
    for (language, search, repair, add_tag, wrap, description, json_link) in [
        (
            "en",
            "Search settings",
            "Edit JSON to fix",
            "Add tag…",
            "Word Wrap",
            "Wrap long lines to fit the editor or a set column.",
            "Edit in JSON",
        ),
        (
            "zh-Hans",
            "搜索设置",
            "在 JSON 中修复",
            "添加标签…",
            "自动换行",
            "在编辑器边缘或指定列自动换行。",
            "在 JSON 中编辑",
        ),
    ] {
        let fixture = Fixture::create(Some(language), &[]).unwrap();
        let settings: Vec<_> = SCENARIOS
            .iter()
            .filter(|s| s.name.starts_with("settings_ui"))
            .collect();
        assert_eq!(settings.len(), 8);
        for scenario in settings {
            for dark in [true, false] {
                for density in [1., 2.] {
                    let mut app = (scenario.build)(&fixture, dark).unwrap();
                    let fonts =
                        Fonts::load_named(&app.workspace.editor().preferences.font_face).unwrap();
                    let size = Size {
                        width: scenario.size.0,
                        height: scenario.size.1,
                    };
                    attach(&mut app, fonts, size, density).unwrap();
                    let scene = app.scene();
                    let texts: Vec<_> = scene
                        .layers
                        .iter()
                        .flat_map(|l| &l.text)
                        .map(|run| run.text.as_str())
                        .collect();
                    let expected = match scenario.name {
                        "settings_ui" => search,
                        "settings_ui_search" | "settings_ui_enum" => "word_wrap",
                        "settings_ui_modified" => "tab_size",
                        "settings_ui_broken" => repair,
                        "settings_ui_array" => add_tag,
                        "settings_ui_guides" => "draw_normal",
                        "settings_ui_dependent" => "wrap_width_style",
                        _ => unreachable!(),
                    };
                    assert!(
                        texts.contains(&expected),
                        "{} {language} dark={dark} @{density}: missing {expected}: {texts:?}",
                        scenario.name
                    );
                    if scenario.name == "settings_ui_enum" {
                        assert!(texts.iter().any(|text| text.starts_with("✓  ")));
                        assert!(
                            texts.contains(&wrap) && texts.contains(&description),
                            "{texts:?}"
                        );
                        assert_eq!(texts.iter().filter(|&&text| text == json_link).count(), 1);
                    }
                    if scenario.name == "settings_ui_array" {
                        assert!(texts.contains(&"*.log"));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[test]
fn shipped_catalogs_pass_draw_list_text_audit() {
    let mut findings = Vec::new();
    for (tag, _) in crate::ui_language::shipped_catalogs() {
        let fixture = Fixture::create(Some(tag.as_str()), &[]).unwrap();
        for scenario in SCENARIOS {
            let mut app = (scenario.build)(&fixture, true).unwrap();
            let fonts = Fonts::load_named(&app.workspace.editor().preferences.font_face).unwrap();
            let size = Size {
                width: scenario.size.0,
                height: scenario.size.1,
            };
            attach(&mut app, fonts, size, 1.).unwrap();
            finish_layout(&mut app, scenario.name);
            let scene = app.scene();
            findings.extend(
                text_audit::runs(scenario.name, &app, &fixture.root, &scene)
                    .into_iter()
                    .map(|run| format!("{} {}: {run:?}", tag.as_str(), scenario.name)),
            );
        }
    }
    findings.sort_unstable();
    findings.dedup();
    assert!(
        findings.is_empty(),
        "unexpected draw-list text:\n{}",
        findings.join("\n")
    );
}
