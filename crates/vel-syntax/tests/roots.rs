//! Ordered roots, virtual resources and content-addressed invalidation.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use vel_syntax::{CacheStatus, MetadataSet, ParseState, ScopeStackOp, SyntaxSet};

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("vel-roots-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn write(root: &Path, path: &str, source: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, source).unwrap();
}
fn definition(name: &str, scope: &str) -> String {
    format!("name: {name}\nscope: source.{scope}\ncontexts: {{ main: [] }}\n")
}

#[test]
fn parallel_source_reads_preserve_override_order_and_content_invalidation() {
    let tmp = Scratch::new("parallel-content");
    let first = tmp.dir("first");
    let second = tmp.dir("second");
    let cache = tmp.dir("cache");
    for i in 0..64 {
        let file = format!("Pkg/Item{i:02}.sublime-syntax");
        write(
            &first,
            &file,
            &definition(&format!("Old{i}"), &format!("old{i}")),
        );
        write(
            &second,
            &file,
            &definition(&format!("New{i}"), &format!("new{i}")),
        );
    }
    for expected in [CacheStatus::Yaml, CacheStatus::Hit] {
        let (set, status) = SyntaxSet::load_cached_roots(&[&first, &second], &[], &cache).unwrap();
        assert_eq!(status, expected);
        assert_eq!(set.syntaxes().len(), 64);
        assert!(set.find_syntax_by_name("New42").is_some());
        assert!(set.find_syntax_by_name("Old42").is_none());
    }
    // Changed shadowed input must invalidate too; equal byte length is deliberate.
    write(
        &first,
        "Pkg/Item42.sublime-syntax",
        &definition("Alt42", "alt42"),
    );
    let (set, status) = SyntaxSet::load_cached_roots(&[&first, &second], &[], &cache).unwrap();
    assert_eq!(status, CacheStatus::Yaml);
    assert!(set.find_syntax_by_name("New42").is_some());
    write(
        &second,
        "Pkg/Item42.sublime-syntax",
        &definition("Fix42", "fix42"),
    );
    let (set, status) = SyntaxSet::load_cached_roots(&[&first, &second], &[], &cache).unwrap();
    assert_eq!(status, CacheStatus::Yaml);
    assert!(set.find_syntax_by_name("Fix42").is_some());
    assert!(set.find_syntax_by_name("New42").is_none());
}
fn preferences(settings: &str) -> String {
    format!(
        "<plist version=\"1.0\"><dict><key>scope</key><string>source.fixture</string><key>settings</key><dict>{settings}</dict></dict></plist>"
    )
}

#[test]
fn overrides_are_selected_before_inheritance_and_package_includes_link() {
    let tmp = Scratch::new("links");
    let shipped = tmp.dir("shipped");
    let user = tmp.dir("packages");
    let cache = tmp.dir("cache");
    write(
        &shipped,
        "Pkg/Base.sublime-syntax",
        &definition("Old display", "old"),
    );
    write(
        &shipped,
        "Pkg/Child.sublime-syntax",
        "name: Child\nscope: source.child\nextends: Packages/Pkg/Base.sublime-syntax\ncontexts: {}\n",
    );
    write(
        &shipped,
        "Pkg/Host.sublime-syntax",
        "name: Host\nscope: source.host\ncontexts:\n  main:\n    - include: Packages/Pkg/Base.sublime-syntax\n",
    );
    write(
        &user,
        "Pkg/Base.sublime-syntax",
        "name: Renamed base\nscope: source.base\ncontexts:\n  main:\n    - match: x\n      scope: constant.selected\n",
    );
    let extra = [(
        "Packages/User/Extra.sublime-syntax",
        "name: Extra\nscope: source.extra\ncontexts: { main: [] }\n",
    )];
    for status in [CacheStatus::Yaml, CacheStatus::Hit] {
        let (set, actual) =
            SyntaxSet::load_cached_roots(&[&shipped, &user], &extra, &cache).unwrap();
        assert_eq!(actual, status);
        assert!(set.warnings().is_empty(), "{:?}", set.warnings());
        assert_eq!(set.syntaxes().len(), 4);
        assert!(set.find_syntax_by_name("Old display").is_none());
        assert_eq!(
            set.find_syntax_by_path("Packages/Pkg/Base.sublime-syntax")
                .unwrap()
                .name,
            "Renamed base"
        );
        let set = Arc::new(set);
        for name in ["Child", "Host"] {
            let mut state = ParseState::new(set.clone(), set.find_syntax_by_name(name).unwrap());
            assert!(state.parse_line("x\n").unwrap().ops.iter().any(|(_, op)| matches!(op, ScopeStackOp::Push(s) if s.build_string() == "constant.selected")));
        }
    }
}

