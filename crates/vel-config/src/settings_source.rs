//! Last valid settings per physical source, shared by independent window loaders.
use crate::{json::Document, resources::Resource};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

type Sources = BTreeMap<(PathBuf, String), Arc<Vec<u8>>>;
static VALID: OnceLock<Mutex<BTreeMap<PathBuf, Sources>>> = OnceLock::new();

pub(crate) fn retain_valid(
    root: &Path,
    entries: &mut BTreeMap<String, Resource>,
    warnings: &mut Vec<String>,
) {
    // Keep a single version of each currently installed file, not a reload history.
    // Parsing and copying stay outside the shared lock. Removed files/profiles are
    // pruned, so independent resource workers do not grow a stale-source backlog.
    // Settings precedence is per source: https://www.sublimetext.com/docs/settings.html
    let parsed = entries
        .values()
        .filter(|r| r.path.ends_with(".sublime-settings") && r.origin.starts_with(root))
        .map(|r| {
            let valid = r.text().and_then(Document::parse).and_then(|doc| {
                if doc.value.is_object() {
                    Ok(())
                } else {
                    Err("settings must be an object at line 1".into())
                }
            });
            (
                (r.origin.clone(), r.path.clone()),
                valid,
                Arc::new(r.bytes.to_vec()),
            )
        })
        .collect::<Vec<_>>();
    let replacements = {
        let mut profiles = VALID
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        profiles.retain(|path, _| path == root || path.is_dir());
        let sources = profiles.entry(root.to_owned()).or_default();
        sources.retain(|path, _| parsed.iter().any(|(p, _, _)| p == path));
        parsed
            .into_iter()
            .filter_map(|(path, result, bytes)| match result {
                Ok(()) => {
                    sources.insert(path, bytes);
                    None
                }
                Err(error) => Some((path.clone(), error, sources.get(&path).cloned())),
            })
            .collect::<Vec<_>>()
    };
    for ((_, path), error, previous) in replacements {
        let resource = entries.get_mut(&path).expect("loaded settings resource");
        // The application's resource report summarizes the origin before ": ".
        // Include the parse location there so the non-modal summary retains it.
        let location = error
            .split_once(" at line ")
            .map(|(_, rest)| format!("line {rest}"))
            .unwrap_or_else(|| "line 1".into());
        warnings.push(format!(
            "{} ({location}): {error}",
            resource.origin.display()
        ));
        resource.bytes = previous
            .map_or_else(|| b"{}".to_vec(), |bytes| (*bytes).clone())
            .into();
    }
}
