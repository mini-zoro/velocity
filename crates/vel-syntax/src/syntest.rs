//! Official syntax-test runner. Assertion spans count as one case; every column must pass.
//! The format follows Sublime's public package tests and syntect's MIT syntest harness.
use crate::{Scope, ScopeStack, ScopeStackOp, SyntaxSet, parser::InterpreterState};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::LazyLock};
use vel_scope::Selector;
static HEADER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"^\s*(?P<start>\S+?)\s*SYNTAX\s+TEST\s+(?P<modifier>(?:[^\s"]+\s+)*)"(?P<path>[^"]+)"(?:\s+(?P<end>\S.*?))?\s*$"#).unwrap()
});
static MARK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*(?P<mark><-|\^+|@+|>)(?P<selector>.*)$").unwrap());
/// Assertion counts. Skips never contribute to the pass-rate denominator.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Counts {
    /// Passing assertion spans.
    pub passed: usize,
    /// Failing assertion spans, including assertions left unexecuted by errors.
    pub failed: usize,
    /// Explicitly unsupported assertions; the current pinned corpus has no skips.
    pub skipped: usize,
    /// Passing individual assertion columns.
    pub passed_cells: usize,
    /// Failing individual assertion columns.
    pub failed_cells: usize,
}
impl Counts {
    /// Pass fraction of all supported planned assertions.
    pub fn rate(&self) -> f64 {
        let n = self.passed + self.failed;
        if n == 0 {
            1.0
        } else {
            self.passed as f64 / n as f64
        }
    }
    fn add(&mut self, c: &Self) {
        self.passed += c.passed;
        self.failed += c.failed;
        self.skipped += c.skipped;
        self.passed_cells += c.passed_cells;
        self.failed_cells += c.failed_cells;
    }
}
/// One failed assertion, including concrete evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    /// Assertion's one-based physical line.
    pub line: usize,
    /// Source line under test.
    pub source_line: usize,
    /// Starting character column.
    pub column: usize,
    /// Expected selector text.
    pub selector: String,
    /// Observed stack at the first failing column.
    pub actual: String,
}
/// Results for one test file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileResult {
    /// Scope-only counts, kept separate from newly enabled metadata tests.
    #[serde(default)]
    pub scope_counts: Counts,
    /// Symbol and exhaustive symbol-coverage counts.
    #[serde(default)]
    pub symbol_counts: Counts,
    /// Whole-document reindent variants (one case per variant).
    #[serde(default)]
    pub reindent_counts: Counts,
    /// Corpus-relative test path.
    pub file: String,
    /// Declared package syntax path.
    pub syntax: String,
    /// Planned assertion counts.
    pub counts: Counts,
    /// Sample failures, capped at 20 per file.
    pub examples: Vec<Failure>,
    /// Parse/header/filesystem error, when present.
    pub error: Option<String>,
    /// Unsupported modifier, when present.
    pub skip_reason: Option<String>,
}
/// Complete corpus results, including all loaded syntaxes even if they have no tests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteResult {
    /// Scope-only counts.
    #[serde(default)]
    pub scope_counts: Counts,
    /// Symbol assertions, including exhaustiveness checks.
    #[serde(default)]
    pub symbol_counts: Counts,
    /// Whole-file indentation checks.
    #[serde(default)]
    pub reindent_counts: Counts,
    /// Process-wide regex compilation and resource-limit counters at completion.
    #[serde(default)]
    pub regex_metrics: vel_regex::Metrics,
    /// Number of syntax definitions successfully loaded.
    pub loaded: usize,
    /// Number of files discovered (including skipped files).
    pub test_files: usize,
    /// Aggregate assertion counts.
    pub overall: Counts,
    /// Per-syntax assertion counts keyed by syntax name.
    pub syntaxes: BTreeMap<String, Counts>,
    /// Per-file evidence.
    pub files: Vec<FileResult>,
    /// Loading and linking warnings.
    pub warnings: Vec<String>,
}
struct Annotation<'a> {
    begin: usize,
    end: usize,
    selector: &'a str,
    reference: bool,
}
fn annotation<'a>(line: &'a str, start: &str, end: Option<&str>) -> Option<Annotation<'a>> {
    let line = line.trim_end_matches(['\n', '\r']);
    let at = line.find(start)?;
    if !line[..at].trim().is_empty() {
        return None;
    }
    let tail = &line[at + start.len()..];
    let c = MARK.captures(tail)?;
    let m = c.name("mark")?;
    let reference = m.as_str().starts_with('@') || m.as_str() == ">";
    if reference && !tail[m.end()..].is_empty() && !tail[m.end()..].starts_with(char::is_whitespace)
    {
        return None;
    }
    let selector = c.name("selector")?.as_str();
    let selector = end
        .and_then(|e| selector.split_once(e).map(|x| x.0))
        .unwrap_or(selector)
        .trim();
    if reference && selector.is_empty() {
        return None;
    }
    let (begin, end) = if m.as_str() == "<-" || m.as_str() == ">" {
        let n = line[..at].chars().count();
        (n, n + 1)
    } else {
        let n = line[..at + start.len() + m.start()].chars().count();
        (n, n + m.as_str().chars().count())
    };
    Some(Annotation {
        begin,
        end,
        selector,
        reference,
    })
}
#[derive(Debug)]
struct Run {
    begin: usize,
    end: usize,
    scopes: Vec<Scope>,
}
fn runs(
    line: &str,
    ops: &[(usize, ScopeStackOp)],
    stack: &mut ScopeStack,
) -> Result<Vec<Run>, String> {
    let mut out = Vec::new();
    let mut byte = 0;
    let mut col = 0;
    for &(at, op) in ops {
        if at < byte || at > line.len() || !line.is_char_boundary(at) {
            return Err(format!(
                "invalid operation offset {at}, previous {byte}, line length {}",
                line.len()
            ));
        }
        if at > byte {
            let len = line[byte..at].chars().count();
            out.push(Run {
                begin: col,
                end: col + len,
                scopes: stack.as_slice().to_vec(),
            });
            col += len;
            byte = at;
        }
        stack.apply(&op).map_err(|e| e.to_string())?;
    }
    if byte < line.len() {
        let len = line[byte..].chars().count();
        out.push(Run {
            begin: col,
            end: col + len,
            scopes: stack.as_slice().to_vec(),
        });
    }
    Ok(out)
}
fn scopes_at<'a>(
    rows: &'a [Vec<Run>],
    lines: &[&str],
    mut row: usize,
    mut col: usize,
) -> &'a [Scope] {
    while row < lines.len() {
        let n = lines[row].chars().count();
        if col < n {
            return rows[row]
                .iter()
                .find(|r| r.begin <= col && col < r.end)
                .map_or(&[], |r| r.scopes.as_slice());
        }
        col = col.saturating_sub(n);
        row += 1;
    }
    &[]
}
/// Evaluates a syntax-test string; useful for independent harness fixtures.
pub fn evaluate(set: &SyntaxSet, text: &str, file: &str) -> FileResult {
    evaluate_with_metadata(set, &crate::MetadataSet::new(), text, file)
}
/// Evaluates scope, symbol, and reindent assertions using package preferences.
pub fn evaluate_with_metadata(
    set: &SyntaxSet,
    metadata: &crate::MetadataSet,
    text: &str,
    file: &str,
) -> FileResult {
    let mut result = FileResult {
        scope_counts: Counts::default(),
        symbol_counts: Counts::default(),
        reindent_counts: Counts::default(),
        file: file.into(),
        syntax: String::new(),
        counts: Counts::default(),
        examples: Vec::new(),
        error: None,
        skip_reason: None,
    };
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    let Some(header) = lines.first().and_then(|l| HEADER.captures(l.trim_end())) else {
        result.error = Some("unrecognized syntax test header".into());
        return result;
    };
    result.syntax = header["path"].into();
    let start = header.name("start").unwrap().as_str();
    let end = header.name("end").map(|m| m.as_str());
    let annotations: Vec<_> = lines.iter().map(|l| annotation(l, start, end)).collect();
    let Some(syntax) = set.find_syntax_by_path(&result.syntax) else {
        result.error = Some("declared syntax is not loaded".into());
        result.counts.failed = annotations
            .iter()
            .flatten()
            .filter(|a| !a.reference)
            .count();
        return result;
    };
    let mut state = InterpreterState::new(syntax);
    let mut ops = vec![Vec::new(); lines.len()];
    let mut stop = lines.len();
    for (row, line) in lines.iter().enumerate() {
        let parsed =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state.parse_line(line, set)));
        match parsed {
            Ok(Ok(output)) => {
                ops[row] = output.ops;
                for (n, corrected) in output.replayed {
                    if n > 0 && n <= ops.len() {
                        ops[n - 1] = corrected;
                    }
                }
            }
            Ok(Err(e)) => {
                result.error = Some(format!("line {}: {e}", row + 1));
                stop = row;
                break;
            }
            Err(_) => {
                result.error = Some(format!("panic at line {}", row + 1));
                stop = row;
                break;
            }
        }
    }
    let mut rows = Vec::with_capacity(lines.len());
    let mut stack = ScopeStack::new();
    for (row, line) in lines.iter().enumerate() {
        match runs(line, &ops[row], &mut stack) {
            Ok(r) => rows.push(r),
            Err(e) => {
                result.error = Some(format!("scope stack line {}: {e}", row + 1));
                rows.push(Vec::new());
            }
        }
    }
    let spans = byte_spans(&rows, &lines);
    let has_symbols = annotations.iter().flatten().any(|a| a.reference);
    let symbols = if has_symbols {
        metadata.symbols(text, &spans)
    } else {
        Ok(Vec::new())
    };
    let mut checked_symbols = std::collections::HashSet::new();
    let mut source = 0;
    for (row, a) in annotations.iter().enumerate() {
        let Some(a) = a else {
            source = row;
            continue;
        };
        if a.reference {
            let start_byte = source_byte(&lines, source, a.begin);
            let end_byte = source_byte(&lines, source, a.end);
            let (kind, name) =
                a.selector
                    .split_once(char::is_whitespace)
                    .map_or((a.selector, None), |(k, n)| {
                        (
                            k,
                            Some(
                                serde_json::from_str::<String>(n.trim())
                                    .unwrap_or_else(|_| n.trim().trim_matches('"').to_string()),
                            ),
                        )
                    });
            let found = symbols.as_ref().ok().and_then(|symbols| {
                symbols
                    .iter()
                    .enumerate()
                    .find(|(_, s)| s.range.start <= start_byte && s.range.end >= end_byte)
            });
            let pass = if kind == "none" {
                symbols.as_ref().is_ok_and(|ss| {
                    !ss.iter()
                        .any(|s| s.range.start < end_byte && start_byte < s.range.end)
                })
            } else {
                found.is_some_and(|(_, s)| {
                    let matches = match kind {
                        "definition" => s.local || s.indexed,
                        "global-definition" => s.indexed,
                        "local-definition" => s.local,
                        "reference" => s.reference,
                        _ => false,
                    };
                    matches
                        && name.as_ref().is_none_or(|n| {
                            if kind == "reference" || kind == "global-definition" {
                                s.index_name == *n
                            } else {
                                s.name == *n
                            }
                        })
                })
            };
            if let Some((i, _)) = found {
                checked_symbols.insert(i);
            }
            if pass {
                result.symbol_counts.passed += 1;
            } else {
                result.symbol_counts.failed += 1;
                if result.examples.len() < 20 {
                    result.examples.push(Failure {
                        line: row + 1,
                        source_line: source + 1,
                        column: a.begin,
                        selector: a.selector.into(),
                        actual: match &symbols {
                            Ok(_) => format!("{:?}", found.map(|(_, s)| s))
                                .chars()
                                .take(1000)
                                .collect(),
                            Err(e) => e.to_string(),
                        },
                    });
                }
            }
            continue;
        }
        let selector = parse_assertion_selector(a.selector);
        let mut first = None;
        for col in a.begin..a.end {
            let scopes = scopes_at(&rows, &lines, source, col);
            let pass = row < stop && selector.as_ref().is_ok_and(|s| s.matches(scopes));
            if pass {
                result.counts.passed_cells += 1;
            } else {
                result.counts.failed_cells += 1;
                if first.is_none() {
                    first = Some((col, scopes));
                }
            }
        }
        if let Some((col, scopes)) = first {
            result.counts.failed += 1;
            if result.examples.len() < 20 {
                result.examples.push(Failure {
                    line: row + 1,
                    source_line: source + 1,
                    column: col,
                    selector: a.selector.into(),
                    actual: scopes
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                });
            }
        } else {
            result.counts.passed += 1;
        }
    }
    result.scope_counts = result.counts.clone();
    if has_symbols
        && !header.name("modifier").is_some_and(|m| {
            m.as_str()
                .split_whitespace()
                .any(|s| s == "partial-symbols")
        })
        && let Ok(symbols) = &symbols
    {
        for (i, symbol) in symbols.iter().enumerate() {
            if !checked_symbols.contains(&i) {
                result.symbol_counts.failed += 1;
                if result.examples.len() < 20 {
                    result.examples.push(Failure {
                        line: 1,
                        source_line: text[..symbol.range.start]
                            .bytes()
                            .filter(|b| *b == b'\n')
                            .count()
                            + 1,
                        column: 0,
                        selector: "exhaustive symbol coverage".into(),
                        actual: format!("{symbol:?}"),
                    });
                }
            }
        }
    }
    if let Some(modifier) = header.name("modifier") {
        for modifier in modifier
            .as_str()
            .split_whitespace()
            .filter(|s| s.starts_with("reindent"))
        {
            for unindented in [false, true] {
                if (unindented && modifier == "reindent-unchanged")
                    || (!unindented && modifier == "reindent-unindented")
                {
                    continue;
                }
                let input = if unindented {
                    lines
                        .iter()
                        .map(|l| l.trim_start_matches([' ', '\t']))
                        .collect::<String>()
                } else {
                    text.into()
                };
                let input_spans = if unindented {
                    match crate::indent::scoped_spans(set, syntax, &input) {
                        Ok(spans) => spans,
                        Err(error) => {
                            result.error = Some(error);
                            result.reindent_counts.failed += 1;
                            continue;
                        }
                    }
                } else {
                    spans.clone()
                };
                let options = crate::indent::IndentOptions {
                    tab_size: 4,
                    spaces: !text.lines().any(|l| l.starts_with('\t')),
                };
                let actual = crate::indent::reindent(&input, &input_spans, metadata, options);
                if actual == text {
                    result.reindent_counts.passed += 1;
                } else {
                    result.reindent_counts.failed += 1;
                    if result.examples.len() < 20 {
                        let row = actual
                            .lines()
                            .zip(text.lines())
                            .position(|(a, b)| a != b)
                            .unwrap_or(0);
                        result.examples.push(Failure {
                            line: row + 1,
                            source_line: row + 1,
                            column: 0,
                            selector: if unindented {
                                "reindent-unindented"
                            } else {
                                "reindent-unchanged"
                            }
                            .into(),
                            actual: format!(
                                "expected {:?}, got {:?}",
                                text.lines().nth(row).unwrap_or(""),
                                actual.lines().nth(row).unwrap_or("")
                            ),
                        });
                    }
                }
            }
        }
    }
    result.counts.add(&result.symbol_counts);
    result.counts.add(&result.reindent_counts);
    result
}
fn source_byte(lines: &[&str], row: usize, col: usize) -> usize {
    lines[..row].iter().map(|l| l.len()).sum::<usize>()
        + lines[row]
            .char_indices()
            .nth(col)
            .map_or(lines[row].len(), |(i, _)| i)
}
fn byte_spans(rows: &[Vec<Run>], lines: &[&str]) -> Vec<crate::ScopedSpan> {
    let mut offset = 0;
    let mut spans = Vec::new();
    for (runs, line) in rows.iter().zip(lines) {
        let positions: Vec<_> = line
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(line.len()))
            .collect();
        spans.extend(runs.iter().map(|r| crate::ScopedSpan {
            range: offset + positions[r.begin]..offset + positions[r.end],
            scopes: r.scopes.clone(),
        }));
        offset += line.len();
    }
    spans
}
/// Runs all `syntax_test_*` files under a pinned package directory, optionally filtering paths.
pub fn run(set: &SyntaxSet, root: &Path, filter: Option<&str>) -> SuiteResult {
    let metadata = crate::MetadataSet::load_directories(&[root]);
    let mut suite = SuiteResult {
        scope_counts: Counts::default(),
        symbol_counts: Counts::default(),
        reindent_counts: Counts::default(),
        regex_metrics: vel_regex::Metrics::default(),
        loaded: set.syntaxes().len(),
        test_files: 0,
        overall: Counts::default(),
        syntaxes: set
            .syntaxes()
            .iter()
            .map(|s| (s.name.clone(), Counts::default()))
            .collect(),
        files: Vec::new(),
        warnings: set.warnings().to_vec(),
    };
    let mut paths: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().is_file() && e.file_name().to_string_lossy().starts_with("syntax_test_")
        })
        .map(|e| e.into_path())
        .filter(|p| {
            filter.is_none_or(|f| {
                p.to_string_lossy()
                    .to_lowercase()
                    .contains(&f.to_lowercase())
            })
        })
        .collect();
    paths.sort();
    for path in paths {
        let file = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
        let result = match std::fs::read_to_string(&path) {
            Ok(text) => match &metadata {
                Ok(metadata) => {
                    evaluate_with_metadata(set, metadata, &text.replace('\r', ""), &file)
                }
                Err(e) => {
                    let mut r = evaluate(set, &text.replace('\r', ""), &file);
                    r.error = Some(e.to_string());
                    r
                }
            },
            Err(e) => FileResult {
                scope_counts: Counts::default(),
                symbol_counts: Counts::default(),
                reindent_counts: Counts::default(),
                file: file.into(),
                syntax: String::new(),
                counts: Counts::default(),
                examples: Vec::new(),
                error: Some(e.to_string()),
                skip_reason: None,
            },
        };
        let name = set
            .find_syntax_by_path(&result.syntax)
            .map_or(result.syntax.as_str(), |s| s.name.as_str());
        suite
            .syntaxes
            .entry(name.into())
            .or_default()
            .add(&result.counts);
        suite.overall.add(&result.counts);
        suite.scope_counts.add(&result.scope_counts);
        suite.symbol_counts.add(&result.symbol_counts);
        suite.reindent_counts.add(&result.reindent_counts);
        if std::env::var_os("VEL_SYNTAX_PROGRESS").is_some() {
            eprintln!(
                "{}: {}/{} ({})",
                result.file,
                result.counts.passed,
                result.counts.passed + result.counts.failed,
                result.error.as_deref().unwrap_or("ok")
            );
        }
        suite.files.push(result);
    }
    suite.test_files = suite.files.len();
    suite.regex_metrics = vel_regex::metrics();
    suite
}

/// Parses an assertion selector. If the whole text is not a valid selector, the longest
/// parseable prefix is used, i.e. trailing text the selector grammar cannot consume is
/// ignored (for example `-comment() {}` in the JavaScript tests evaluates `-comment`).
/// ST-BEHAVIOR: unverified — this mirrors a lenient parser; it is a general rule, not a
/// per-test exception.
fn parse_assertion_selector(text: &str) -> Result<Selector, vel_scope::ParseScopeError> {
    let text = text.trim();
    let full = Selector::parse(text);
    if full.is_ok() {
        return full;
    }
    (1..text.len())
        .rev()
        .filter(|&end| text.is_char_boundary(end))
        .map(|end| text[..end].trim_end())
        .filter(|prefix| !prefix.is_empty())
        .find_map(|prefix| Selector::parse(prefix).ok())
        .ok_or_else(|| full.unwrap_err())
}
