//! Comment/trailing-comma project parsing and atomic project/workspace paths.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
/// A folder's persisted discovery rules.
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Saves without truncating an existing project on failure.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        super::atomic_write(path, |f| {
            serde_json::to_writer_pretty(f, self).map_err(|e| e.to_string())
        })
    }
}
/// Workspace lives beside its project, including on a fresh directory tree.
pub fn workspace_path(project: &Path) -> PathBuf {
    project.with_extension("sublime-workspace")
}
