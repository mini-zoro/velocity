//! Adopted verifier state machine; both eager and genuinely lazy startup restores.
//! Independent verifier probes for round 19 lane B review fixes (not part of the product).
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use tab_adapter::CloseDecision;

fn read_text(path: impl AsRef<Path>) -> Result<String, String> {
    use std::io::Read;
    let mut file = vel_fileio::regular::read(path.as_ref())?;
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|e| e.to_string())?;
    Ok(text)
}
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, p: u64) -> bool {
        self.next() % 100 < p
    }
}

fn owner<'a>(a: &'a App, tab: &'a crate::workspace::Tab) -> &'a crate::workspace::Tab {
    // A materialized owner of the same session document wins over stale lazy copies.
    a.workspace
        .tabs
        .iter()
        .find(|t| {
            t.session_document == tab.session_document
                && t.lazy.is_none()
                && t.editor.buffer.id() == t.document
        })
        .or_else(|| {
            a.workspace
                .tabs
                .iter()
                .find(|t| t.editor.buffer.id() == tab.document && t.lazy.is_none())
        })
        .unwrap_or(tab)
}
fn doc_text(a: &App, tab: &crate::workspace::Tab) -> String {
    let o = owner(a, tab);
    if let Some((doc, _)) = &o.lazy {
        if let Some(h) = &doc.history {
            return h
                .current_text()
                .map(|t| t.to_string())
                .unwrap_or_else(|e| format!("<unavailable {e}>"));
        }
        if let Some(p) = &doc.path {
            return read_text(p).unwrap_or_default();
        }
        return String::new();
    }
    o.editor.text.to_string()
}
#[derive(Clone, Debug)]
struct Doc {
    text: String,
    precious: bool,
    views: usize,
    path: Option<PathBuf>,
    unavailable: bool,
}
#[derive(Clone, Debug)]
struct Snap {
    info: BTreeMap<u64, String>,
    sheets: BTreeSet<u64>,
    docs: BTreeMap<u64, Doc>,
}
fn snap(a: &App) -> Snap {
    let mut docs: BTreeMap<u64, Doc> = BTreeMap::new();
    if a.workspace.is_empty() {
        let t = &a.workspace.tabs[0];
        if t.editor.text.len_bytes() > 0 {
            docs.insert(
                t.session_document,
                Doc {
                    text: t.editor.text.to_string(),
                    precious: true,
                    views: 1,
                    path: None,
                    unavailable: false,
                },
            );
        }
        return Snap {
            info: BTreeMap::new(),
            sheets: BTreeSet::new(),
            docs,
        };
    }
    for t in &a.workspace.tabs {
        let entry = docs.entry(t.session_document).or_insert_with(|| Doc {
            text: doc_text(a, t),
            precious: a.workspace.document_contents_dirty(owner(a, t)),
            views: 0,
            path: t.path.clone(),
            unavailable: t.path.as_ref().is_some_and(|p| read_text(p).is_err()),
        });
        entry.views += 1;
    }
    let info = a
        .workspace
        .tabs
        .iter()
        .map(|t| {
            (
                t.sheet,
                format!(
                    "path={:?} len={} transient={} dirty={} lazy={} read_only={} group={:?} wb={:?}",
                    t.path,
                    t.editor.text.len_bytes(),
                    t.transient,
                    t.dirty(),
                    t.lazy.is_some(),
                    t.editor.read_only,
                    a.workspace.workbench.group_of(t.sheet),
                    a.workspace.workbench.groups.iter().map(|g| (g.sheets.clone(), g.focused)).collect::<Vec<_>>()
                ),
            )
        })
        .collect();
    Snap {
        info,
        sheets: ids(a),
        docs,
    }
}
fn session_ok(a: &mut App, ctx: &str) {
    let (window, documents) = a.workspace.snapshot().unwrap();
    vel_session::Session {
        windows: vec![window],
        documents,
        ..Default::default()
    }
    .validate()
    .unwrap_or_else(|e| panic!("{ctx}: session invalid: {e}"));
}
fn coherent(a: &App) -> Result<(), String> {
    if a.workspace.is_empty() {
        return Ok(());
    }
    let sheet = a.workspace.tabs[a.workspace.active].sheet;
    let wb = &a.workspace.workbench;
    if wb.groups[wb.active].focused != Some(sheet) {
        return Err(format!(
            "active tab {sheet} is not the focused sheet {:?} of active group {}",
            wb.groups[wb.active].focused, wb.active
        ));
    }
    Ok(())
}
fn mouse(a: &mut App, x: f64, y: f64, button: usize, modifiers: u64, kind: MouseKind) {
    a.handle_mouse_event(
        None,
        MouseEvent {
            kind,
            position: Point { x, y },
            button,
            click_count: 1,
            modifiers: Modifiers(modifiers),
            timestamp: 0.,
        },
    );
}
fn click_tab(a: &mut App, sheet: u64, button: usize, modifiers: u64) -> bool {
    a.scene();
    let Some((_, rect)) = a.tab_hits.iter().find(|(id, _)| *id == sheet).copied() else {
        return false;
    };
    let (x, y) = (rect.x + rect.width / 2., rect.y + rect.height / 2.);
    mouse(a, x, y, button, modifiers, MouseKind::Down);
    mouse(a, x, y, button, modifiers, MouseKind::Up);
    true
}
fn wait_loaded(a: &mut App) {
    let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
    while a
        .workspace
        .tabs
        .iter()
        .any(|t| t.loading.is_some() || t.binary_loading.is_some())
    {
        a.workspace.poll();
        assert!(Instant::now() < deadline, "file load timeout");
        std::thread::sleep(Duration::from_millis(1));
    }
}
/// Simulates the startup restore path in main.rs for one window of the saved session.
fn restore_into(data: &Path, index: usize) -> App {
    let mut b = app(Some(data));
    let restored = b.sessions.borrow().restored.clone();
    let reserved = b.sessions.borrow_mut().reserve_recovered();
    let window = restored.windows[index].clone();
    b.workspace
        .restore_window(&window, &restored.documents)
        .unwrap();
    b.restore_ui(&window);
    b.restore_session_id(reserved[index]);
    b.sync_active_view();
    b
}

