//! User grammar loading, detection, picker, cache and live reload through Workspace.
use crate::{
    cli::FileLocation,
    panels::{Overlay, Panels},
    workspace::Workspace,
};
use std::{
    io::Write,
    path::Path,
    time::{Duration, Instant},
};
use vel_config::json;
use vel_text::Text;

const JSON: &str = include_str!("../fixtures/user-grammars/Velocity Notes.tmLanguage.json");
const XML: &str = include_str!("../fixtures/user-grammars/Velocity XML.tmLanguage");

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
fn open(w: &mut Workspace, path: &Path) {
    w.open(FileLocation {
        path: path.into(),
        line: 1,
        column: 1,
    })
    .unwrap();
    wait(w);
}
fn wait(w: &mut Workspace) {
    // A hang watchdog, not a budget: each reload rebuilds every grammar, which takes
    // 4-5 s alone and more than 10 s while the whole suite runs in parallel.
    let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
    loop {
        w.wait_resources();
        let ready = !w.resources_busy()
            && w.tabs.iter().all(|t| t.loading.is_none())
            && w.editor()
                .language
                .borrow()
                .as_ref()
                .is_some_and(|l| !l.busy() && l.analysis.is_some());
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "{:?}", w.messages);
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

#[test]
fn interactive_startup_honors_user_grammar_overlays() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("profile");
    write(
        &data.join("Packages/User/Velocity Notes.tmLanguage.json"),
        JSON,
    );
    let file = temp.path().join("sample.vnote");
    write(&file, "TODO\n");
    let mut workspace =
        Workspace::new_for_startup(Text::new(), Some(data), None, true, &mut Default::default())
            .unwrap();
    workspace.open(FileLocation::path_only(file)).unwrap();
    let editor = workspace.editor();
    assert_eq!(
        editor.language.borrow().as_ref().unwrap().name,
        "Velocity Notes"
    );
    assert!(workspace.palette_pending());
}

#[test]
fn loose_xml_json_detection_picker_scopes_and_nested_add_edit_remove_reload() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let grammar = data.join("Packages/Notes/Nested/Velocity Notes.tmLanguage.json");
    write(&grammar, JSON);
    write(&data.join("Packages/User/Velocity XML.tmLanguage"), XML);
    let file = temp.path().join("sample.vnote");
    write(&file, "TODO \"text\" # comment\n");
    let mut w = Workspace::new(Text::new(), Some(data.clone()), None, false).unwrap();
    open(&mut w, &file);
    assert_eq!(
        w.editor().language.borrow().as_ref().unwrap().name,
        "Velocity Notes"
    );
    assert!(
        w.palette
            .iter()
            .any(|p| p.caption == "Set Syntax: Velocity XML Notes")
    );
    {
        let language = w.editor().language.borrow();
        assert!(language.as_ref().unwrap().selector(
            &w.editor().text,
            "keyword.other.todo.velocity-notes",
            1
        ));
    }
    // The picker reads the same updated syntax index as detection/palette entries.
    let mut p = Panels::new(&w, false);
    p.action(&mut w, "show_overlay", &json!({"overlay":"syntax"}), 0..100)
        .unwrap();
    assert_eq!(p.overlay, Some(Overlay::Syntax));
    assert!(p.quick.items.iter().any(|i| i.caption == "Velocity Notes") || p.busy());
    p.key(
        &mut w,
        &vel_config::keymap::Key::parse("escape").unwrap(),
        0..100,
    )
    .unwrap();
    w.editor_mut()
        .run("insert", &json!({"characters":"!"}))
        .unwrap();
    let unsaved = w.editor().text.to_string();
    // A direct in-place edit is observed by the grammar file's own stamp.
    write(
        &grammar,
        &JSON.replace("Velocity Notes", "Velocity Notes Reloaded"),
    );
    wait(&mut w);
    assert_eq!(
        w.editor().language.borrow().as_ref().unwrap().name,
        "Velocity Notes Reloaded"
    );
    assert_eq!(w.editor().text.to_string(), unsaved);
    // New nested directories are discovered through their already-observed parent.
    write(
        &data.join("Packages/Notes/Nested/New/added.tmLanguage.json"),
        &JSON
            .replace("Velocity Notes", "Added Notes")
            .replace("vnote", "added-note"),
    );
    w.wait_resources();
    assert!(
        w.palette
            .iter()
            .any(|p| p.caption == "Set Syntax: Added Notes")
    );
    std::fs::remove_file(&grammar).unwrap();
    wait(&mut w);
    assert_eq!(
        w.editor().language.borrow().as_ref().unwrap().name,
        "Plain Text"
    );
    assert!(
        !w.palette
            .iter()
            .any(|p| p.caption == "Set Syntax: Velocity Notes Reloaded")
    );
    assert!(
        w.set_file_type("Packages/User/does-not-exist.tmLanguage.json")
            .is_err()
    );
    let xml_file = temp.path().join("sample.vxnote");
    write(&xml_file, "TODO\n");
    open(&mut w, &xml_file);
    assert_eq!(
        w.editor().language.borrow().as_ref().unwrap().name,
        "Velocity XML Notes"
    );
}

