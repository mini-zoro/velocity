//! Pure snippet parsing, expansion, ordered fields, mirrors and transformations.
#![doc = include_str!("../README.md")]
//! Edits use old UTF-8 coordinates and can be applied in one vel-buffer transaction.
#![forbid(unsafe_code)]
mod parse;
mod transform;
use parse::{Key, Node};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ops::Range,
    path::Path,
};
use vel_syntax::Metadata;
use vel_text::Text;

/// Malformed XML/snippet or invalid input coordinates.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Resource or inline syntax error.
    #[error("invalid snippet: {0}")]
    Parse(String),
    /// Invalid, overlapping or stale caret/field coordinates.
    #[error("invalid snippet edit: {0}")]
    Edit(&'static str),
}
/// Snippet operation result.
pub type Result<T> = std::result::Result<T, Error>;
/// Parsed inline snippet, safe to reuse for many buffers.
#[derive(Debug, Clone)]
pub struct Snippet {
    nodes: Vec<Node>,
}
/// Parsed .sublime-snippet resource.
#[derive(Debug, Clone)]
pub struct SnippetFile {
    /// Inline content, preserving CDATA whitespace.
    pub content: String,
    /// Optional exact Tab trigger.
    pub tab_trigger: String,
    /// Scope selector (empty means any scope).
    pub scope: String,
    /// Command palette/popup description.
    pub description: String,
    /// Compiled inline snippet.
    pub snippet: Snippet,
}
impl SnippetFile {
    /// Reads XML including CDATA and escaped character data; rejects malformed/foreign roots.
    pub fn parse(xml: &str) -> Result<Self> {
        use quick_xml::{Reader, events::Event};
        let mut reader = Reader::from_str(xml);
        let mut stack: Vec<String> = Vec::new();
        let mut values: HashMap<String, String> = HashMap::new();
        let mut root = false;
        loop {
            match reader
                .read_event()
                .map_err(|e| Error::Parse(e.to_string()))?
            {
                Event::Start(tag) => {
                    let name = tag.name().as_ref().to_owned();
                    if stack.is_empty() {
                        if root || name != "snippet" {
                            return Err(Error::Parse("expected one snippet root".into()));
                        }
                        root = true;
                    }
                    stack.push(name);
                }
                Event::Empty(tag) => {
                    if stack.len() == 1 {
                        values.entry(tag.name().as_ref().to_owned()).or_default();
                    } else if stack.is_empty() {
                        return Err(Error::Parse("missing snippet content".into()));
                    }
                }
                Event::End(_) => {
                    stack.pop();
                }
                Event::CData(data) => {
                    if stack.len() == 2 {
                        values
                            .entry(stack[1].clone())
                            .or_default()
                            .push_str(data.as_ref());
                    }
                }
                Event::Text(data) => {
                    if stack.len() == 2 {
                        values
                            .entry(stack[1].clone())
                            .or_default()
                            .push_str(data.as_ref());
                    }
                }
                Event::GeneralRef(reference) => {
                    if stack.len() == 2 {
                        let name = reference.as_ref();
                        let escaped = format!("&{name};");
                        let decoded = quick_xml::escape::unescape(&escaped)
                            .map_err(|e| Error::Parse(e.to_string()))?;
                        values
                            .entry(stack[1].clone())
                            .or_default()
                            .push_str(&decoded);
                    }
                }
                Event::Eof => break,
                _ => (),
            }
        }
        if !root || !stack.is_empty() {
            return Err(Error::Parse("incomplete snippet XML".into()));
        }
        let content = values
            .remove("content")
            .ok_or_else(|| Error::Parse("missing content".into()))?;
        let snippet = Snippet::parse(&content)?;
        Ok(Self {
            content,
            snippet,
            tab_trigger: values
                .remove("tabTrigger")
                .unwrap_or_default()
                .trim()
                .into(),
            scope: values.remove("scope").unwrap_or_default().trim().into(),
            description: values
                .remove("description")
                .unwrap_or_default()
                .trim()
                .into(),
        })
    }
}
/// Variables supplied by the caller; per-caret dynamic variables are filled during expansion.
#[derive(Debug, Clone, Default)]
pub struct Environment(pub BTreeMap<String, String>);
impl Environment {
    /// Adds filename/path variables without performing file I/O.
    pub fn with_path(mut self, path: &Path) -> Self {
        self.0
            .insert("TM_FILEPATH".into(), path.to_string_lossy().into());
        self.0.insert(
            "TM_FILENAME".into(),
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
        );
        self.0.insert(
            "TM_DIRECTORY".into(),
            path.parent()
                .unwrap_or(Path::new(""))
                .to_string_lossy()
                .into(),
        );
        self
    }
    /// Merges scope-selected shell variables, including TM_COMMENT_*.
    pub fn with_metadata(mut self, metadata: &Metadata) -> Self {
        self.0.extend(metadata.shell_variables.clone());
        self
    }
}
/// Insertion indentation policy, independent of UI settings storage.
#[derive(Debug, Clone, Copy)]
pub struct ExpansionOptions {
    /// Spaces per visual tab stop (clamped to at least one).
    pub tab_size: usize,
    /// Translate snippet tabs to spaces at visual tab stops.
    pub translate_tabs_to_spaces: bool,
}
impl Default for ExpansionOptions {
    fn default() -> Self {
        Self {
            tab_size: 4,
            translate_tabs_to_spaces: true,
        }
    }
}
/// Owned transaction edit; ranges are in the pre-transaction buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetEdit {
    /// UTF-8 range to replace.
    pub range: Range<usize>,
    /// Replacement content.
    pub text: String,
}
/// A field occurrence in post-transaction document coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRegion {
    /// Ordered field number; zero is the final stop.
    pub number: u32,
    /// Absolute UTF-8 range.
    pub range: Range<usize>,
    /// True for a secondary occurrence updated automatically with its primary field.
    pub mirror: bool,
    /// Whether this occurrence applies a regex transformation.
    pub transformed: bool,
    /// Caret instance in the input's sorted caret order.
    pub instance: usize,
}
/// Expansion transaction and its live field model.
#[derive(Debug)]
pub struct Expansion {
    /// Apply together in a single old-coordinate transaction.
    pub edits: Vec<SnippetEdit>,
    /// Navigation/mirror state after applying those edits.
    pub session: Session,
}
#[derive(Debug)]
struct Instance {
    range: Range<usize>,
    env: Environment,
    indent: String,
    column: usize,
    overrides: HashMap<u32, String>,
}
/// Pure field state. Apply every returned transaction before issuing the next operation.
/// An unrelated edit within a snippet should cancel the session; external edits can be rebased.
#[derive(Debug)]
pub struct Session {
    snippet: Snippet,
    options: ExpansionOptions,
    instances: Vec<Instance>,
    fields: Vec<FieldRegion>,
    order: Vec<u32>,
    active: usize,
}
impl Snippet {
    /// Compiles inline fields, nested defaults, variables and substitutions.
    pub fn parse(source: &str) -> Result<Self> {
        Ok(Self {
            nodes: parse::parse(source)?,
        })
    }
    /// Expands at sorted, nonoverlapping caret/selection ranges in an immutable buffer.
    pub fn expand(
        &self,
        text: &Text,
        carets: &[Range<usize>],
        environment: &Environment,
        options: ExpansionOptions,
    ) -> Result<Expansion> {
        let mut instances = Vec::new();
        let mut edits = Vec::new();
        let mut fields = Vec::new();
        let mut delta = 0isize;
        let mut last_end = 0;
        for (i, caret) in carets.iter().enumerate() {
            if caret.start > caret.end
                || caret.end > text.len_bytes()
                || !text.is_char_boundary(caret.start)
                || !text.is_char_boundary(caret.end)
                || (i > 0 && (caret.start < last_end || caret.start == carets[i - 1].start))
            {
                return Err(Error::Edit(
                    "carets must be valid, sorted and nonoverlapping",
                ));
            }
            last_end = caret.end;
            let row = text.byte_to_line(caret.start);
            let line = text.line(row).to_string();
            let column = caret.start - text.line_to_byte(row);
            let indent: String = line
                .chars()
                .take_while(|c| matches!(c, ' ' | '\t'))
                .collect();
            let mut env = environment.clone();
            let selected = text.slice(caret.clone()).to_string();
            env.0.insert("SELECTION".into(), selected.clone());
            env.0.insert("TM_SELECTED_TEXT".into(), selected);
            env.0.insert("TM_LINE_INDEX".into(), row.to_string());
            env.0.insert("TM_LINE_NUMBER".into(), (row + 1).to_string());
            env.0.insert(
                "TM_CURRENT_LINE".into(),
                line.trim_end_matches(['\n', '\r']).into(),
            );
            let word = |c: char| c.is_alphanumeric() || c == '_';
            let before = line[..column]
                .char_indices()
                .rev()
                .take_while(|(_, c)| word(*c))
                .last()
                .map_or(column, |p| p.0);
            let after = line[column..]
                .char_indices()
                .find(|(_, c)| !word(*c))
                .map_or(line.len(), |p| column + p.0);
            env.0
                .insert("TM_CURRENT_WORD".into(), line[before..after].into());
            env.0
                .insert("TM_TAB_SIZE".into(), options.tab_size.max(1).to_string());
            env.0.insert(
                "TM_SOFT_TABS".into(),
                if options.translate_tabs_to_spaces {
                    "YES"
                } else {
                    "NO"
                }
                .into(),
            );
            let instance = Instance {
                range: caret
                    .start
                    .checked_add_signed(delta)
                    .ok_or(Error::Edit("coordinate overflow"))?..0,
                env,
                indent,
                column: visual_column(&line[..column], options.tab_size),
                overrides: HashMap::new(),
            };
            let (inserted, mut local) = self.render(&instance, options, i);
            let start = instance.range.start;
            let mut instance = instance;
            instance.range = start..start + inserted.len();
            for field in &mut local {
                field.range.start += start;
                field.range.end += start;
            }
            fields.extend(local);
            delta += inserted.len() as isize - caret.len() as isize;
            edits.push(SnippetEdit {
                range: caret.clone(),
                text: inserted,
            });
            instances.push(instance);
        }
        let mut session = Session {
            snippet: self.clone(),
            options,
            instances,
            fields,
            order: Vec::new(),
            active: 0,
        };
        session.reorder(None);
        Ok(Expansion { edits, session })
    }
    fn render(
        &self,
        instance: &Instance,
        options: ExpansionOptions,
        index: usize,
    ) -> (String, Vec<FieldRegion>) {
        let mut definitions = HashMap::new();
        collect(&self.nodes, &mut definitions);
        let mut renderer = Renderer {
            env: &instance.env,
            definitions,
            overrides: &instance.overrides,
            seen: HashSet::new(),
            visiting: HashSet::new(),
            fields: Vec::new(),
            out: String::new(),
            instance: index,
        };
        renderer.nodes(&self.nodes, true);
        if !renderer.seen.contains(&0) {
            renderer.fields.push(FieldRegion {
                number: 0,
                range: renderer.out.len()..renderer.out.len(),
                mirror: false,
                transformed: false,
                instance: index,
            });
        }
        let (out, map) = indent(&renderer.out, &instance.indent, instance.column, options);
        for field in &mut renderer.fields {
            field.range = map[field.range.start]..map[field.range.end];
        }
        (out, renderer.fields)
    }
}
impl Session {
    /// All field occurrences, including mirrors, in current document coordinates.
    pub fn fields(&self) -> &[FieldRegion] {
        &self.fields
    }
    /// Currently selected field number, or None for a session with no caret instances.
    pub fn active_field(&self) -> Option<u32> {
        self.order.get(self.active).copied()
    }
    /// Primary ranges selected for the active field, one per caret where that field exists.
    pub fn selections(&self) -> Vec<Range<usize>> {
        self.fields
            .iter()
            .filter(|field| Some(field.number) == self.active_field() && !field.mirror)
            .map(|field| field.range.clone())
            .collect()
    }
    /// Whether Tab can advance to another field (including final $0).
    pub fn has_next_field(&self) -> bool {
        self.active + 1 < self.order.len()
    }
    /// Selects the next number; $0 is visited last and remains selected at completion.
    pub fn next_field(&mut self) -> Vec<Range<usize>> {
        if self.has_next_field() {
            self.active += 1;
        }
        self.selections()
    }
    /// Selects the preceding number without wrapping.
    pub fn prev_field(&mut self) -> Vec<Range<usize>> {
        self.active = self.active.saturating_sub(1);
        self.selections()
    }
    /// Replaces the active primary value in every instance, updating all mirrors and enclosing
    /// defaults. Nested stops disappear when their parent is explicitly replaced.
    /// Edits replace each snippet instance atomically; UI code should group them with the field edit.
    pub fn replace_field(&mut self, value: &str) -> Result<Vec<SnippetEdit>> {
        self.replace_field_values(&vec![value.to_string(); self.instances.len()])
    }
    /// Replaces the active field independently for each caret instance. Values use unindented
    /// snippet text; every returned edit belongs to one old-coordinate atomic transaction.
    pub fn replace_field_values(&mut self, values: &[String]) -> Result<Vec<SnippetEdit>> {
        let number = self.active_field().ok_or(Error::Edit("no active field"))?;
        if values.len() != self.instances.len() {
            return Err(Error::Edit("one value per caret instance is required"));
        }
        let mut edits = Vec::new();
        let mut fields = Vec::new();
        let mut delta = 0isize;
        for (i, instance) in self.instances.iter_mut().enumerate() {
            instance.overrides.insert(number, values[i].clone());
            let (out, mut local) = self.snippet.render(instance, self.options, i);
            let old = instance.range.clone();
            let start = old
                .start
                .checked_add_signed(delta)
                .ok_or(Error::Edit("coordinate overflow"))?;
            for field in &mut local {
                field.range.start += start;
                field.range.end += start;
            }
            fields.extend(local);
            instance.range = start..start + out.len();
            delta += out.len() as isize - old.len() as isize;
            edits.push(SnippetEdit {
                range: old,
                text: out,
            });
        }
        self.fields = fields;
        self.reorder(Some(number));
        Ok(edits)
    }
    /// Plans typing/deletion within the active primary fields, together with mirror changes.
    /// Pass the pre-edit buffer and sorted edits, then apply only this method's returned edits.
    pub fn edit_fields(
        &mut self,
        before: &Text,
        input: &[SnippetEdit],
    ) -> Result<Vec<SnippetEdit>> {
        let number = self.active_field().ok_or(Error::Edit("no active field"))?;
        let mut values = vec![String::new(); self.instances.len()];
        let primaries: Vec<_> = self
            .fields
            .iter()
            .filter(|f| f.number == number && !f.mirror)
            .collect();
        for field in &primaries {
            if field.range.end > before.len_bytes()
                || !before.is_char_boundary(field.range.start)
                || !before.is_char_boundary(field.range.end)
            {
                return Err(Error::Edit("stale field coordinates"));
            }
            values[field.instance] = before.slice(field.range.clone()).to_string();
        }
        let mut previous_end = 0;
        for (i, edit) in input.iter().enumerate() {
            if edit.range.start > edit.range.end
                || edit.range.end > before.len_bytes()
                || !before.is_char_boundary(edit.range.start)
                || !before.is_char_boundary(edit.range.end)
                || i > 0 && edit.range.start < previous_end
            {
                return Err(Error::Edit("invalid field edit range"));
            }
            previous_end = edit.range.end;
            if !primaries.iter().any(|field| {
                field.range.start <= edit.range.start && edit.range.end <= field.range.end
            }) {
                return Err(Error::Edit("edit is outside active fields"));
            }
        }
        for edit in input.iter().rev() {
            let field = primaries
                .iter()
                .find(|field| {
                    field.range.start <= edit.range.start && edit.range.end <= field.range.end
                })
                .unwrap();
            values[field.instance].replace_range(
                edit.range.start - field.range.start..edit.range.end - field.range.start,
                &edit.text,
            );
        }
        for (value, instance) in values.iter_mut().zip(&self.instances) {
            let prefix = if self.options.translate_tabs_to_spaces {
                " ".repeat(visual_column(&instance.indent, self.options.tab_size))
            } else {
                instance.indent.clone()
            };
            if !prefix.is_empty() {
                *value = value.replace(&format!("\n{prefix}"), "\n");
            }
        }
        self.replace_field_values(&values)
    }
    /// Maps a transaction wholly outside snippet instances. Returns false (caller cancels) for
    /// intersecting edits. Active-field typing must use `replace_field` to update mirrors.
    pub fn rebase(&mut self, changes: &vel_buffer::ChangeSet) -> bool {
        if changes.changes().iter().any(|change| {
            self.instances.iter().any(|instance| {
                change.old_range.start < instance.range.end
                    && change.old_range.end > instance.range.start
                    || change.old_range.is_empty()
                        && instance.range.contains(&change.old_range.start)
            })
        }) {
            return false;
        }
        for field in &mut self.fields {
            let at_end = field.range.is_empty()
                && field.range.end == self.instances[field.instance].range.end;
            let bias = if at_end {
                vel_buffer::Bias::Left
            } else {
                vel_buffer::Bias::Right
            };
            field.range = changes.transform_point(field.range.start, bias)
                ..changes.transform_point(field.range.end, bias);
        }
        for instance in &mut self.instances {
            instance.range = changes.transform_point(instance.range.start, vel_buffer::Bias::Right)
                ..changes.transform_point(instance.range.end, vel_buffer::Bias::Left);
        }
        true
    }
    fn reorder(&mut self, current: Option<u32>) {
        self.order = self
            .fields
            .iter()
            .filter(|f| !f.mirror)
            .map(|f| f.number)
            .collect();
        self.order
            .sort_unstable_by_key(|&n| if n == 0 { u64::MAX } else { n as u64 });
        self.order.dedup();
        self.active = current
            .and_then(|n| self.order.iter().position(|&x| x == n))
            .unwrap_or(0);
    }
}
fn collect<'a>(nodes: &'a [Node], out: &mut HashMap<u32, &'a [Node]>) {
    for node in nodes {
        if let Node::Value(Key::Field(n), Some(default), _) = node {
            out.entry(*n).or_insert(default);
            collect(default, out);
        }
    }
}
struct Renderer<'a> {
    env: &'a Environment,
    definitions: HashMap<u32, &'a [Node]>,
    overrides: &'a HashMap<u32, String>,
    seen: HashSet<u32>,
    visiting: HashSet<u32>,
    fields: Vec<FieldRegion>,
    out: String,
    instance: usize,
}
impl Renderer<'_> {
    fn nodes(&mut self, nodes: &[Node], record: bool) {
        for node in nodes {
            match node {
                Node::Text(text) => self.out.push_str(text),
                Node::Value(key, default, transform) => {
                    let start = self.out.len();
                    let (number, mirror) = match key {
                        Key::Field(n) => (Some(*n), self.seen.contains(n) || transform.is_some()),
                        Key::Variable(_) => (None, false),
                    };
                    if let Some(n) = number
                        && record
                        && !mirror
                    {
                        self.seen.insert(n);
                    }
                    let before = self.out.len();
                    match key {
                        Key::Variable(name) => {
                            if let Some(value) = self.env.0.get(name).filter(|s| !s.is_empty()) {
                                self.out.push_str(value);
                            } else if let Some(default) = default {
                                self.nodes(default, record);
                            }
                        }
                        Key::Field(n) => {
                            if let Some(value) = self.overrides.get(n) {
                                self.out.push_str(value);
                            } else if self.visiting.insert(*n) {
                                if let Some(default) = self.definitions.get(n).copied() {
                                    self.nodes(default, record && !mirror);
                                }
                                self.visiting.remove(n);
                            }
                        }
                    }
                    if let Some(transform) = transform {
                        let value = transform.apply(&self.out[before..]);
                        self.out.truncate(before);
                        self.out.push_str(&value);
                    }
                    if let Some(number) = number
                        && record
                    {
                        self.fields.push(FieldRegion {
                            number,
                            range: start..self.out.len(),
                            mirror,
                            transformed: transform.is_some(),
                            instance: self.instance,
                        });
                    }
                }
            }
        }
    }
}
fn indent(
    input: &str,
    prefix: &str,
    initial_column: usize,
    options: ExpansionOptions,
) -> (String, Vec<usize>) {
    let mut out = String::new();
    let mut map = vec![0; input.len() + 1];
    let size = options.tab_size.max(1);
    let mut column = initial_column;
    for (byte, c) in input.char_indices() {
        map[byte] = out.len();
        if c == '\t' && options.translate_tabs_to_spaces {
            let spaces = size - column % size;
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            out.push(c);
            column += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        }
        if c == '\n' {
            column = 0;
            for c in prefix.chars() {
                if c == '\t' {
                    let width = size - column % size;
                    if options.translate_tabs_to_spaces {
                        out.extend(std::iter::repeat_n(' ', width));
                    } else {
                        out.push(c);
                    }
                    column += width;
                } else {
                    out.push(c);
                    column += 1;
                }
            }
        }
        map[byte + c.len_utf8()] = out.len();
    }
    (out, map)
}

fn visual_column(text: &str, tab_size: usize) -> usize {
    let size = tab_size.max(1);
    text.chars().fold(0, |column, c| {
        column
            + if c == '\t' {
                size - column % size
            } else {
                unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
            }
    })
}
