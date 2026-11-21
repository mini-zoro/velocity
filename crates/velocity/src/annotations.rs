//! Worker-owned diff and spelling. Publications are bound to buffer/version/configuration.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ops::Range,
    path::PathBuf,
    sync::{Arc, mpsc},
};
use vel_buffer::{Buffer, BufferId, ChangeId, ChangeSet, ViewId};
use vel_config::{Value, settings::Settings};
use vel_diff::{DocumentDiff, Hunk, InlineChange, InlineMode};
use vel_highlight::ScopeSnapshot;
use vel_spell::{DocumentSpelling, Misspelling, SpellChecker};
use vel_text::Text;

#[path = "fold_state.rs"]
pub(crate) mod fold_state;
// Public spelling_selector default (scope facts, not bundled package content):
// https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html#system-and-miscellaneous-settings
pub const SPELLING_SELECTOR: &str = "markup.raw, source string.quoted - punctuation - meta.preprocessor.c.include, source comment - source comment.block.preprocessor, -(source, constant, keyword, storage, support, variable, markup.underline.link, meta.tag)";
pub const ENGLISH: &str = "Packages/Language - English/en_US.dic";
#[derive(Clone, Debug, PartialEq)]
struct Config {
    path: Option<PathBuf>,
    packages: Option<PathBuf>,
    target: String,
    fallback: String,
    mini: Value,
    spell: bool,
    navigation_dictionary: bool,
    selector: String,
    dictionary: String,
    added: Vec<String>,
    ignored: Vec<String>,
    epoch: u64,
}
impl Config {
    fn new(
        settings: &Settings,
        path: Option<PathBuf>,
        packages: Option<PathBuf>,
        epoch: u64,
    ) -> Self {
        let words = |key| {
            settings
                .get(key)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            path,
            packages,
            fallback: settings
                .string("fallback_encoding", "Western (Windows 1252)")
                .into(),
            target: settings.string("git_diff_target", "index").into(),
            mini: settings
                .get("mini_diff")
                .cloned()
                .unwrap_or(Value::Bool(true)),
            spell: settings.bool("spell_check", false),
            navigation_dictionary: false,
            selector: settings
                .string("spelling_selector", SPELLING_SELECTOR)
                .into(),
            dictionary: settings.string("dictionary", ENGLISH).into(),
            added: words("added_words"),
            ignored: words("ignored_words"),
            epoch,
        }
    }
}
#[cfg(test)]
pub(crate) fn worker_configuration(settings: &Settings) -> impl std::fmt::Debug {
    Config::new(settings, None, None, 0)
}

