use std::{
    env, fs,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};
use vel_config::{Value, json::Document};
use vel_snippet::SnippetFile;

#[path = "src/bundled_syntax.rs"]
mod bundled_syntax;
#[path = "build/dispatch.rs"]
mod dispatch;
#[path = "build/i18n.rs"]
mod i18n;

fn main() -> io::Result<()> {
    let root = Path::new("../../third_party/sublime-packages");
    println!("cargo:rerun-if-changed={}", root.display());
    println!("cargo:rerun-if-changed=src/bundled_syntax.rs");
    println!("cargo:rerun-if-changed=resources/Packages/Default/Find Results.sublime-syntax");
    let mut files = Vec::new();
    inventory(root, root, &mut files)?;
    // The runtime binary-searches these slash-separated resource IDs.
    files.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    #[cfg(windows)]
    if env::var("TARGET").is_ok_and(|target| target == "x86_64-pc-windows-msvc") {
        windows_resources(&out)?;
    }
    dispatch::generate(&out)?;
    i18n::generate(&out)?;
    let index = fs::File::create(out.join("startup-syntaxes.bin"))?;
    // Context bodies in this index are already individually compressed and lazy.
    // The build and the legacy runtime loader use exactly the same corpus/linker.
    if root.is_dir() {
        let set = vel_syntax::SyntaxSet::load_roots(&[root], &bundled_syntax::INTERNAL_SYNTAXES)
            .map_err(io::Error::other)?;
        let mut index = BufWriter::new(index);
        bincode::serialize_into(&mut index, &set).map_err(io::Error::other)?;
        index.flush()?;
    }
    let mut out = BufWriter::new(fs::File::create(out.join("shipped.rs"))?);
    writeln!(out, "static SHIPPED: &[File] = &[")?;
    for (name, path) in files {
        if name.ends_with(".py") {
            writeln!(
                out,
                "File {{ path: {name:?}, data: None, meta: Meta::None }},"
            )?;
            continue;
        }
        let absolute = path.canonicalize()?;
        let data = format!(
            "Some(include_bytes!({:?}))",
            absolute.to_str().expect("UTF-8 corpus path")
        );
        let meta = if name.ends_with(".sublime-snippet") {
            match SnippetFile::parse(&fs::read_to_string(&path)?) {
                Ok(s) => format!(
                    "Meta::Snippet {{ scope: {:?}, trigger: {:?}, description: {:?} }}",
                    s.scope, s.tab_trigger, s.description
                ),
                Err(e) => format!("Meta::Invalid({:?})", e.to_string()),
            }
        } else if name.ends_with(".sublime-completions") || name.ends_with(".sublime-settings") {
            match Document::parse(&fs::read_to_string(&path)?) {
                Ok(doc) if name.ends_with(".sublime-completions") => {
                    let parent = doc.value["scope"].as_str().unwrap_or("");
                    let mut scopes = vec![parent];
                    if let Some(items) = doc.value["completions"].as_array() {
                        scopes.extend(items.iter().filter_map(|entry| entry["scope"].as_str()));
                    }
                    scopes.sort_unstable();
                    scopes.dedup();
                    format!("Meta::Completion(&{scopes:?})")
                }
                Ok(doc) => match doc.value["extensions"].as_array() {
                    Some(items) => {
                        let values = items.iter().filter_map(Value::as_str).collect::<Vec<_>>();
                        format!("Meta::Extensions(&{values:?})")
                    }
                    None => "Meta::None".into(),
                },
                Err(e) => format!("Meta::Invalid({e:?})"),
            }
        } else {
            "Meta::None".into()
        };
        writeln!(
            out,
            "File {{ path: {name:?}, data: {data}, meta: {meta} }},"
        )?;
    }
    writeln!(out, "];")?;
    out.flush()
}

// Native MSVC builds have the SDK resource compiler; cargo check from another
// host does not link binaries and must not require an SDK.
#[cfg(windows)]
fn windows_resources(out: &Path) -> io::Result<()> {
    use std::process::Command;
    let version = env!("CARGO_PKG_VERSION");
    let numeric = format!(
        "{},{},{},0",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH")
    );
    let packaging = Path::new("packaging").canonicalize()?;
    let icon = packaging.join("velocity.ico");
    let source = packaging.join("velocity.rc");
    println!("cargo:rerun-if-changed=packaging/velocity.ico");
    println!("cargo:rerun-if-changed=packaging/velocity.rc");
    let template = fs::read_to_string(source)?;
    for (bin, subsystem) in [
        ("velocity", "Velocity editor"),
        ("vel", "Velocity console client"),
    ] {
        let rc = out.join(format!("{bin}.rc"));
        let res = out.join(format!("{bin}.res"));
        let script = template
            .replace("@VERSION@", version)
            .replace("@VERSION_COMMAS@", &numeric)
            .replace("@EXE@", bin)
            .replace("@DESCRIPTION@", subsystem);
        // Use the working directory and /I for paths with spaces or non-ASCII.
        fs::write(&rc, script)?;
        let mut compiler = find_msvc_tools::find("x86_64-pc-windows-msvc", "rc.exe")
            .unwrap_or_else(|| Command::new("rc.exe"));
        let status = compiler
            .current_dir(packaging.as_path())
            .arg("/nologo")
            .arg("/I")
            .arg(icon.parent().expect("icon directory"))
            .arg("/fo")
            .arg(&res)
            .arg(&rc)
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "rc.exe failed to compile {bin}: {status}"
            )));
        }
        println!("cargo:rustc-link-arg-bin={bin}={}", res.display());
    }
    Ok(())
}

fn inventory(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    if dir == root && !dir.is_dir() {
        // Source archives without the submodule retain the existing syntax-loader error.
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Clean-room boundary: only public language resources, never Sublime's
        // Default package, shipped UI resources, or the content of Python plugins.
        if name.starts_with('.') || (dir == root && name == "Default") {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            inventory(root, &entry.path(), out)?;
        } else if kind.is_file()
            && [
                ".sublime-settings",
                ".sublime-build",
                ".sublime-snippet",
                ".sublime-completions",
                ".sublime-syntax",
                ".sublime-macro",
                ".sublime-keymap",
                ".tmLanguage",
                ".tmLanguage.json",
                ".tmPreferences",
                ".py",
            ]
            .iter()
            .any(|ext| name.ends_with(ext))
        {
            let path = entry.path();
            let relative = path.strip_prefix(root).expect("inventory relative path");
            let name = format!("Packages/{}", vel_config::paths::slash_separated(relative));
            out.push((name, path));
        }
    }
    Ok(())
}
