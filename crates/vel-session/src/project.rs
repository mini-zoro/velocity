//! Comment/trailing-comma project parsing and atomic project/workspace paths.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
/// A folder's persisted discovery rules.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Folder {
    /// Absolute or project-relative directory.
    pub path: PathBuf,
    /// Optional sidebar label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Directory globs to hide.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folder_exclude_patterns: Vec<String>,
    /// File globs to hide.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_exclude_patterns: Vec<String>,
    /// Files admitted before exclusions (an empty list admits all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_include_patterns: Vec<String>,
    /// Directories admitted before exclusions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folder_include_patterns: Vec<String>,
    /// Extension data, retained when the project is edited.
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
    /// Files excluded from text navigation/search.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binary_file_patterns: Vec<String>,
    /// Whether directory symlinks are followed; `None` keeps the key absent (the default
    /// applies) so saving a project never pins the current default into the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_symlinks: Option<bool>,
}
/// Shareable `.sublime-project`; unknown keys survive editing and Save As.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    /// Discovery roots.
    pub folders: Vec<Folder>,
    /// Project editor settings.
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub settings: Map<String, Value>,
    /// Inline named build definitions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub build_systems: Vec<Value>,
    /// Extension data retained verbatim semantically.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl Project {
    /// Parses comments/trailing commas using Velocity's common JSON parser.
    pub fn parse(source: &str) -> Result<Self, String> {
        let doc = vel_config::json::Document::parse(source)?;
        serde_json::from_value(doc.value).map_err(|e| e.to_string())
    }
    /// Reads and parses one explicit project file.
    pub fn load(path: &Path) -> Result<Self, String> {
        Self::parse(&std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?)
    }
    /// Resolves relative folder paths against the project file's directory.
    pub fn resolve(&mut self, path: &Path) {
        let base = path.parent().unwrap_or(Path::new("."));
        for folder in &mut self.folders {
            if folder.path.is_relative() {
                folder.path = base.join(&folder.path);
            }
        }
    }
    /// Rebase originally relative paths for Save As; absolute paths stay absolute.
    pub fn rebase(&mut self, old: &Path, new: &Path) {
        let old = std::path::absolute(old).unwrap_or_else(|_| old.into());
        let new = std::path::absolute(new).unwrap_or_else(|_| new.into());
        let old_base = old.parent().unwrap_or(Path::new("."));
        let new_base = new.parent().unwrap_or(Path::new("."));
        for folder in &mut self.folders {
            if folder.path.is_relative() {
                folder.path = relative_path(&old_base.join(&folder.path), new_base);
            }
        }
    }
    /// Saves without truncating an existing project on failure.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        super::atomic_write(path, |f| {
            use std::io::Write;
            f.write_all(&bytes).map_err(|e| e.to_string())
        })
    }
}
/// Computes a relative path, preserving OS resolution across symlinked parents.
/// Missing descendants are resolved against their nearest existing ancestor.
pub fn relative_path(target: &Path, base: &Path) -> PathBuf {
    fn resolved(path: &Path) -> Option<PathBuf> {
        let absolute = std::path::absolute(path).ok()?;
        let mut ancestor = absolute.as_path();
        let mut tail = Vec::new();
        loop {
            if let Ok(mut canonical) = std::fs::canonicalize(ancestor) {
                for part in tail.into_iter().rev() {
                    canonical.push(part);
                }
                return Some(canonical);
            }
            tail.push(ancestor.file_name()?);
            ancestor = ancestor.parent()?;
        }
    }
    fn lexical(target: &Path, base: &Path) -> PathBuf {
        #[cfg(windows)]
        let (display_target, display_base) = (
            vel_config::paths::display_path(target),
            vel_config::paths::display_path(base),
        );
        #[cfg(windows)]
        let (target, base) = (display_target.as_ref(), display_base.as_ref());
        let normalize = |p: &Path| {
            let mut out = PathBuf::new();
            for part in p.components() {
                match part {
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir => {
                        out.pop();
                    }
                    part => out.push(part.as_os_str()),
                }
            }
            out
        };
        let target = normalize(target);
        let base = normalize(base);
        let target_parts: Vec<_> = target.components().collect();
        let base_parts: Vec<_> = base.components().collect();
        let common = target_parts
            .iter()
            .zip(&base_parts)
            .take_while(|(a, b)| {
                #[cfg(windows)]
                return vel_config::paths::components_equal(**a, **b);
                #[cfg(not(windows))]
                return a == b;
            })
            .count();
        if common == 0 && target.is_absolute() {
            return target;
        }
        let mut out = PathBuf::new();
        for _ in common..base_parts.len() {
            out.push("..");
        }
        for part in &target_parts[common..] {
            out.push(part.as_os_str());
        }
        if out.as_os_str().is_empty() {
            out.push(".");
        }
        out
    }
    let relative = lexical(target, base);
    if [target, base, &relative].iter().any(|path| {
        path.components()
            .any(|c| c == std::path::Component::ParentDir)
    }) {
        // A lexical `..` may escape to a different parent after following a symlink.
        // Prefer a relative path between the canonical parents only when needed.
        if let (Some(target), Some(base)) = (resolved(target), resolved(base)) {
            if resolved(&base.join(&relative)).as_ref() == Some(&target) {
                return relative;
            }
            return lexical(&target, &base);
        }
        // Without resolvable parents, an absolute spelling is the safe fallback.
        return std::path::absolute(target).unwrap_or_else(|_| target.into());
    }
    relative
}
/// Workspace lives beside its project, including on a fresh directory tree.
pub fn workspace_path(project: &Path) -> PathBuf {
    project.with_extension("sublime-workspace")
}
/// The shareable definition corresponding to a named project or its companion workspace.
pub fn project_path(path: &Path) -> PathBuf {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sublime-workspace"))
    {
        path.with_extension("sublime-project")
    } else {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_as_rebases_a_relative_project_filename_and_retains_folder_extensions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::create_dir(dir.path().join("new")).unwrap();
        let original = dir.path().join("original.sublime-project");
        std::fs::write(
            &original,
            r#"{"folders":[{"path":"src","custom":{"kept":true}}]}"#,
        )
        .unwrap();
        let relative = relative_path(&original, &std::env::current_dir().unwrap());
        assert!(relative.is_relative());
        let mut project = Project::load(&relative).unwrap();
        let new = dir.path().join("new/project.sublime-project");
        project.rebase(&relative, &new);
        project.save(&new).unwrap();
        let saved = Project::load(&new).unwrap();
        assert!(saved.folders[0].path.is_relative());
        assert_eq!(
            new.parent()
                .unwrap()
                .join(&saved.folders[0].path)
                .canonicalize()
                .unwrap(),
            dir.path().join("src").canonicalize().unwrap()
        );
        assert_eq!(saved.folders[0].extra["custom"]["kept"], true);
    }
}