#[test]
fn digest_covers_order_shadowed_sources_extras_removal_and_relocation() {
    let tmp = Scratch::new("digest");
    let a = tmp.dir("a");
    let b = tmp.dir("b");
    let cache = tmp.dir("cache");
    let file = "Pkg/Test.sublime-syntax";
    write(&a, file, &definition("Before", "before"));
    write(&b, file, &definition("After", "after"));
    let load = |roots: &[&Path], extra: &[(&str, &str)]| {
        SyntaxSet::load_cached_roots(roots, extra, &cache).unwrap()
    };
    assert_eq!(load(&[&a, &b], &[]).1, CacheStatus::Yaml);
    assert_eq!(load(&[&a, &b], &[]).1, CacheStatus::Hit);
    let moved = tmp.0.join("moved");
    std::fs::rename(&b, &moved).unwrap();
    let (set, status) = load(&[&a, &moved], &[]);
    assert_eq!(status, CacheStatus::Hit);
    assert_eq!(set.syntaxes()[0].path, "Packages/Pkg/Test.sublime-syntax");
    assert!(
        set.find_syntax_by_path(moved.join(file).to_str().unwrap())
            .is_some()
    );
    assert!(
        set.find_syntax_by_path(b.join(file).to_str().unwrap())
            .is_none()
    );
    assert_eq!(load(&[&moved, &a], &[]).0.syntaxes()[0].name, "Before");
    write(&a, file, &definition("Shadow changed", "changed"));
    assert_eq!(load(&[&a, &moved], &[]).1, CacheStatus::Yaml);
    let first = definition("Virtual", "virtual");
    let extra = [("Packages/Pkg/Test.sublime-syntax", first.as_str())];
    let (set, status) = load(&[&a, &moved], &extra);
    assert_eq!(status, CacheStatus::Yaml);
    assert_eq!(set.syntaxes().len(), 1);
    assert_eq!(set.syntaxes()[0].name, "Virtual");
    assert_eq!(load(&[&a, &moved], &extra).1, CacheStatus::Hit);
    let changed = definition("Virtual changed", "virtual");
    assert_eq!(
        load(&[&a, &moved], &[(extra[0].0, &changed)]).1,
        CacheStatus::Yaml
    );
    write(
        &moved,
        "Pkg/Added.tmLanguage.json",
        r#"{"name":"JSON fixture","scopeName":"source.jsonfixture","patterns":[]}"#,
    );
    assert_eq!(load(&[&a, &moved], &[]).0.syntaxes().len(), 2);
    std::fs::remove_file(moved.join(file)).unwrap();
    let (set, status) = load(&[&a, &moved], &[]);
    assert_eq!(status, CacheStatus::Yaml);
    assert!(set.find_syntax_by_name("Shadow changed").is_some());
    for entry in std::fs::read_dir(&cache).unwrap() {
        std::fs::write(entry.unwrap().path(), b"broken").unwrap();
    }
    assert_eq!(load(&[&a, &moved], &[]).1, CacheStatus::Yaml);
    assert_eq!(load(&[], &extra).0.syntaxes()[0].name, "Virtual");
    assert!(SyntaxSet::load_roots(&[], &[("../bad.sublime-syntax", &first)]).is_err());
    assert!(SyntaxSet::load_roots(&[&tmp.0.join("absent")], &[]).is_err());
}