#[derive(Default)]
struct Stats {
    ops: BTreeMap<&'static str, usize>,
    closes_exact: usize,
    closes_partial: usize,
    prompts: usize,
    discards: usize,
    saves: usize,
    restores: usize,
    failed_loads: [usize; 6],
    incoherent: Vec<String>,
    k1: Vec<String>,
    errors: BTreeMap<String, usize>,
}

const PRESETS: [&str; 8] = [
    "single", "columns2", "columns3", "columns4", "rows2", "rows3", "grid4", "custom",
];
const CLOSES: [&str; 14] = [
    "close_all",
    "close",
    "close_file",
    "super+w",
    "close_by_index",
    "close_by_index_ctx",
    "close_other_tabs",
    "close_others_by_index",
    "close_tabs_to_right",
    "close_to_right_by_index",
    "close_unmodified",
    "close_deleted_files",
    "close_selected",
    "close_selected_ctx",
];

fn expected_targets(a: &App, op: &str, args: &Value) -> BTreeSet<u64> {
    if a.workspace.is_empty() {
        return BTreeSet::new();
    }
    let wb = &a.workspace.workbench;
    let ctx = args["_context_sheet"].as_u64();
    let g = ctx
        .and_then(|s| wb.group_of(s))
        .or_else(|| args["group"].as_u64().map(|g| g as usize))
        .unwrap_or(wb.active);
    let Some(group) = wb.groups.get(g) else {
        return BTreeSet::new();
    };
    let index = if let Some(s) = ctx {
        group.sheets.iter().position(|x| *x == s)
    } else if let Some(i) = args["index"].as_u64() {
        (i < group.sheets.len() as u64).then_some(i as usize)
    } else {
        group
            .focused
            .and_then(|f| group.sheets.iter().position(|x| *x == f))
    };
    let dirty = |id: &u64| {
        let t = &a.workspace.tabs[a.workspace.sheet_index(*id).unwrap()];
        a.workspace.document_dirty(t)
    };
    match op {
        "close_all" => ids(a),
        "close_deleted_files" => a
            .workspace
            .tabs
            .iter()
            .filter(|t| {
                t.path
                    .as_ref()
                    .is_some_and(|p| matches!(p.try_exists(), Ok(false)))
            })
            .map(|t| t.sheet)
            .collect(),
        "close_selected" | "close_selected_ctx" => {
            if index.is_none() {
                return BTreeSet::new();
            }
            group.selected.clone()
        }
        _ => {
            let Some(index) = index else {
                return BTreeSet::new();
            };
            group
                .sheets
                .iter()
                .enumerate()
                .filter(|(i, id)| match op {
                    "close_other_tabs" | "close_others_by_index" => *i != index,
                    "close_tabs_to_right" | "close_to_right_by_index" => *i > index,
                    "close_unmodified" => !dirty(id),
                    _ => *i == index,
                })
                .map(|(_, id)| *id)
                .collect()
        }
    }
}

