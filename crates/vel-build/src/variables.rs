//! Sublime-style build variables with nested defaults and regex replacements.
use std::{collections::BTreeMap, path::Path};
/// Named variables from the current file/project/window.
#[derive(Clone, Debug, Default)]
pub struct Variables(pub BTreeMap<String, String>);
impl Variables {
    /// Creates the documented file/project/folder/packages variables.
    pub fn new(
        file: Option<&Path>,
        folder: Option<&Path>,
        project: Option<&Path>,
        packages: Option<&Path>,
    ) -> Self {
        let mut result = Self::default();
        for (prefix, path) in [("file", file), ("project", project)] {
            if let Some(path) = path {
                result
                    .0
                    .insert(prefix.into(), path.to_string_lossy().into_owned());
                result.0.insert(
                    format!("{prefix}_path"),
                    path.parent()
                        .unwrap_or(Path::new(""))
                        .to_string_lossy()
                        .into_owned(),
                );
                result.0.insert(
                    format!("{prefix}_name"),
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                );
                result.0.insert(
                    format!("{prefix}_base_name"),
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                );
                result.0.insert(
                    format!("{prefix}_extension"),
                    path.extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        for (key, path) in [("folder", folder), ("packages", packages)] {
            if let Some(path) = path {
                result
                    .0
                    .insert(key.into(), path.to_string_lossy().into_owned());
            }
        }
        result.0.insert(
            "platform".into(),
            if cfg!(target_os = "macos") {
                "osx"
            } else if cfg!(windows) {
                "windows"
            } else {
                "linux"
            }
            .into(),
        );
        result
    }
    /// Expands strings once; substituted path contents are never reinterpreted as variables.
    pub fn expand(&self, source: &str) -> Result<String, String> {
        self.expand_depth(source, 0)
    }
    fn expand_depth(&self, source: &str, depth: usize) -> Result<String, String> {
        if depth > 32 {
            return Err("variable defaults exceed 32 levels".into());
        }
        let mut out = String::new();
        let mut i = 0;
        let bytes = source.as_bytes();
        while i < bytes.len() {
            if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'$') {
                out.push('$');
                i += 2;
                continue;
            }
            if bytes[i] != b'$' {
                let c = source[i..].chars().next().expect("character");
                out.push(c);
                i += c.len_utf8();
                continue;
            }
            i += 1;
            if bytes.get(i) == Some(&b'{') {
                i += 1;
                let start = i;
                let mut nesting = 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                        continue;
                    }
                    if bytes[i] == b'{' {
                        nesting += 1;
                    } else if bytes[i] == b'}' {
                        nesting -= 1;
                        if nesting == 0 {
                            break;
                        }
                    }
                    i += 1;
                }
                if nesting != 0 {
                    return Err("unclosed build variable".into());
                }
                out.push_str(&self.expression(&source[start..i], depth + 1)?);
                i += 1;
            } else {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                if i == start {
                    out.push('$');
                } else {
                    out.push_str(self.0.get(&source[start..i]).map_or("", String::as_str));
                }
            }
        }
        Ok(out)
    }
    fn expression(&self, expression: &str, depth: usize) -> Result<String, String> {
        let end = expression.find([':', '/']).unwrap_or(expression.len());
        let name = &expression[..end];
        let value = self.0.get(name).map_or("", String::as_str);
        let tail = &expression[end..];
        if let Some(default) = tail.strip_prefix(':') {
            return if value.is_empty() {
                self.expand_depth(default, depth)
            } else {
                Ok(value.into())
            };
        }
        if let Some(substitution) = tail.strip_prefix('/') {
            // Build substitutions use snippet formats, including first-only and g.
            // https://www.sublimetext.com/docs/build_systems.html#variables
            let source = format!("${{BUILD_VALUE/{substitution}}}");
            let mut environment = vel_snippet::Environment::default();
            environment.0.insert("BUILD_VALUE".into(), value.into());
            let expansion = vel_snippet::Snippet::parse(&source)
                .and_then(|s| {
                    s.expand(
                        &vel_text::Text::new(),
                        std::slice::from_ref(&(0..0)),
                        &environment,
                        vel_snippet::ExpansionOptions::default(),
                    )
                })
                .map_err(|e| e.to_string())?;
            return Ok(expansion.edits[0].text.clone());
        }
        Ok(value.into())
    }
}