#[cfg(all(windows, test))]
mod windows_tests {
    use super::*;
    #[test]
    fn relative_paths_respect_drive_and_unc_share_boundaries() {
        assert_eq!(
            relative_path(Path::new(r"D:\Work\src"), Path::new(r"c:\Work")),
            Path::new(r"D:\Work\src")
        );
        assert_eq!(
            relative_path(Path::new(r"C:\WORK\src"), Path::new(r"c:\work\old")),
            Path::new(r"..\src")
        );
        assert_eq!(
            relative_path(
                Path::new(r"\\SERVER\Shared\src"),
                Path::new(r"\\server\shared")
            ),
            Path::new("src")
        );
        assert_eq!(
            relative_path(
                Path::new(r"\\server\Shared\src"),
                Path::new(r"\\server\Elsewhere")
            ),
            Path::new(r"\\server\Shared\src")
        );
    }
}

#[cfg(all(test, unix))]
mod review15_tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn relative_paths_resolve_across_symlinked_parents_and_missing_descendants() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let physical = root.join("physical/deep");
        let sibling = root.join("sibling");
        std::fs::create_dir_all(&physical).unwrap();
        std::fs::create_dir(&sibling).unwrap();
        symlink(&physical, root.join("alias")).unwrap();
        let base = root.join("alias");
        let target = sibling.join("src");
        std::fs::create_dir(&target).unwrap();
        let relative = relative_path(&target, &base);
        assert_eq!(
            base.join(&relative).canonicalize().unwrap(),
            target.canonicalize().unwrap()
        );
        assert_eq!(
            relative_path(&target, &physical),
            Path::new("../../sibling/src")
        );
        let missing = target.join("not-created/nested");
        let relative_missing = relative_path(&missing, &base);
        std::fs::create_dir_all(&missing).unwrap();
        assert_eq!(
            base.join(relative_missing).canonicalize().unwrap(),
            missing.canonicalize().unwrap()
        );
        let mut project = Project::parse(r#"{"folders":[{"path":"sibling/src"}]}"#).unwrap();
        project.rebase(
            &root.join("old.sublime-project"),
            &base.join("new.sublime-project"),
        );
        project.resolve(&base.join("new.sublime-project"));
        assert_eq!(
            project.folders[0].path.canonicalize().unwrap(),
            target.canonicalize().unwrap()
        );
    }
}