fn sweep_seed(seed: u64, steps: usize, stats: &mut Stats, materialize: bool, failed_loads: bool) {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let saves = tempfile::tempdir().unwrap();
    let mut a = app(Some(data.path()));
    named(
        &mut a,
        "insert",
        json!({"characters": format!("seed {seed} start ")}),
    );
    let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
    let mut file_counter = 0;
    let mut damaged = Vec::<(PathBuf, Vec<u8>)>::new();
    for step in 0..steps {
        let ctx = format!("seed {seed} step {step}");
        let before = snap(&a);
        // Documents whose views have not been reconciled to one buffer (restored lazy clones).
        let mixed: BTreeSet<u64> = {
            let mut m: BTreeMap<u64, BTreeSet<vel_buffer::BufferId>> = BTreeMap::new();
            if !a.workspace.is_empty() {
                for t in &a.workspace.tabs {
                    m.entry(t.session_document).or_default().insert(t.document);
                }
            }
            m.into_iter()
                .filter(|(_, d)| d.len() > 1)
                .map(|(k, _)| k)
                .collect()
        };
        assert!(
            mixed.is_empty(),
            "{ctx}: restored siblings reference distinct buffers"
        );
        let count = a.workspace.workbench.groups.len();
        let pick = rng.below(100);
        let mut op: &'static str;
        let mut close: Option<(&'static str, Value)> = None;
        let mut edited = false;
        let mut restored = false;
        let mut pushed: Vec<CloseDecision> = Vec::new();
        a.panels.message.clear();
        match pick {
            0..=7 => {
                op = "set_layout";
                let preset = PRESETS[rng.below(PRESETS.len())];
                if rng.chance(40) && preset != "custom" && preset != "rows2" && preset != "rows3" {
                    let k = match preset {
                        "single" => "super+alt+1",
                        "columns2" => "super+alt+2",
                        "columns3" => "super+alt+3",
                        "columns4" => "super+alt+4",
                        _ => "super+alt+5",
                    };
                    op = "layout_key";
                    key(&mut a, k);
                } else {
                    let args = if preset == "custom" {
                        json!({"cols":[0.,0.25,1.],"rows":[0.,0.5,1.],"cells":[[0,0,1,2],[1,0,2,1],[1,1,2,2]]})
                    } else {
                        json!({"preset":preset})
                    };
                    named(&mut a, "set_layout", args);
                }
            }
            8..=11 => {
                op = "new_pane";
                if count < 10 {
                    named(&mut a, "new_pane", json!({"move": rng.chance(50)}));
                }
            }
            12..=14 => {
                op = "close_pane";
                named(&mut a, "close_pane", json!({}));
            }
            15..=19 => {
                op = "focus_group";
                let g = rng.below(count + 2);
                if rng.chance(50) && g < 9 {
                    key(&mut a, &format!("ctrl+{}", g + 1));
                } else {
                    named(&mut a, "focus_group", json!({"group": g}));
                }
            }
            20..=24 => {
                op = "move_to_group";
                let g = rng.below(count + 2);
                if rng.chance(50) && g < 9 {
                    key(&mut a, &format!("ctrl+shift+{}", g + 1));
                } else {
                    named(&mut a, "move_to_group", json!({"group": g}));
                }
            }
            25..=28 => {
                op = "clone_file";
                named(&mut a, "clone_file", json!({}));
            }
            29..=32 => {
                op = "split_view";
                named(&mut a, "split_view", json!({}));
            }
            33..=52 => {
                op = "close";
                let variant = CLOSES[rng.below(CLOSES.len())];
                let mut args = json!({});
                if !a.workspace.is_empty() {
                    let all: Vec<u64> = ids(&a).into_iter().collect();
                    let s = all[rng.below(all.len())];
                    let g = a.workspace.workbench.group_of(s).unwrap();
                    let i = a.workspace.workbench.groups[g]
                        .sheets
                        .iter()
                        .position(|x| *x == s)
                        .unwrap();
                    match variant {
                        "close_by_index" => {
                            args = json!({"group": g, "index": if rng.chance(10) {i + 50} else {i}})
                        }
                        "close_by_index_ctx"
                        | "close_others_by_index"
                        | "close_to_right_by_index"
                        | "close_selected_ctx" => {
                            args = json!({"_context_menu":"tab","_context_sheet": s, "group": g, "index": i})
                        }
                        _ => {}
                    }
                }
                let n = 2 + a.workspace.tabs.len();
                for k in 0..n {
                    let d = match rng.below(10) {
                        0..=3 => CloseDecision::Discard,
                        4..=6 => {
                            CloseDecision::Save(Some(saves.path().join(format!("s{step}-{k}.txt"))))
                        }
                        7 => CloseDecision::Save(Some(saves.path().to_owned())), // fails: a directory
                        _ => CloseDecision::Cancel,
                    };
                    pushed.push(match &d {
                        CloseDecision::Discard => CloseDecision::Discard,
                        CloseDecision::Save(p) => CloseDecision::Save(p.clone()),
                        CloseDecision::Cancel => CloseDecision::Cancel,
                    });
                    a.close_decisions.push_back(d);
                }
                close = Some((variant, args));
            }
            53..=58 => {
                op = "middle_click";
                if !a.workspace.is_empty() {
                    let all: Vec<u64> = ids(&a).into_iter().collect();
                    let s = all[rng.below(all.len())];
                    let n = 3;
                    for k in 0..n {
                        let d = match rng.below(3) {
                            0 => CloseDecision::Discard,
                            1 => CloseDecision::Save(Some(
                                saves.path().join(format!("m{step}-{k}.txt")),
                            )),
                            _ => CloseDecision::Cancel,
                        };
                        pushed.push(match &d {
                            CloseDecision::Discard => CloseDecision::Discard,
                            CloseDecision::Save(p) => CloseDecision::Save(p.clone()),
                            CloseDecision::Cancel => CloseDecision::Cancel,
                        });
                        a.close_decisions.push_back(d);
                    }
                    close = Some(("middle", json!({"_context_sheet": s})));
                }
            }
            59..=67 => {
                op = "click";
                if !a.workspace.is_empty() {
                    let all: Vec<u64> = ids(&a).into_iter().collect();
                    let s = all[rng.below(all.len())];
                    let m = match rng.below(3) {
                        0 => 0,
                        1 => 1 << 20,
                        _ => 1 << 17,
                    };
                    click_tab(&mut a, s, 0, m);
                }
            }
            68..=77 => {
                op = "edit";
                edited = true;
                let marker = format!("<{seed}:{step}>");
                if rng.chance(50) {
                    named(&mut a, "insert", json!({"characters": marker}));
                } else {
                    for c in marker.chars() {
                        a.workspace.editor_mut().text_input(TextInput::Insert {
                            text: c.to_string(),
                            replacement: None,
                        });
                    }
                    a.changed_state(Instant::now(), true);
                }
            }
            78..=81 => {
                op = "new_file";
                if rng.chance(50) {
                    key(&mut a, "super+n");
                } else {
                    named(&mut a, "new_file", json!({}));
                }
            }
            82..=85 => {
                op = "open_file";
                file_counter += 1;
                let path = files.path().join(format!("f{file_counter}.txt"));
                std::fs::write(&path, format!("file {file_counter} of seed {seed}\n")).unwrap();
                named(&mut a, "open_file", json!({"file": path}));
                wait_loaded(&mut a);
            }
            86..=88 => {
                op = "delete_on_disk";
                let candidates: Vec<usize> = (0..a.workspace.tabs.len())
                    .filter(|i| {
                        !a.workspace.is_empty()
                            && a.workspace.tabs[*i]
                                .path
                                .as_ref()
                                .is_some_and(|p| p.is_file())
                            // A clean disk-backed lazy history has no retained bytes.
                            // Delete only documents with live contents for this model
                            // to keep checking exact text, without opening lazy views.
                            && a.workspace.tabs.iter().any(|owner| {
                                owner.session_document == a.workspace.tabs[*i].session_document
                                    && owner.lazy.is_none()
                            })
                    })
                    .collect();
                if !candidates.is_empty() {
                    let i = candidates[rng.below(candidates.len())];
                    let path = a.workspace.tabs[i].path.clone().unwrap();
                    std::fs::remove_file(&path).unwrap();
                    for t in &mut a.workspace.tabs {
                        if t.path.as_ref() == Some(&path) && t.lazy.is_none() {
                            t.deleted = true;
                        }
                    }
                }
            }
            89..=90 => {
                op = "save";
                if !a.workspace.is_empty()
                    && a.workspace.tabs[a.workspace.active].path.is_some()
                    && !a.workspace.tabs[a.workspace.active].editor.read_only
                {
                    key(&mut a, "super+s");
                }
            }
            91..=93 => {
                op = "select_all_tabs";
                named(&mut a, "select_all_tabs", json!({}));
            }
            94..=95 => {
                op = "drag_tab";
                if count > 1 && !a.workspace.is_empty() {
                    a.scene();
                    let target = rng.below(count);
                    let tab = a.tab_hits.first().copied();
                    let bounds = a
                        .tab_strips
                        .iter()
                        .find(|strip| strip.group == target)
                        .map(|strip| strip.bounds);
                    if let (Some((_, r)), Some(b)) = (tab, bounds) {
                        mouse(&mut a, r.x + 4., r.y + 4., 0, 0, MouseKind::Down);
                        mouse(
                            &mut a,
                            b.x + b.width / 2.,
                            b.y + b.height / 2.,
                            0,
                            0,
                            MouseKind::Drag,
                        );
                        mouse(
                            &mut a,
                            b.x + b.width / 2.,
                            b.y + b.height / 2.,
                            0,
                            0,
                            MouseKind::Up,
                        );
                    }
                }
            }
            _ => {
                op = "restore";
                restored = true;
            }
        }
        *stats.ops.entry(op).or_default() += 1;
        if std::env::var("B19_TRACE")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            == Some(seed)
        {
            println!(
                "TRACE step {step} op {op} close {close:?} empty {} active {} wb {:?}",
                a.workspace.is_empty(),
                a.workspace.active,
                a.workspace
                    .workbench
                    .groups
                    .iter()
                    .map(|g| (g.sheets.clone(), g.selected.clone(), g.focused))
                    .collect::<Vec<_>>()
            );
            for t in &a.workspace.tabs {
                println!(
                    "   sheet {} sdoc {} doc {:?} buf {:?} view {:?} shadow {} lazy {} text {:?} dirty {} path {:?} deleted {} lazyhist {:?}",
                    t.sheet,
                    t.session_document,
                    t.document,
                    t.editor.buffer.id(),
                    t.editor.id,
                    t.editor.shared_shadow,
                    t.lazy.is_some(),
                    t.editor.text.to_string(),
                    t.dirty(),
                    t.path,
                    t.deleted,
                    t.lazy.as_ref().map(|(d, _)| (
                        d.dirty,
                        d.history
                            .as_ref()
                            .map(|h| h.current_text().map(|t| t.to_string()))
                    ))
                );
            }
        }
        if restored {
            stats.restores += 1;
            a.checkpoint(false)
                .unwrap_or_else(|e| panic!("{ctx}: checkpoint failed: {e}"));
            a.sessions.borrow().flush().unwrap();
            assert!(
                a.workspace.messages.is_empty(),
                "{ctx}: {:?}",
                a.workspace.messages
            );
            let recovery = vel_session::load(&data.path().join("Sessions/Session.sublime-session"));
            assert!(
                recovery.messages.is_empty(),
                "{ctx}: {:?}",
                recovery.messages
            );
            recovery.session.validate().unwrap();
            let groups_before: Vec<usize> = a
                .workspace
                .workbench
                .groups
                .iter()
                .map(|g| g.sheets.len())
                .collect();
            let layout_before = a.workspace.workbench.layout.clone();
            let active_before = a.workspace.workbench.active;
            drop(a);
            if failed_loads {
                use std::os::unix::fs::PermissionsExt;
                // Leave the files unavailable throughout the following session: hidden
                // views may materialize much later, or never be opened at all.
                for (path, bytes) in damaged.drain(..) {
                    let metadata = std::fs::symlink_metadata(&path).ok();
                    if metadata.as_ref().is_some_and(|m| m.is_dir()) {
                        std::fs::remove_dir(&path).unwrap();
                    } else if metadata.as_ref().is_some_and(|m| !m.is_file()) {
                        std::fs::remove_file(&path).unwrap();
                    } else if path.exists() {
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                            .unwrap();
                    }
                    std::fs::write(path, bytes).unwrap();
                }
                let paths: BTreeSet<_> = before
                    .docs
                    .values()
                    .filter_map(|d| d.path.clone())
                    .filter(|p| p.is_file())
                    .collect();
                if !paths.is_empty() {
                    let pick = rng.below(paths.len());
                    let path = paths.into_iter().nth(pick).unwrap();
                    let bytes = std::fs::read(&path).unwrap();
                    let kind = rng.below(6);
                    match kind {
                        0 => {
                            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
                                .unwrap()
                        }
                        1 => {
                            std::fs::remove_file(&path).unwrap();
                            std::fs::create_dir(&path).unwrap();
                        }
                        2 => std::fs::remove_file(&path).unwrap(),
                        3 => {
                            std::fs::remove_file(&path).unwrap();
                            assert!(
                                std::process::Command::new("mkfifo")
                                    .arg(&path)
                                    .status()
                                    .unwrap()
                                    .success()
                            );
                        }
                        4 => {
                            std::fs::remove_file(&path).unwrap();
                            let _socket = std::os::unix::net::UnixListener::bind(&path).unwrap();
                        }
                        _ => {
                            std::fs::remove_file(&path).unwrap();
                            std::os::unix::fs::symlink("/dev/zero", &path).unwrap();
                        }
                    }
                    stats.failed_loads[kind] += 1;
                    damaged.push((path, bytes));
                }
            }
            a = restore_into(data.path(), 0);
            wait_loaded(&mut a);
            if materialize {
                for i in 0..a.workspace.tabs.len() {
                    let _ = a.workspace.prepare_sheet(i);
                }
                wait_loaded(&mut a);
            }
            if !materialize {
                let visible = a
                    .workspace
                    .workbench
                    .groups
                    .iter()
                    .flat_map(|g| g.visible())
                    .collect::<BTreeSet<_>>();
                for tab in &a.workspace.tabs {
                    if !visible.contains(&tab.sheet) && !a.workspace.is_empty() {
                        assert!(
                            tab.lazy.is_some() || (failed_loads && tab.load_failed),
                            "{ctx}: hidden view was eagerly opened"
                        );
                    }
                }
            }
            let after = snap(&a);
            let retained: BTreeSet<_> = before
                .docs
                .values()
                .chain(after.docs.values())
                .filter(|d| d.precious)
                .filter_map(|d| d.path.as_ref())
                .collect();
            let unavailable: BTreeSet<_> = before
                .docs
                .values()
                .chain(after.docs.values())
                .filter(|d| failed_loads && unavailable_clean(d))
                .filter_map(|d| d.path.as_ref())
                .filter(|p| !retained.contains(p))
                .collect();
            // Every document with content or a path must survive, with identical text.
            let mut want: Vec<(String, bool)> = before
                .docs
                .values()
                .filter(|d| {
                    (d.path.is_some() || !d.text.is_empty())
                        && (d.precious || !d.path.as_ref().is_some_and(|p| unavailable.contains(p)))
                })
                .map(|d| (d.text.clone(), d.precious))
                .collect();
            let mut got: Vec<(String, bool)> = after
                .docs
                .values()
                .filter(|d| {
                    (d.path.is_some() || !d.text.is_empty())
                        && (d.precious || !d.path.as_ref().is_some_and(|p| unavailable.contains(p)))
                })
                .map(|d| (d.text.clone(), d.precious))
                .collect();
            want.sort();
            got.sort();
            assert_eq!(
                want.iter().map(|w| &w.0).collect::<Vec<_>>(),
                got.iter().map(|g| &g.0).collect::<Vec<_>>(),
                "{ctx}: restore changed document contents"
            );
            for (w, g) in want.iter().zip(&got) {
                assert!(
                    !w.1 || g.1,
                    "{ctx}: dirty document restored clean: {:?}",
                    w.0
                );
            }
            let groups_after: Vec<usize> = a
                .workspace
                .workbench
                .groups
                .iter()
                .map(|g| g.sheets.len())
                .collect();
            if !before.sheets.is_empty() {
                assert_eq!(groups_before.len(), groups_after.len(), "{ctx}");
                let extra: usize = groups_after.iter().sum::<usize>()
                    - groups_before
                        .iter()
                        .sum::<usize>()
                        .min(groups_after.iter().sum());
                assert!(
                    groups_before == groups_after || extra <= 1,
                    "{ctx}: groups {groups_before:?} -> {groups_after:?}"
                );
                assert_eq!(
                    layout_before.cells, a.workspace.workbench.layout.cells,
                    "{ctx}"
                );
                assert_eq!(active_before, a.workspace.workbench.active, "{ctx}");
            }
            validate(&a);
            session_ok(&mut a, &ctx);
            continue;
        }
        let mut target = BTreeSet::new();
        let mut enabled = true;
        if let Some((variant, args)) = &close {
            target = expected_targets(
                &a,
                match *variant {
                    "middle" => "close_by_index_ctx",
                    "super+w" => "close",
                    v => v,
                },
                args,
            );
            let name = match *variant {
                "super+w" | "close" => "close",
                "middle" | "close_by_index_ctx" => "close_by_index",
                "close_selected_ctx" => "close_selected",
                v => v,
            };
            enabled = a.command_enabled_args(name, args);
            match *variant {
                "super+w" => {
                    key(&mut a, "super+w");
                }
                "middle" => {
                    let s = args["_context_sheet"].as_u64().unwrap();
                    if !click_tab(&mut a, s, 2, 0) {
                        // Tab scrolled out of view: nothing should change.
                        target.clear();
                    }
                }
                _ => named(&mut a, name, args.clone()),
            }
        }
        let remaining = a.close_decisions.len();
        let consumed: Vec<CloseDecision> = pushed.drain(..pushed.len() - remaining).collect();
        if std::env::var("B19_TRACE")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            == Some(seed)
            && !consumed.is_empty()
        {
            println!("   consumed {consumed:?}");
        }
        a.close_decisions.clear();
        let msg = a.panels.message.clone();
        if !msg.is_empty() {
            let key = msg.chars().take(60).collect::<String>();
            *stats.errors.entry(key).or_default() += 1;
        }
        // Invariants.
        a.workspace
            .workbench
            .validate(&ids(&a))
            .unwrap_or_else(|e| panic!("{ctx} op {op} {close:?}: workbench invalid: {e}"));
        a.workspace
            .validate_document_views()
            .unwrap_or_else(|e| panic!("{ctx} op {op}: invalid document attachment: {e}"));
        assert!(
            a.workspace
                .tabs
                .iter()
                .all(|t| !t.load_failed || t.editor.read_only),
            "{ctx} op {op}: failed-load view became editable"
        );
        if let Err(e) = coherent(&a) {
            stats.incoherent.push(format!("{ctx} op {op}: {e}"));
        }
        let after = snap(&a);
        let closed: BTreeSet<u64> = before.sheets.difference(&after.sheets).copied().collect();
        if close.is_none() {
            // Opening a file may replace a lone empty, clean untitled tab.
            let closed: BTreeSet<u64> = closed
                .iter()
                .copied()
                .filter(|c| {
                    !(op == "open_file"
                        && before.info[c]
                            .starts_with("path=None len=0 transient=false dirty=false"))
                })
                .collect();
            assert!(
                closed.is_empty(),
                "{ctx} op {op}: lost sheets {closed:?} without a close: {:?}",
                closed
                    .iter()
                    .map(|c| before.info.get(c))
                    .collect::<Vec<_>>()
            );
        } else if let Some((variant, _)) = &close {
            assert!(
                closed.is_subset(&target),
                "{ctx} {variant}: closed {closed:?} outside targets {target:?}"
            );
            // Every generated destination is fresh. An unavailable/read-only
            // source can also fail Save As: no destination is published, and
            // the sequential close operation must stop at that decision.
            let stopped = consumed.iter().any(close_stopped);
            if let Some(i) = consumed.iter().position(close_stopped) {
                assert_eq!(
                    i + 1,
                    consumed.len(),
                    "{ctx} {variant}: prompts continued after cancel/failed save"
                );
            }
            if !stopped && enabled {
                assert_eq!(
                    closed, target,
                    "{ctx} {variant}: closed {closed:?} != targets {target:?}; msg {msg}"
                );
                stats.closes_exact += 1;
            } else {
                stats.closes_partial += 1;
            }
            stats.prompts += consumed.len();
        }
        // Dirty content may be lost only through an explicit Don't Save.
        let mut discards = consumed
            .iter()
            .filter(|d| matches!(d, CloseDecision::Discard))
            .count();
        stats.discards += discards;
        let saved: Vec<String> = consumed
            .iter()
            .filter_map(|d| match d {
                CloseDecision::Save(Some(p)) if p.is_file() => Some(read_text(p).unwrap()),
                _ => None,
            })
            .collect();
        stats.saves += saved.len();
        for (id, doc) in &before.docs {
            match after.docs.get(id) {
                Some(now) if mixed.contains(id) && now.text != doc.text && !edited => {
                    stats
                        .k1
                        .push(format!("{ctx} {close:?}: {:?} -> {:?}", doc.text, now.text));
                }
                Some(now) => {
                    if !edited && !(failed_loads && unavailable_clean(doc)) {
                        assert_eq!(
                            now.text, doc.text,
                            "{ctx} op {op}: document text changed without an edit"
                        );
                    }
                }
                None => {
                    if doc.precious {
                        let on_disk_saved = saved.contains(&doc.text)
                            || doc
                                .path
                                .as_ref()
                                .is_some_and(|p| read_text(p).is_ok_and(|s| s == doc.text));
                        if !on_disk_saved && mixed.contains(id) && discards == 0 {
                            stats
                                .k1
                                .push(format!("{ctx} {close:?}: lost {:?}", doc.text));
                        } else if !on_disk_saved {
                            assert!(
                                discards > 0,
                                "{ctx} op {op} {close:?}: dirty document lost without Don't Save: {:?}",
                                doc.text
                            );
                            discards -= 1;
                        }
                    }
                }
            }
        }
        if step % 7 == 0 || close.is_some() {
            session_ok(&mut a, &ctx);
        }
    }
    // Final hot-exit quit must succeed and keep every dirty document.
    let before = snap(&a);
    assert!(a.prepare_quit(None), "seed {seed}: quit refused");
    assert!(a.workspace.messages.is_empty());
    drop(a);
    let recovery = vel_session::load(&data.path().join("Sessions/Session.sublime-session"));
    recovery.session.validate().unwrap();
    let texts: Vec<String> = recovery
        .session
        .documents
        .iter()
        .filter(|d| !failed_loads || d.dirty)
        .filter_map(|d| {
            d.history
                .as_ref()
                .map(|h| h.current_text().unwrap().to_string())
        })
        .collect();
    for doc in before.docs.values().filter(|d| d.precious) {
        assert!(
            texts.contains(&doc.text),
            "seed {seed}: dirty doc missing from hot exit: {:?}",
            doc.text
        );
    }
}

fn unavailable_clean(doc: &Doc) -> bool {
    !doc.precious && (doc.unavailable || doc.path.as_ref().is_some_and(|p| read_text(p).is_err()))
}
fn close_stopped(decision: &CloseDecision) -> bool {
    match decision {
        CloseDecision::Cancel | CloseDecision::Save(None) => true,
        CloseDecision::Save(Some(path)) => !path.is_file(),
        CloseDecision::Discard => false,
    }
}
fn sweep_range(seeds: std::ops::Range<u64>, steps: usize, materialize: bool, failed_loads: bool) {
    let mut stats = Stats::default();
    for seed in seeds.clone() {
        sweep_seed(seed, steps, &mut stats, materialize, failed_loads);
    }
    println!(
        "B19_SWEEP seeds {seeds:?} steps {steps} ops {:?} exact_closes {} partial {} prompts {} discards {} saves {} restores {} incoherent {} errors {:?}",
        stats.ops,
        stats.closes_exact,
        stats.closes_partial,
        stats.prompts,
        stats.discards,
        stats.saves,
        stats.restores,
        stats.incoherent.len(),
        stats.errors
    );
    println!("B19_K1 count {}", stats.k1.len());
    println!(
        "B19_FAILED_LOADS chmod/directory/deleted/fifo/socket/device {:?}",
        stats.failed_loads
    );
    for k in stats.k1.iter().take(5) {
        println!("B19_K1 {k}");
    }
    for i in stats.incoherent.iter().take(5) {
        println!("B19_INCOHERENT {i}");
    }
    assert!(
        stats.k1.is_empty(),
        "lazy shared document losses: {:?}",
        stats.k1
    );
    assert!(
        stats.incoherent.is_empty(),
        "focus mismatches: {:?}",
        stats.incoherent
    );
}

#[test]
fn r19_verify_lazy_restore_state_machine() {
    sweep_range(0..4, 160, false, false);
}
#[test]
#[ignore = "40 seeds x 1000 operations with startup restores and all close variants"]
fn r19_verify_extended_state_machine() {
    sweep_range(0..40, 1000, true, false);
}
#[test]
#[ignore = "40 seeds x 1000 operations: leave every restored hidden sheet lazy"]
fn r19_verify_extended_lazy_state_machine() {
    sweep_range(0..40, 1000, false, false);
}

#[test]
fn r19_recovery_failed_load_lazy_state_machine() {
    sweep_range(0..4, 160, false, true);
}
#[test]
#[ignore = "40 seeds x 1000 operations with failed loads, FIFOs, sockets and devices"]
fn r19_recovery_extended_failed_load_lazy_state_machine() {
    sweep_range(0..40, 1000, false, true);
}
