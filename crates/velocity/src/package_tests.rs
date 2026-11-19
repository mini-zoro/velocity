//! App integration tests: use local generated packages and never initialize native services.
use super::*;
use std::io::Write;
use vel_config::json;
use vel_config::keymap::{Key, Resolution};
fn archive(dir: &Path) -> PathBuf {
    let path = dir.join("Round13.sublime-package");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    for (name, source) in [
        ("package-metadata.json", r#"{"version":"1.2.3"}"#),
        (
            "fixture.sublime-syntax",
            "name: Round 13\nscope: source.round13\nfile_extensions: [r13]\ncontexts: {main: []}\n",
        ),
        (
            "fixture.sublime-color-scheme",
            r##"{"name":"Round 13","globals":{"background":"#123456","foreground":"#ffffff"},"rules":[]}"##,
        ),
        (
            "fixture.sublime-theme",
            r##"{"rules":[{"class":"button","layer0.tint":"#123456"}]}"##,
        ),
        (
            "Default.sublime-keymap",
            r#"[{"keys":["ctrl+alt+9"],"command":"insert","args":{"characters":"package key"}}]"#,
        ),
        (
            "fixture.sublime-snippet",
            "<snippet><content><![CDATA[package ${1:field}]]></content><tabTrigger>r13</tabTrigger><description>Round 13 snippet</description></snippet>",
        ),
        (
            "fixture.sublime-completions",
            r#"{"scope":"source.round13","completions":["round13completion"]}"#,
        ),
        ("fixture.sublime-build", r#"{"cmd":["true"]}"#),
        (
            "fixture.sublime-commands",
            r#"[{"caption":"Round13 Command","command":"new_file"}]"#,
        ),
        (
            "Main.sublime-menu",
            r#"[{"id":"help","caption":"Help","children":[{"caption":"Round13 Menu","command":"new_file"}]}]"#,
        ),
        ("plugin.py", "raise AssertionError('must never run')"),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(source.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    path
}
fn manager_job(
    data: &Path,
    job: impl FnOnce(vel_packages::PackageManager) -> Result<(), String> + Send + 'static,
) {
    let data = data.to_owned();
    crate::release::enqueue(move || job(vel_packages::PackageManager::new(data)))
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(120))
        .unwrap()
        .unwrap();
}
fn assert_contributions(config: &Configuration, present: bool) {
    for suffix in [
        ".sublime-syntax",
        ".sublime-color-scheme",
        ".sublime-theme",
        ".sublime-keymap",
        ".sublime-snippet",
        ".sublime-completions",
        ".sublime-build",
        ".sublime-menu",
    ] {
        assert_eq!(
            config
                .resources
                .matching(suffix)
                .iter()
                .any(|r| r.path.starts_with("Packages/Round13/")),
            present,
            "{suffix}"
        );
    }
    assert_eq!(
        config
            .palette
            .iter()
            .any(|p| p.caption == "Round13 Command"),
        present
    );
    assert_eq!(
        config
            .assets
            .syntaxes
            .syntaxes()
            .iter()
            .any(|s| s.name == "Round 13"),
        present
    );
    assert_eq!(
        config
            .catalog
            .snippets
            .contains_key("Packages/Round13/fixture.sublime-snippet"),
        present
    );
    assert_eq!(
        config.build_systems.iter().any(|b| b.name == "fixture"),
        present
    );
    assert_eq!(
        config.schemes.contains_key("fixture.sublime-color-scheme"),
        present
    );
}
#[test]
fn package_worker_install_disable_enable_remove_reloads_every_resource_and_keeps_buffers() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("profile");
    let source = archive(temp.path());
    let mut w =
        Workspace::new(Text::from("keep unsaved"), Some(data.clone()), None, false).unwrap();
    w.editor_mut()
        .run("insert", &json!({"characters":"local "}))
        .unwrap();
    let buffer = w.editor().buffer.id();
    let original = w.editor().text.clone();
    manager_job(&data, move |m| {
        m.install_local("Round13", &source)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    w.request_resource_reload();
    w.wait_resources();
    assert_contributions(
        &Configuration::load(Some(data.clone()), None).unwrap(),
        true,
    );
    assert!(
        matches!(w.editor_mut().resolve_key(&Key::parse("ctrl+alt+9").unwrap()),Resolution::Command(i) if i.args["characters"]=="package key")
    );
    let mut inherited = w.ignored_packages();
    inherited.insert("KeepDisabled".into());
    manager_job(&data, move |m| {
        m.set_enabled_inheriting("Round13", false, &inherited)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    w.request_resource_reload();
    w.wait_resources();
    assert_contributions(
        &Configuration::load(Some(data.clone()), None).unwrap(),
        false,
    );
    assert!(w.ignored_packages().contains("KeepDisabled"));
    assert!(w.ignored_packages().contains("Round13"));
    assert!(
        !matches!(w.editor_mut().resolve_key(&Key::parse("ctrl+alt+9").unwrap()),Resolution::Command(i) if i.args["characters"]=="package key")
    );
    manager_job(&data, |m| {
        m.set_enabled("Round13", true)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    w.request_resource_reload();
    w.wait_resources();
    assert_contributions(
        &Configuration::load(Some(data.clone()), None).unwrap(),
        true,
    );
    manager_job(&data, |m| {
        m.remove("Round13")
            .map(|paths| assert!(!paths.is_empty()))
            .map_err(|e| e.to_string())
    });
    w.request_resource_reload();
    w.wait_resources();
    assert_contributions(&Configuration::load(Some(data), None).unwrap(), false);
    assert_eq!(w.editor().buffer.id(), buffer);
    assert_eq!(w.editor().text, original);
    w.editor_mut().run("undo", &json!({})).unwrap();
    assert_eq!(w.editor().text.to_string(), "keep unsaved");
}
#[test]
fn explicit_index_command_and_ignored_bundled_grammar_filter_are_consistent() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    let user = data.join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("Preferences.sublime-settings"),r#"{"ignored_packages":["Rust","Vintage"],"package_index_url":"https://example.invalid/explicit.json"}"#).unwrap();
    let config = Configuration::load(Some(data.into()), None).unwrap();
    assert!(
        config
            .palette
            .iter()
            .any(|p| p.invocation.command == "package_check_upgrades")
    );
    assert!(
        !config
            .assets
            .syntaxes
            .syntaxes()
            .iter()
            .any(|s| s.name == "Rust")
    );
    assert!(!config.build_systems.iter().any(|b| b.name == "Cargo"));
    let fresh = Configuration::load(None, None).unwrap();
    assert!(
        !fresh
            .palette
            .iter()
            .any(|p| p.invocation.command == "package_check_upgrades")
    );
    assert!(
        fresh
            .palette
            .iter()
            .any(|p| p.invocation.command == "install_cli")
    );
    assert!(
        fresh
            .palette
            .iter()
            .any(|p| p.invocation.command == "open_crash_reports")
    );
}
