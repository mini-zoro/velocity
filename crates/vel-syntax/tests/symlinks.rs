//! Regression: symlinked definitions must load (users link packages into `Packages/`, and the
//! editor stages bundled definitions as file symlinks); directories named like grammars must
//! not be mistaken for files.
// Creating symlinks on Windows needs Developer Mode or elevation; the loader path is shared.
#![cfg(unix)]
use std::path::PathBuf;
use vel_syntax::SyntaxSet;

const DEF: &str =
    "name: Linked\nscope: source.linked\nfile_extensions: [lnk]\ncontexts:\n  main: []\n";

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vel-syntax-symlink-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn symlinked_definitions_load_and_grammar_named_directories_are_skipped() {
    let base = scratch("load");
    let source = base.join("source");
    let packages = base.join("packages");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(packages.join("Pkg/Decoy.tmLanguage")).unwrap();
    std::fs::write(source.join("Linked.sublime-syntax"), DEF).unwrap();
    std::os::unix::fs::symlink(
        source.join("Linked.sublime-syntax"),
        packages.join("Pkg/Linked.sublime-syntax"),
    )
    .unwrap();
    // A dangling link must be ignored rather than fail the whole load.
    std::os::unix::fs::symlink(
        base.join("missing"),
        packages.join("Pkg/Dangling.sublime-syntax"),
    )
    .unwrap();

    let set = SyntaxSet::load_from_folder(&packages).expect("folder load");
    assert!(
        set.find_syntax_by_name("Linked").is_some(),
        "symlinked definition not loaded"
    );

    let (cached, _) = SyntaxSet::load_cached(&packages, &base.join("cache")).expect("cached load");
    assert!(
        cached.find_syntax_by_name("Linked").is_some(),
        "cached loader skipped the symlink"
    );
    let _ = std::fs::remove_dir_all(&base);
}