#[derive(Clone, Debug)]
pub struct Publication {
    pub identity: (BufferId, ChangeId),
    pub epoch: u64,
    pub reference: Text,
    pub hunks: Arc<[Hunk]>,
    pub spelling: Arc<[Misspelling]>,
    pub inline_spans: BTreeMap<usize, Arc<[InlineChange]>>,
    pub navigation: Option<(u64, Option<Misspelling>)>,
    pub checked: Range<usize>,
    pub suggestions: Option<(String, Vec<String>)>,
    pub error: Option<String>,
}
#[derive(Debug)]
enum Job {
    Configure(Config, Text, Text, (BufferId, ChangeId)),
    Edit(Text, (BufferId, ChangeId), Option<ChangeSet>),
    Check(ViewId, Arc<ScopeSnapshot>, Range<usize>),
    Suggest(String),
    Inline(usize),
    Navigate(Arc<ScopeSnapshot>, usize, bool, u64),
}
#[derive(Debug)]
pub struct Annotations {
    allowed: (bool, bool),
    staged: VecDeque<Job>,
    tx: Option<mpsc::Sender<Job>>,
    rx: Option<mpsc::Receiver<Publication>>,
    config: Option<Config>,
    identity: Option<(BufferId, ChangeId)>,
    pub current: Option<Publication>,
    pub inline: BTreeSet<usize>,
    base: Text,
    path: Option<PathBuf>,
    packages: Option<PathBuf>,
    epoch: u64,
    serial: u64,
    settings_revision: u64,
    navigation_dictionary: bool,
    fold_states: fold_state::FoldStates,
    requests: BTreeMap<ViewId, (ChangeId, Range<usize>, usize)>,
    pub pending: usize,
}
impl Annotations {
    pub fn new(base: Text) -> Self {
        Self {
            allowed: (true, true),
            staged: VecDeque::new(),
            tx: None,
            rx: None,
            config: None,
            identity: None,
            current: None,
            inline: BTreeSet::new(),
            base,
            path: None,
            packages: None,
            epoch: 0,
            serial: 0,
            settings_revision: u64::MAX,
            navigation_dictionary: false,
            fold_states: Default::default(),
            requests: BTreeMap::new(),
            pending: 0,
        }
    }
    pub fn set_policy(&mut self, diff: bool, spell: bool) {
        if self.allowed != (diff, spell) {
            self.allowed = (diff, spell);
            self.refresh_reference();
        }
    }
    pub fn set_location(&mut self, path: Option<PathBuf>, packages: Option<PathBuf>) {
        if self.path != path || self.packages != packages {
            self.path = path;
            self.packages = packages;
            self.refresh_reference();
        }
    }
    pub fn reference(&mut self, base: Text, path: Option<PathBuf>, packages: Option<PathBuf>) {
        self.base = base;
        self.path = path;
        self.packages = packages;
        self.refresh_reference();
    }
    fn clear_current(&mut self) {
        if let Some(old) = self.current.take() {
            crate::language::retire(old);
        }
    }
    pub fn refresh_reference(&mut self) {
        self.epoch += 1;
        self.requests.clear();
        self.clear_current();
    }
    fn send(&mut self, job: Job) {
        self.staged.push_back(job);
        self.pending += 1;
    }
    /// Wakes analysis after scene construction so it does not compete with painting.
    pub fn flush(&mut self) {
        if self.staged.is_empty() {
            return;
        }
        if self.tx.is_none() {
            let (tx, jobs) = mpsc::channel();
            let (results, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("velocity-diff-spell".into())
                .spawn(move || worker(jobs, results))
                .expect("annotation worker");
            self.tx = Some(tx);
            self.rx = Some(rx);
        }
        for job in self.staged.drain(..) {
            if self.tx.as_ref().unwrap().send(job).is_err() {
                self.pending = self.pending.saturating_sub(1);
            }
        }
    }
    /// Feeds a directly observed transaction without scanning retained history maps.
    pub fn change(
        &mut self,
        buffer: &Buffer,
        settings: &Settings,
        from: ChangeId,
        changes: &ChangeSet,
    ) {
        if self.tx.is_none() && self.staged.is_empty() {
            return;
        }
        if self.identity == Some((buffer.id(), from))
            && self.settings_revision == settings.revision()
            && self.config.as_ref().is_some_and(|c| c.epoch == self.epoch)
        {
            let identity = (buffer.id(), buffer.change_id());
            if self
                .config
                .as_ref()
                .is_some_and(|c| c.mini != false || c.spell)
            {
                self.send(Job::Edit(
                    buffer.text().clone(),
                    identity,
                    Some(changes.clone()),
                ));
            }
            self.identity = Some(identity);
            self.requests.clear();
            self.inline.clear();
        } else {
            self.committed(buffer, settings);
        }
    }
    pub fn sync(&mut self, buffer: &Buffer, settings: &Settings) {
        let identity = (buffer.id(), buffer.change_id());
        let reconfigure = self.settings_revision != settings.revision()
            || self.config.as_ref().is_none_or(|c| c.epoch != self.epoch)
            || self.identity.is_none_or(|i| i.0 != buffer.id());
        if reconfigure {
            self.epoch += 1;
            let mut config = Config::new(
                settings,
                self.path.clone(),
                self.packages.clone(),
                self.epoch,
            );
            if !self.allowed.0 {
                config.mini = Value::Bool(false);
            }
            config.spell &= self.allowed.1;
            config.navigation_dictionary = self.navigation_dictionary && self.allowed.1;
            self.settings_revision = settings.revision();
            // No dictionary or diff work when both settings disable the features.
            if config.mini == false && !config.spell && !config.navigation_dictionary {
                self.config = Some(config);
                self.identity = Some(identity);
                self.clear_current();
                return;
            }
            self.send(Job::Configure(
                config.clone(),
                self.base.clone(),
                buffer.text().clone(),
                identity,
            ));
            self.config = Some(config);
            self.identity = Some(identity);
            self.requests.clear();
            self.clear_current();
            self.inline.clear();
        } else if self.identity != Some(identity) {
            if self
                .config
                .as_ref()
                .is_some_and(|c| c.mini == false && !c.spell && !c.navigation_dictionary)
            {
                self.identity = Some(identity);
                return;
            }
            let changes = self
                .identity
                .and_then(|(_, from)| buffer.changes_since(from));
            let single = changes.and_then(|mut c| {
                if c.len() == 1 {
                    Some(c.remove(0).1)
                } else {
                    None
                }
            });
            self.send(Job::Edit(buffer.text().clone(), identity, single));
            self.identity = Some(identity);
            self.requests.clear();
            self.inline.clear();
        }
    }
    /// Called at each directly observed transaction, before another command can edit again.
    pub fn committed(&mut self, buffer: &Buffer, settings: &Settings) {
        // Selection-only syncs also publish here: Find/Goto/undo/mouse must reveal
        // hidden endpoints before a later command or viewport can move them away.
        self.fold_states.observe(buffer);
        if self.tx.is_some() || !self.staged.is_empty() {
            self.sync(buffer, settings);
        }
    }
    pub(crate) fn folded(&mut self, buffer: &Buffer, view: ViewId) -> Vec<vel_buffer::Region> {
        self.fold_states.visible(buffer, view)
    }
    pub(crate) fn reset_folds(&mut self, view: ViewId) {
        self.fold_states.forget(view);
    }
    pub fn check(&mut self, view: ViewId, scopes: Arc<ScopeSnapshot>, lines: Range<usize>) {
        if !self.config.as_ref().is_some_and(|c| c.spell) {
            return;
        }
        let key = (scopes.version, lines.clone(), Arc::as_ptr(&scopes) as usize);
        if self.requests.get(&view) != Some(&key) {
            self.requests.insert(view, key);
            self.send(Job::Check(view, scopes, lines));
        }
    }
    /// Loads a dictionary for explicit navigation without enabling viewport checks
    /// or writing the user's spell_check preference. Reuse it for subsequent jumps.
    pub fn prepare_navigation(&mut self, buffer: &Buffer, settings: &Settings) {
        if !self.navigation_dictionary {
            self.navigation_dictionary = true;
            self.epoch += 1;
        }
        self.sync(buffer, settings);
    }
    pub fn navigate(&mut self, scopes: Arc<ScopeSnapshot>, caret: usize, forward: bool) -> u64 {
        self.serial += 1;
        let token = self.serial;
        self.send(Job::Navigate(scopes, caret, forward, token));
        token
    }
    pub fn load_inline(&mut self, row: usize) {
        self.send(Job::Inline(row));
    }
    pub fn suggest(&mut self, word: String) {
        self.send(Job::Suggest(word));
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.rx {
            for p in rx.try_iter() {
                self.pending = self.pending.saturating_sub(1);
                if self.identity == Some(p.identity) && self.epoch == p.epoch {
                    if let Some(old) = self.current.replace(p) {
                        crate::language::retire(old);
                    }
                    changed = true;
                }
            }
        }
        changed
    }
    pub fn exact(&self, buffer: BufferId, version: ChangeId) -> Option<&Publication> {
        self.current
            .as_ref()
            .filter(|p| p.identity == (buffer, version) && p.epoch == self.epoch)
    }
}
fn worker(jobs: mpsc::Receiver<Job>, results: mpsc::Sender<Publication>) {
    let mut state: Option<State> = None;
    while let Ok(job) = jobs.recv() {
        if let Job::Configure(config, base, text, identity) = job {
            state = Some(State::new(config, base, text, identity));
        } else if let Some(s) = &mut state {
            s.job(job);
        }
        if let Some(s) = &state {
            let _ = results.send(s.publication());
        }
    }
}
struct State {
    config: Config,
    identity: (BufferId, ChangeId),
    diff: DocumentDiff,
    spell: DocumentSpelling,
    checker: Option<SpellChecker>,
    enabled: bool,
    navigation: Option<(u64, Option<Misspelling>)>,
    inline_spans: BTreeMap<usize, Arc<[InlineChange]>>,
    spelling: BTreeMap<ViewId, Vec<Misspelling>>,
    checked: Range<usize>,
    suggestions: Option<(String, Vec<String>)>,
    error: Option<String>,
}
impl State {
    fn new(config: Config, base: Text, text: Text, identity: (BufferId, ChangeId)) -> Self {
        let mut reference = base;
        let mut repo = false;
        let mut error = None;
        if let Some(path) = &config.path
            && let Ok(repository) = vel_git::Repository::discover(path.parent().unwrap_or(path))
        {
            repo = true;
            let target = if config.target == "head" {
                vel_git::DiffTarget::Head
            } else {
                vel_git::DiffTarget::Index
            };
            match repository.blob(path, target) {
                Ok(Some(bytes)) => match reference_text(&bytes, &config.fallback) {
                    Ok(text) => reference = text,
                    Err(message) => error = Some(message),
                },
                Ok(None) => reference = Text::new(),
                Err(e) => error = Some(e.to_string()),
            }
        }
        if !repo && let Some(path) = &config.path {
            match vel_fileio::load(path, &config.fallback, |_| {}) {
                Ok(saved) => reference = saved.text,
                Err(e) => error = Some(e),
            }
        }
        let enabled = error.is_none() && (config.mini == true || (config.mini == "auto" && repo));
        let selector = vel_scope::Selector::parse(&config.selector)
            .unwrap_or_else(|_| vel_scope::Selector::parse(SPELLING_SELECTOR).unwrap());
        let checker = if config.spell || config.navigation_dictionary {
            let loaded = if config.dictionary == ENGLISH {
                SpellChecker::english()
            } else if std::path::Path::new(&config.dictionary).is_absolute() {
                vel_spell::system_spelling_directory()
                    .ok_or(vel_spell::Error::PackagePath)
                    .and_then(|root| {
                        SpellChecker::from_system_directory(
                            &root,
                            std::path::Path::new(&config.dictionary),
                        )
                    })
            } else if let Some(root) = &config.packages {
                SpellChecker::from_package(root, &config.dictionary)
            } else {
                Err(vel_spell::Error::PackagePath)
            };
            match loaded {
                Ok(mut c) => {
                    c.set_ignored_words(&config.ignored);
                    for w in &config.added {
                        c.add_word(w);
                    }
                    Some(c)
                }
                Err(e) => {
                    error = Some(e.to_string());
                    None
                }
            }
        } else {
            None
        };
        Self {
            diff: DocumentDiff::new(reference, text.clone(), identity.1),
            spell: DocumentSpelling::new(text, identity.1, selector),
            config,
            identity,
            checker,
            enabled,
            navigation: None,
            inline_spans: BTreeMap::new(),
            spelling: BTreeMap::new(),
            checked: 0..0,
            suggestions: None,
            error,
        }
    }
    fn job(&mut self, job: Job) {
        match job {
            Job::Edit(text, identity, changes) => {
                let applied = changes
                    .as_ref()
                    .is_some_and(|c| self.diff.apply_change(text.clone(), identity.1, c).is_ok());
                if !applied {
                    self.diff =
                        DocumentDiff::new(self.diff.reference().clone(), text.clone(), identity.1);
                }
                if changes.as_ref().is_none_or(|c| {
                    self.spell
                        .apply_change(text.clone(), identity.1, c)
                        .is_err()
                }) {
                    self.spell = DocumentSpelling::new(
                        text,
                        identity.1,
                        vel_scope::Selector::parse(&self.config.selector).unwrap_or_else(|_| {
                            vel_scope::Selector::parse(SPELLING_SELECTOR).unwrap()
                        }),
                    );
                }
                self.identity = identity;
                self.spelling.clear();
                self.inline_spans.clear();
                self.checked = 0..0;
                self.suggestions = None;
            }
            Job::Check(view, scopes, lines) if scopes.version == self.identity.1 => {
                if let Some(checker) = &self.checker {
                    match self.spell.check_lines(checker, &scopes, lines.clone()) {
                        Ok(errors) => {
                            self.spelling.insert(view, errors);
                            self.checked = self.checked.start.min(lines.start)
                                ..self.checked.end.max(lines.end);
                        }
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
            }
            Job::Navigate(scopes, caret, forward, token) if scopes.version == self.identity.1 => {
                let mut errors = Vec::new();
                if let Some(checker) = &self.checker {
                    for start in (0..scopes.text.len_lines()).step_by(128) {
                        if let Ok(mut page) = self.spell.check_lines(
                            checker,
                            &scopes,
                            start..(start + 128).min(scopes.text.len_lines()),
                        ) {
                            errors.append(&mut page);
                        }
                    }
                }
                let selected = if forward {
                    errors
                        .iter()
                        .find(|m| m.range.start > caret)
                        .or_else(|| errors.first())
                } else {
                    errors
                        .iter()
                        .rev()
                        .find(|m| m.range.end <= caret)
                        .or_else(|| errors.last())
                };
                self.navigation = Some((token, selected.cloned()));
            }
            Job::Inline(row) => {
                if let Some(hunk) = self.diff.hunks().iter().find(|h| h.new.start == row)
                    && let Ok(changes) = self.diff.inline_diff(hunk, InlineMode::Character)
                {
                    self.inline_spans.insert(row, changes.into());
                }
            }
            Job::Suggest(word) => {
                if let Some(c) = &self.checker {
                    self.suggestions = Some((word.clone(), c.suggestions(&word, 8)));
                }
            }
            _ => {}
        }
    }
    fn publication(&self) -> Publication {
        Publication {
            navigation: self.navigation.clone(),
            identity: self.identity,
            epoch: self.config.epoch,
            inline_spans: self.inline_spans.clone(),
            reference: self.diff.reference().clone(),
            hunks: if self.enabled {
                self.diff.hunks().into()
            } else {
                Arc::from([])
            },
            spelling: {
                let mut errors: Vec<_> = self.spelling.values().flatten().cloned().collect();
                errors.sort_by_key(|m| m.range.start);
                errors.dedup_by(|a, b| a.range == b.range);
                errors.into()
            },
            checked: self.checked.clone(),
            suggestions: self.suggestions.clone(),
            error: self.error.clone(),
        }
    }
}

/// Decodes Git blobs using the same BOM/fallback policy and LF model as open files.
fn reference_text(bytes: &[u8], fallback: &str) -> Result<Text, String> {
    let (name, skip, bom) = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        ("UTF-8", 3, true)
    } else if bytes.starts_with(&[0xff, 0xfe]) {
        ("UTF-16LE", 2, true)
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        ("UTF-16BE", 2, true)
    } else {
        ("UTF-8", 0, false)
    };
    if !name.starts_with("UTF-16") && bytes[..bytes.len().min(8192)].contains(&0) {
        return Err("Git diff reference is binary".into());
    }
    let codec = vel_fileio::encoding(name)?;
    let (mut decoded, mut errors) = codec.decode_without_bom_handling(&bytes[skip..]);
    if errors && !bom {
        (decoded, errors) = vel_fileio::encoding(fallback)?.decode_without_bom_handling(bytes);
    }
    if errors {
        return Err("Git diff reference has invalid text encoding".into());
    }
    let normalized = decoded.replace("\r\n", "\n").replace('\r', "\n");
    Ok(Text::from(normalized.as_str()))
}