fn archive(path: &Path, entries: &[(&str, &str)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, contents) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn installed_archives_loose_overrides_bundled_overrides_and_live_archive_reload() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let package = data.join("Installed Packages/Notes.sublime-package");
    archive(&package, &[("notes.tmLanguage.json", JSON)]);
    let mut w = Workspace::new(Text::new(), Some(data.clone()), None, false).unwrap();
    let file = temp.path().join("sample.vnote");
    write(&file, "TODO\n");
    open(&mut w, &file);
    assert_eq!(
        w.editor().language.borrow().as_ref().unwrap().name,
        "Velocity Notes"
    );
    archive(
        &package,
        &[(
            "notes.tmLanguage.json",
            &JSON.replace("Velocity Notes", "Archive Edited Notes"),
        )],
    );
    wait(&mut w);
    assert_eq!(
        w.editor().language.borrow().as_ref().unwrap().name,
        "Archive Edited Notes"
    );
    write(
        &data.join("Packages/Notes/notes.tmLanguage.json"),
        &JSON.replace("Velocity Notes", "Loose Notes"),
    );
    wait(&mut w);
    assert_eq!(
        w.editor().language.borrow().as_ref().unwrap().name,
        "Loose Notes"
    );
    write(
        &data.join("Packages/Rust/Rust.sublime-syntax"),
        "name: User Rust\nfile_extensions: [rs]\nscope: source.user-rust\ncontexts: {main: []}\n",
    );
    w.wait_resources();
    assert_eq!(
        w.assets
            .syntaxes
            .find_syntax_by_path("Packages/Rust/Rust.sublime-syntax")
            .unwrap()
            .name,
        "User Rust"
    );
    std::fs::remove_file(data.join("Packages/Notes/notes.tmLanguage.json")).unwrap();
    wait(&mut w);
    assert_eq!(
        w.editor().language.borrow().as_ref().unwrap().name,
        "Archive Edited Notes"
    );
}

#[test]
fn corrupt_grammar_and_cache_keep_valid_grammars_with_readonly_cache_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    write(&data.join("Packages/User/good.tmLanguage.json"), JSON);
    write(&data.join("Packages/User/bad.tmLanguage.json"), "{ broken");
    // A file at Cache makes cache writes fail on every platform, independent of UID.
    write(&data.join("Cache"), "cache writes unavailable");
    let mut w = Workspace::new(Text::new(), Some(data.clone()), None, false).unwrap();
    assert!(w.messages.is_empty(), "{:?}", w.messages);
    assert_eq!(
        w.resource_reports,
        ["1 syntax warnings (Packages/User/bad.tmLanguage.json), see Velocity.log"]
    );
    assert!(
        w.assets
            .syntaxes
            .warnings()
            .iter()
            .any(|m| m.contains("bad.tmLanguage.json"))
    );
    let file = temp.path().join("sample.vnote");
    write(&file, "TODO\n");
    open(&mut w, &file);
    assert_eq!(
        w.editor().language.borrow().as_ref().unwrap().name,
        "Velocity Notes"
    );
    std::fs::remove_file(data.join("Cache")).unwrap();
    let _ = Workspace::new(Text::new(), Some(data.clone()), None, false).unwrap();
    for cache in std::fs::read_dir(data.join("Cache/Syntaxes")).unwrap() {
        std::fs::write(cache.unwrap().path(), "corrupt cache").unwrap();
    }
    let rebuilt = Workspace::new(Text::new(), Some(data), None, false).unwrap();
    assert!(
        rebuilt
            .assets
            .syntaxes
            .find_syntax_by_extension("vnote")
            .is_some()
    );
}

/// The packaged app ships `Packages/Default/Find Results.sublime-syntax` as a resource file,
/// identical to the internal grammar. That copy must reuse the shipped syntax set instead of
/// rebuilding every grammar on each launch (it cost ~1.5 s per launch in the packaged app);
/// a changed copy is still a real override.
#[test]
fn identical_internal_grammar_resource_reuses_the_shipped_syntax_set() {
    use crate::bundled_syntax::{FIND_RESULTS_PATH, INTERNAL_SYNTAXES};
    let base = crate::language::Assets::packed()
        .unwrap_or_else(crate::language::Assets::bundled)
        .unwrap();
    let packed = crate::language::Assets::packed().is_some();
    let shipped_resources = || {
        let mut resources = vel_config::resources::Resources::default();
        crate::shipped::register(&crate::language::packages_root(), &mut resources);
        resources
    };
    let find_results = INTERNAL_SYNTAXES
        .iter()
        .find(|(path, _)| *path == FIND_RESULTS_PATH)
        .unwrap()
        .1;

    let mut identical = shipped_resources();
    identical.insert(
        FIND_RESULTS_PATH,
        find_results.as_bytes().to_vec(),
        "/Applications/Velocity.app/Contents/Resources/Packages/Default/Find Results.sublime-syntax"
            .into(),
    );
    let reused =
        crate::language::Assets::with_package_overlay(&base, None, &identical, packed).unwrap();
    assert!(
        std::sync::Arc::ptr_eq(&base, &reused),
        "an identical internal grammar resource rebuilt the syntax set"
    );

    let mut changed = shipped_resources();
    changed.insert(
        FIND_RESULTS_PATH,
        format!("{find_results}\n# user edit\n").into_bytes(),
        "/tmp/profile/Packages/Default/Find Results.sublime-syntax".into(),
    );
    let rebuilt =
        crate::language::Assets::with_package_overlay(&base, None, &changed, packed).unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(&base, &rebuilt),
        "a changed Find Results grammar must still override"
    );
}
