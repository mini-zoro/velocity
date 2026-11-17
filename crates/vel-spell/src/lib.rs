#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
use spellbook::Dictionary;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    ops::Range,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
use unicode_segmentation::UnicodeSegmentation;
use vel_buffer::{Bias, ChangeId, ChangeSet};
use vel_highlight::ScopeSnapshot;
use vel_scope::Selector;
use vel_text::Text;

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
fn generation() -> u64 {
    NEXT_GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// Bundled SCOWL-derived US English affixes, unmodified from the pinned source.
pub const ENGLISH_AFFIXES: &str = include_str!("../dictionaries/en_US/en_US.aff");
/// Bundled SCOWL size-60 US English words, unmodified from the pinned source.
pub const ENGLISH_WORDS: &str = include_str!("../dictionaries/en_US/en_US.dic");
/// Full upstream license, provenance, notices and source credits; ship with binary distributions.
pub const ENGLISH_LICENSE: &str = include_str!("../dictionaries/en_US/LICENSE");

/// macOS's documented user dictionary folder. Other platforms have no macOS folder.
/// Source: <https://www.sublimetext.com/download>, build 4107, Spell Checking.
pub fn system_spelling_directory() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Spelling"))
    } else {
        None
    }
}

/// An unknown word and its absolute UTF-8 byte interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Misspelling {
    /// Original casing and spelling.
    pub word: String,
    /// Absolute document bytes, never a partial scalar.
    pub range: Range<usize>,
}
/// Dictionary/coordinates/version failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Dictionary resource could not be read.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Hunspell data is unsupported or malformed.
    #[error("dictionary: {0}")]
    Dictionary(String),
    /// Package-relative dictionary paths cannot escape the Packages root.
    #[error("invalid Packages dictionary path")]
    PackagePath,
    /// Requested range is outside the document or splits UTF-8.
    #[error("invalid spelling range")]
    Range,
    /// Scope/current text versions disagree.
    #[error("stale spelling scopes or edit")]
    Stale,
    /// Invalid old-to-new text descriptor.
    #[error(transparent)]
    Change(#[from] vel_diff::Error),
}
/// Hunspell dictionary with per-user additions/ignores. Share the dictionary across buffers.
#[derive(Debug, Clone)]
pub struct SpellChecker {
    dictionary: Arc<Dictionary>,
    ignored: BTreeSet<String>,
    added: BTreeSet<String>,
    generation: u64,
}
impl SpellChecker {
    /// Parses any UTF-8 Hunspell affix/word pair. Load dictionaries off the main thread.
    pub fn new(affixes: &str, words: &str) -> Result<Self, Error> {
        let dictionary =
            Dictionary::new(affixes, words).map_err(|e| Error::Dictionary(e.to_string()))?;
        Ok(Self {
            dictionary: Arc::new(dictionary),
            ignored: BTreeSet::new(),
            added: BTreeSet::new(),
            generation: generation(),
        })
    }
    /// Loads the shipped permissive US English dictionary once per process.
    pub fn english() -> Result<Self, Error> {
        static DICTIONARY: OnceLock<Result<Arc<Dictionary>, String>> = OnceLock::new();
        let dictionary = DICTIONARY.get_or_init(|| {
            Dictionary::new(ENGLISH_AFFIXES, ENGLISH_WORDS)
                .map(Arc::new)
                .map_err(|e| e.to_string())
        });
        Ok(Self {
            dictionary: dictionary
                .as_ref()
                .map_err(|e| Error::Dictionary(e.clone()))?
                .clone(),
            ignored: BTreeSet::new(),
            added: BTreeSet::new(),
            generation: generation(),
        })
    }
    /// Loads a user dictionary such as `Packages/User/en_GB.dic` with its adjacent `.aff`.
    /// `packages_root` is the actual Packages directory; symlinks escaping it are rejected.
    pub fn from_package(packages_root: &Path, resource: &str) -> Result<Self, Error> {
        let relative = Path::new(resource)
            .strip_prefix("Packages")
            .map_err(|_| Error::PackagePath)?;
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || relative.extension().is_none_or(|e| e != "dic")
        {
            return Err(Error::PackagePath);
        }
        let root = packages_root.canonicalize()?;
        let dic = root.join(relative).canonicalize()?;
        let aff = root.join(relative).with_extension("aff").canonicalize()?;
        if !dic.starts_with(&root) || !aff.starts_with(&root) {
            return Err(Error::PackagePath);
        }
        Self::new(&fs::read_to_string(aff)?, &fs::read_to_string(dic)?)
    }
    /// Finds available UTF-8 dictionary pairs in immediate `Packages/*/` folders.
    pub fn package_dictionaries(packages_root: &Path) -> Result<Vec<PathBuf>, Error> {
        let mut result = Vec::new();
        for package in fs::read_dir(packages_root)? {
            let path = package?.path();
            if !path.is_dir() {
                continue;
            }
            for file in fs::read_dir(path)? {
                let path = file?.path();
                if path.extension().is_some_and(|e| e == "dic")
                    && path.with_extension("aff").is_file()
                {
                    result.push(path);
                }
            }
        }
        result.sort();
        Ok(result)
    }
    /// Lists paired Hunspell dictionaries in a system spelling folder. A missing
    /// folder is normal; no directory is created as a side effect.
    pub fn system_dictionaries(root: &Path) -> Result<Vec<PathBuf>, Error> {
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(_) => return Ok(Vec::new()),
        };
        let mut result = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "dic")
                && path.is_file()
                && path.with_extension("aff").is_file()
            {
                result.push(path);
            }
        }
        result.sort();
        Ok(result)
    }
    /// Loads an explicitly selected system dictionary, enforcing its folder boundary
    /// just like package dictionaries (including symlink destinations).
    pub fn from_system_directory(root: &Path, path: &Path) -> Result<Self, Error> {
        if path.extension().is_none_or(|e| e != "dic") {
            return Err(Error::PackagePath);
        }
        let root = root.canonicalize()?;
        let dic = path.canonicalize()?;
        let aff = path.with_extension("aff").canonicalize()?;
        if !dic.starts_with(&root) || !aff.starts_with(&root) {
            return Err(Error::PackagePath);
        }
        Self::new(&fs::read_to_string(aff)?, &fs::read_to_string(dic)?)
    }
    /// Replaces `ignored_words`. Matching is Unicode lowercase, including capitalized forms.
    pub fn set_ignored_words(&mut self, words: impl IntoIterator<Item = impl AsRef<str>>) {
        self.ignored = words
            .into_iter()
            .map(|s| s.as_ref().to_lowercase())
            .collect();
        self.generation = generation();
    }
    /// Adds a user word. Persist `added_words()` in User settings outside this engine.
    pub fn add_word(&mut self, word: impl AsRef<str>) {
        self.added.insert(word.as_ref().to_lowercase());
        self.generation = generation();
    }
    /// User additions, normalized for deterministic persistence.
    pub fn added_words(&self) -> &BTreeSet<String> {
        &self.added
    }
    /// Tests a single complete word, including Hunspell affixes and casing rules.
    pub fn check(&self, word: &str) -> bool {
        let normalized = word.replace('’', "'");
        let lower = normalized.to_lowercase();
        self.ignored.contains(&lower)
            || self.added.contains(&lower)
            || self.dictionary.check(&normalized)
    }
    /// Suggestions are potentially expensive: request on a worker when the user invokes them.
    pub fn suggestions(&self, word: &str, limit: usize) -> Vec<String> {
        if limit == 0 {
            return Vec::new();
        }
        let mut suggestions = Vec::new();
        self.dictionary.suggest(word, &mut suggestions);
        suggestions.truncate(limit);
        suggestions
    }
    /// Checks only complete words inside supplied absolute UTF-8 ranges. Ranges may overlap.
    /// Requested line bodies are tokenized once, so scope edges cannot create partial words.
    pub fn check_words(
        &self,
        text: &Text,
        ranges: &[Range<usize>],
    ) -> Result<Vec<Misspelling>, Error> {
        let ranges = normalize_ranges(text, ranges)?;
        let mut lines = BTreeSet::new();
        for range in &ranges {
            if range.is_empty() {
                continue;
            }
            let end = text.floor_char_boundary(range.end - 1);
            lines.extend(text.byte_to_line(range.start)..=text.byte_to_line(end));
        }
        let mut result = Vec::new();
        for line in lines {
            let start = text.line_to_byte(line);
            let body = text.line(line).to_string();
            for (range, word) in word_parts(&body) {
                let range = start + range.start..start + range.end;
                if ranges
                    .iter()
                    .any(|allowed| allowed.start <= range.start && allowed.end >= range.end)
                    && !self.check(word)
                {
                    result.push(Misspelling {
                        word: word.to_owned(),
                        range,
                    });
                }
            }
        }
        Ok(result)
    }
    /// Filters exact vel-highlight scope runs through `spelling_selector`, then checks them.
    /// Unavailable/speculative scopes produce no diagnostics; retry when exact runs arrive.
    pub fn check_scoped(
        &self,
        scopes: &ScopeSnapshot,
        selector: &Selector,
    ) -> Result<Vec<Misspelling>, Error> {
        self.check_words(
            &scopes.text,
            &selected_ranges(scopes, selector, scopes.range.clone()),
        )
    }
}
fn normalize_ranges(text: &Text, ranges: &[Range<usize>]) -> Result<Vec<Range<usize>>, Error> {
    let mut ranges = ranges.to_vec();
    for r in &ranges {
        if r.start > r.end || !text.is_char_boundary(r.start) || !text.is_char_boundary(r.end) {
            return Err(Error::Range);
        }
    }
    ranges.sort_by_key(|r| r.start);
    let mut result: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        if let Some(last) = result.last_mut()
            && r.start <= last.end
        {
            last.end = last.end.max(r.end);
        } else {
            result.push(r);
        }
    }
    Ok(result)
}
fn selected_ranges(
    scopes: &ScopeSnapshot,
    selector: &Selector,
    lines: Range<usize>,
) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    for line in &scopes.lines {
        if !line.exact || !lines.contains(&line.line) || line.line >= scopes.text.len_lines() {
            continue;
        }
        let offset = scopes.text.line_to_byte(line.line);
        for run in line.runs.iter() {
            if scopes
                .stacks
                .get(run.stack)
                .is_some_and(|stack| selector.matches(stack))
            {
                result.push(offset + run.start as usize..offset + run.end as usize);
            }
        }
    }
    result
}
// ST-BEHAVIOR: unverified [TOK1]: skip URL/email whitespace tokens, split snake_case and
// camelCase/acronym boundaries, retain apostrophes, skip digit-containing components.
// Uppercase letters remain at the start of the following component (ST4 changelog fix).
fn word_parts(text: &str) -> Vec<(Range<usize>, &str)> {
    let mut result = Vec::new();
    let mut offset = 0;
    for segment in text.split_inclusive(char::is_whitespace) {
        let lower = segment.to_ascii_lowercase();
        if lower.contains("://")
            || lower.contains('@')
            || lower
                .trim_start_matches(['(', '[', '<', '"'])
                .starts_with("www.")
            || lower.contains("mailto:")
        {
            offset += segment.len();
            continue;
        }
        for (start, word) in segment.unicode_word_indices() {
            let chars: Vec<_> = word.char_indices().collect();
            let mut begin = 0;
            let mut emit = |a: usize, b: usize| {
                if a < b {
                    let w = &word[a..b];
                    if w.chars().any(char::is_alphabetic) && !w.chars().any(char::is_numeric) {
                        result.push((offset + start + a..offset + start + b, w));
                    }
                }
            };
            for (i, &(at, ch)) in chars.iter().enumerate() {
                if ch == '_' {
                    emit(begin, at);
                    begin = at + 1;
                    continue;
                }
                let previous = i.checked_sub(1).map(|j| chars[j].1);
                let next = chars.get(i + 1).map(|c| c.1);
                if at > begin
                    && ch.is_uppercase()
                    && (previous.is_some_and(char::is_lowercase)
                        || (previous.is_some_and(char::is_uppercase)
                            && next.is_some_and(char::is_lowercase)))
                {
                    emit(begin, at);
                    begin = at;
                }
            }
            emit(begin, word.len());
        }
        offset += segment.len();
    }
    result
}
#[derive(Debug)]
struct CachedLine {
    allowed: Vec<Range<usize>>,
    generation: u64,
    errors: Vec<Misspelling>, // line-local
}
/// Bounded viewport cache. Edits invalidate touched lines; unchanged cached lines relocate.
#[derive(Debug)]
pub struct DocumentSpelling {
    text: Text,
    version: ChangeId,
    selector: Selector,
    cache: BTreeMap<usize, CachedLine>,
    checked_lines: usize,
}
impl DocumentSpelling {
    /// Creates an empty per-buffer cache using the user's parsed spelling selector.
    pub fn new(text: Text, version: ChangeId, selector: Selector) -> Self {
        Self {
            text,
            version,
            selector,
            cache: BTreeMap::new(),
            checked_lines: 0,
        }
    }
    /// Number of lines actually rechecked by the most recent viewport request.
    pub fn checked_lines(&self) -> usize {
        self.checked_lines
    }
    /// Replaces the selector and invalidates cached diagnostics.
    pub fn set_selector(&mut self, selector: Selector) {
        self.selector = selector;
        self.cache.clear();
    }
    /// Applies one matching post-edit snapshot and descriptor; never scans document contents.
    pub fn apply_change(
        &mut self,
        text: Text,
        version: ChangeId,
        changes: &ChangeSet,
    ) -> Result<(), Error> {
        if version <= self.version {
            return Err(Error::Stale);
        }
        vel_diff::validate_change(&self.text, &text, changes)?;
        let touched: Vec<_> = changes
            .changes()
            .iter()
            .map(|c| {
                self.text.byte_to_line(c.old_range.start)..=self.text.byte_to_line(c.old_range.end)
            })
            .collect();
        let mut next = BTreeMap::new();
        for (line, cached) in std::mem::take(&mut self.cache) {
            if touched.iter().any(|r| r.contains(&line)) {
                continue;
            }
            let point = changes.transform_point(self.text.line_to_byte(line), Bias::Right);
            next.insert(text.byte_to_line(point), cached);
        }
        self.cache = next;
        self.text = text;
        self.version = version;
        Ok(())
    }
    /// Checks only missing/edited visible lines whose current scope runs are exact.
    /// Changing additions/ignores also invalidates lazily via the checker's generation.
    pub fn check_lines(
        &mut self,
        checker: &SpellChecker,
        scopes: &ScopeSnapshot,
        lines: Range<usize>,
    ) -> Result<Vec<Misspelling>, Error> {
        if scopes.version != self.version || scopes.text.len_bytes() != self.text.len_bytes() {
            return Err(Error::Stale);
        }
        if lines.start > lines.end || lines.end > self.text.len_lines() {
            return Err(Error::Range);
        }
        self.checked_lines = 0;
        let mut errors = Vec::new();
        for line in lines {
            let offset = self.text.line_to_byte(line);
            let allowed = normalize_ranges(
                &self.text,
                &selected_ranges(scopes, &self.selector, line..line + 1),
            )?;
            let local: Vec<_> = allowed
                .iter()
                .map(|r| r.start - offset..r.end - offset)
                .collect();
            if self
                .cache
                .get(&line)
                .is_none_or(|v| v.generation != checker.generation || v.allowed != local)
            {
                let mut errors = checker.check_words(&self.text, &allowed)?;
                for error in &mut errors {
                    error.range = error.range.start - offset..error.range.end - offset;
                }
                self.cache.insert(
                    line,
                    CachedLine {
                        allowed: local,
                        generation: checker.generation,
                        errors,
                    },
                );
                self.checked_lines += 1;
            }
            errors.extend(self.cache[&line].errors.iter().cloned().map(|mut e| {
                e.range = offset + e.range.start..offset + e.range.end;
                e
            }));
        }
        while self.cache.len() > 4096 {
            self.cache.pop_first();
        }
        Ok(errors)
    }
}