#[test]
fn preference_replacement_removes_shadowed_keys_and_distinct_rules_still_merge() {
    let tmp = Scratch::new("metadata");
    let shipped = tmp.dir("shipped");
    let user = tmp.dir("user");
    write(
        &shipped,
        "Pkg/Test.tmPreferences",
        &preferences("<key>removed</key><true/><key>choice</key><string>shipped</string>"),
    );
    write(
        &shipped,
        "Pkg/Other.tmPreferences",
        &preferences("<key>keep</key><true/>"),
    );
    write(
        &user,
        "Pkg/Test.tmPreferences",
        &preferences("<key>choice</key><string>user</string>"),
    );
    let meta = MetadataSet::load_directories(&[&shipped, &user]).unwrap();
    let stack = "source.fixture".parse().unwrap();
    let value = meta.metadata_for(&stack);
    assert_eq!(value.string("choice"), Some("user"));
    assert!(value.boolean("removed").is_none());
    assert_eq!(value.boolean("keep"), Some(true));
    let extra = preferences("<key>choice</key><string>virtual</string>");
    let meta = MetadataSet::load_roots(
        &[&shipped, &user],
        &[("Packages/Pkg/Test.tmPreferences", &extra)],
    )
    .unwrap();
    assert_eq!(meta.metadata_for(&stack).string("choice"), Some("virtual"));
}

#[test]
fn distinct_resources_with_the_same_name_keep_root_priority_and_user_is_last() {
    let tmp = Scratch::new("ties");
    let a = tmp.dir("a");
    let b = tmp.dir("b");
    write(
        &a,
        "Zulu/Test.sublime-syntax",
        &definition("Shared", "zulu"),
    );
    write(
        &a,
        "User/Test.sublime-syntax",
        &definition("Shared", "user"),
    );
    let set = SyntaxSet::load_directories(&[&a]).unwrap();
    assert_eq!(set.syntaxes().len(), 2);
    assert_eq!(
        set.find_syntax_by_name("Shared")
            .unwrap()
            .scope
            .build_string(),
        "source.user"
    );
    write(
        &b,
        "Alpha/Test.sublime-syntax",
        &definition("Shared", "laterroot"),
    );
    let set = SyntaxSet::load_directories(&[&a, &b]).unwrap();
    assert_eq!(set.syntaxes().len(), 3);
    assert_eq!(
        set.find_syntax_by_name("Shared")
            .unwrap()
            .scope
            .build_string(),
        "source.laterroot"
    );
    let first = definition("First extra", "first");
    let last = definition("Last extra", "last");
    let set = SyntaxSet::load_roots(
        &[&a],
        &[
            ("User/Test.sublime-syntax", &first),
            ("Packages\\User\\Test.sublime-syntax", &last),
        ],
    )
    .unwrap();
    assert_eq!(set.syntaxes().len(), 2);
    assert!(set.find_syntax_by_name("First extra").is_none());
    assert!(set.find_syntax_by_name("Last extra").is_some());
}

#[cfg(unix)]
#[test]
fn linked_package_directories_and_sources_invalidate_by_target_content() {
    use std::os::unix::fs::symlink;
    let tmp = Scratch::new("symlinks");
    let source = tmp.dir("source");
    let root = tmp.dir("root");
    let cache = tmp.dir("cache");
    write(
        &source,
        "Test.sublime-syntax",
        &definition("Before", "before"),
    );
    write(
        &source,
        "Test.tmPreferences",
        &preferences("<key>linked</key><true/>"),
    );
    symlink(&source, root.join("Pkg")).unwrap();
    symlink(tmp.0.join("missing"), root.join("Dangling.sublime-syntax")).unwrap();
    assert_eq!(
        SyntaxSet::load_cached_roots(&[&root], &[], &cache)
            .unwrap()
            .1,
        CacheStatus::Yaml
    );
    assert_eq!(
        SyntaxSet::load_cached_roots(&[&root], &[], &cache)
            .unwrap()
            .1,
        CacheStatus::Hit
    );
    write(
        &source,
        "Test.sublime-syntax",
        &definition("After", "after"),
    );
    let (set, status) = SyntaxSet::load_cached_roots(&[&root], &[], &cache).unwrap();
    assert_eq!(status, CacheStatus::Yaml);
    assert!(set.find_syntax_by_name("After").is_some());
    assert_eq!(
        MetadataSet::load_directories(&[&root])
            .unwrap()
            .metadata_for(&"source.fixture".parse().unwrap())
            .boolean("linked"),
        Some(true)
    );
}
