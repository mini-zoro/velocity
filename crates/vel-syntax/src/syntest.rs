//! Official syntax-test runner. Assertion spans count as one case; every column must pass.
//! The format follows Sublime's public package tests and syntect's MIT syntest harness.
use crate::{Scope, ScopeStack, ScopeStackOp, SyntaxSet, parser::InterpreterState};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::LazyLock};
use vel_scope::Selector;
static HEADER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"^\s*(?P<start>\S+?)\s*SYNTAX\s+TEST\s+(?:(?P<modifier>[^\s"]+)\s+)?"(?P<path>[^"]+)"(?:\s+(?P<end>\S.*?))?\s*$"#).unwrap()
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
    /// Explicitly unsupported symbol/reference assertions or modified-file cases.
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
    let mut result = FileResult {
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
    // Only reindent tests are skipped wholesale (no indentation engine yet). `partial-symbols`
    // merely relaxes symbol assertions; the file's scope assertions must still be checked.
    if let Some(m) = header
        .name("modifier")
        .filter(|m| m.as_str().starts_with("reindent"))
    {
        result.skip_reason = Some(m.as_str().into());
        result.counts.skipped = annotations.iter().flatten().count().max(1);
        return result;
    }
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
    let mut source = 0;
    for (row, a) in annotations.iter().enumerate() {
        let Some(a) = a else {
            source = row;
            continue;
        };
        if a.reference {
            result.counts.skipped += 1;
            continue;
        }
        let selector = Selector::parse(a.selector);
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
    result
}
/// Runs all `syntax_test_*` files under a pinned package directory, optionally filtering paths.
pub fn run(set: &SyntaxSet, root: &Path, filter: Option<&str>) -> SuiteResult {
    let mut suite = SuiteResult {
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
            Ok(text) => evaluate(set, &text.replace('\r', ""), &file),
            Err(e) => FileResult {
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
