//! A deliberately small syntax loader, not a Sublime parser.
use anyhow::{Context as _, Result, bail};
use serde::Serialize;
use std::{collections::{BTreeMap, HashMap, HashSet}, fs, path::{Path, PathBuf}};
use yaml_rust2::{Yaml, YamlLoader};

#[derive(Debug, Clone, Serialize)]
pub struct Context {
    pub name: String,
    /// Indices into Syntax.patterns, preserving include order (including duplicates).
    pub patterns: Vec<usize>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Skips {
    pub external_includes: BTreeMap<String, usize>,
    pub missing_includes: BTreeMap<String, usize>,
    pub include_cycles: BTreeMap<String, usize>,
    pub variable_errors: BTreeMap<String, usize>,
    pub ignored_prototypes: usize,
    pub ignored_embeds: usize,
    pub ignored_with_prototype: usize,
}

#[derive(Debug, Serialize)]
pub struct Syntax {
    pub name: String,
    pub files: Vec<String>,
    pub patterns: Vec<String>,
    pub contexts: Vec<Context>,
    pub skips: Skips,
}

#[derive(Default)]
struct Raw {
    variables: BTreeMap<String, String>,
    contexts: BTreeMap<String, Vec<Yaml>>,
    files: Vec<String>,
}

fn read_inherited(path: &Path, chain: &mut HashSet<PathBuf>) -> Result<Raw> {
    let path = path.canonicalize()?;
    if !chain.insert(path.clone()) { bail!("inheritance cycle: {}", path.display()); }
    let s = fs::read_to_string(&path)?;
    let docs = YamlLoader::load_from_str(&s).with_context(|| format!("YAML {}", path.display()))?;
    let doc = &docs[0];
    let mut raw = Raw::default();
    let parents: Vec<&str> = if let Some(p) = doc["extends"].as_str() { vec![p] }
        else { doc["extends"].as_vec().map(|v| v.iter().filter_map(Yaml::as_str).collect()).unwrap_or_default() };
    for parent in parents {
        // Official JS/HTML use a same-directory Plain parent. This is inheritance,
        // not cross-file include expansion. Refuse scope/path escape inheritance.
        if parent.contains('/') || parent.contains('\\') || parent.starts_with("scope:") {
            bail!("unsupported inheritance: {parent}");
        }
        let base = read_inherited(&path.parent().unwrap().join(parent), chain)?;
        raw.variables.extend(base.variables);
        raw.contexts.extend(base.contexts);
        raw.files.extend(base.files);
    }
    if let Some(vars) = doc["variables"].as_hash() {
        for (k, v) in vars {
            if let (Some(k), Some(v)) = (k.as_str(), v.as_str()) { raw.variables.insert(k.into(), v.into()); }
        }
    }
    if let Some(ctxs) = doc["contexts"].as_hash() {
        for (k, v) in ctxs {
            if let (Some(k), Some(v)) = (k.as_str(), v.as_vec()) {
                let prepend = v.iter().any(|x| x["meta_prepend"].as_bool() == Some(true));
                let append = v.iter().any(|x| x["meta_append"].as_bool() == Some(true));
                let mut rules = v.clone();
                if let Some(old) = raw.contexts.remove(k) {
                    if prepend { rules.extend(old); }
                    else if append { rules = old.into_iter().chain(rules).collect(); }
                }
                raw.contexts.insert(k.into(), rules);
            }
        }
    }
    raw.files.push(path.display().to_string());
    chain.remove(&path);
    Ok(raw)
}

fn expand(s: &str, vars: &BTreeMap<String, String>, chain: &mut Vec<String>) -> Result<String> {
    let mut rest = s;
    let mut out = String::new();
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i + 2..].find("}}") else { out.push_str(&rest[i..]); return Ok(out); };
        let key = &rest[i + 2..i + 2 + j];
        if chain.iter().any(|x| x == key) { bail!("variable cycle: {} -> {key}", chain.join(" -> ")); }
        let value = vars.get(key).with_context(|| format!("undefined variable: {key}"))?;
        chain.push(key.into());
        out.push_str(&expand(value, vars, chain)?);
        chain.pop();
        rest = &rest[i + j + 4..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Anonymous push/set/branch contexts get stable synthetic names, but are not
/// flattened into their parent's matches: they are separate runtime contexts.
fn anonymous(name: &str, rules: &[Yaml], out: &mut BTreeMap<String, Vec<Yaml>>) {
    for (i, rule) in rules.iter().enumerate() {
        for key in ["push", "set", "branch"] {
            if let Some(items) = rule[key].as_vec() {
                if items.iter().any(|v| v.as_hash().is_some()) {
                    let n = format!("{name}/@{i}:{key}");
                    out.insert(n.clone(), items.clone());
                    anonymous(&n, items, out);
                } else {
                    for (j, item) in items.iter().enumerate() {
                        if let Some(nested) = item.as_vec() {
                            let n = format!("{name}/@{i}:{key}:{j}");
                            out.insert(n.clone(), nested.clone());
                            anonymous(&n, nested, out);
                        }
                    }
                }
            }
        }
    }
}

fn flatten(name: &str, all: &BTreeMap<String, Vec<Yaml>>, stack: &mut Vec<String>,
    expanded: &HashMap<(String, usize), usize>, out: &mut Vec<usize>, skips: &mut Skips) {
    if stack.iter().any(|s| s == name) {
        *skips.include_cycles.entry(format!("{} -> {name}", stack.join(" -> "))).or_default() += 1;
        return;
    }
    let Some(rules) = all.get(name) else { *skips.missing_includes.entry(name.into()).or_default() += 1; return; };
    stack.push(name.into());
    for (i, rule) in rules.iter().enumerate() {
        if let Some(include) = rule["include"].as_str() {
            if include == "prototype" { continue; }
            if include.contains(':') || include.contains('#') || include.ends_with(".sublime-syntax") {
                *skips.external_includes.entry(include.into()).or_default() += 1;
            } else {
                flatten(include, all, stack, expanded, out, skips);
            }
        }
        if let Some(id) = expanded.get(&(name.to_string(), i)) { out.push(*id); }
    }
    stack.pop();
}

pub fn load(name: &str, path: &Path) -> Result<Syntax> {
    let mut raw = read_inherited(path, &mut HashSet::new())?;
    let mut skips = Skips::default();
    skips.ignored_prototypes = raw.contexts.remove("prototype").map(|v| v.len()).unwrap_or(0);
    let mut extra = BTreeMap::new();
    for (n, r) in &raw.contexts { anonymous(n, r, &mut extra); }
    raw.contexts.extend(extra);
    let mut patterns = Vec::new();
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut expanded = HashMap::new();
    for (n, r) in &raw.contexts {
        for (i, rule) in r.iter().enumerate() {
            skips.ignored_embeds += usize::from(!rule["embed"].is_badvalue());
            skips.ignored_with_prototype += usize::from(!rule["with_prototype"].is_badvalue());
            if let Some(p) = rule["match"].as_str() {
                match expand(p, &raw.variables, &mut Vec::new()) {
                    Ok(p) => {
                        let id = *ids.entry(p.clone()).or_insert_with(|| { let id = patterns.len(); patterns.push(p); id });
                        expanded.insert((n.clone(), i), id);
                    }
                    Err(e) => { *skips.variable_errors.entry(e.to_string()).or_default() += 1; }
                }
            }
        }
    }
    let contexts = raw.contexts.keys().map(|n| {
        let mut p = Vec::new();
        flatten(n, &raw.contexts, &mut Vec::new(), &expanded, &mut p, &mut skips);
        Context { name: n.clone(), patterns: p }
    }).collect();
    Ok(Syntax { name: name.into(), files: raw.files, patterns, contexts, skips })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recursive_variables_and_cycles() {
        let vars = BTreeMap::from([("a".into(), "{{b}}+".into()), ("b".into(), "[abc]".into())]);
        assert_eq!(expand("^{{a}}$", &vars, &mut vec![]).unwrap(), "^[abc]+$");
        let vars = BTreeMap::from([("a".into(), "{{b}}".into()), ("b".into(), "{{a}}".into())]);
        assert!(expand("{{a}}", &vars, &mut vec![]).is_err());
    }
    #[test]
    fn includes_preserve_priority_and_terminate_cycles() {
        let y = YamlLoader::load_from_str("a:\n - match: x\n - include: b\n - match: z\nb:\n - match: y\n - include: a\n").unwrap();
        let all: BTreeMap<_, _> = y[0].as_hash().unwrap().iter().map(|(k,v)| (k.as_str().unwrap().to_string(),v.as_vec().unwrap().clone())).collect();
        let e = HashMap::from([(("a".into(), 0),0),(("a".into(),2),2),(("b".into(),0),1)]);
        let mut out = vec![]; let mut skips = Skips::default();
        flatten("a", &all, &mut vec![], &e, &mut out, &mut skips);
        assert_eq!(out, vec![0,1,2]); assert_eq!(skips.include_cycles.len(),1);
    }
}
