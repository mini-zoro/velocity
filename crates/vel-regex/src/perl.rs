//! Bounded Perl matching for editor Find, separate from the syntax-engine dialect.
use crate::CompileError;
use onig::{MatchParam, RegexOptions, SearchOptions, Syntax, SyntaxOperator};
use std::{
    collections::HashMap,
    ffi::{c_int, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::LazyLock,
};

static PERL: LazyLock<Syntax> = LazyLock::new(|| {
    let mut syntax = *Syntax::perl_ng();
    // Keep the named-group spelling also accepted by regex-cursor.
    // onig 6.5.3's named constant puts this op2 bit in the wrong half.
    syntax.enable_operators(SyntaxOperator::from_bits_retain(
        u64::from(onig_sys::ONIG_SYN_OP2_QMARK_CAPITAL_P_NAME) << 32,
    ));
    syntax
});

/// Compiled Oniguruma Perl regex with editor line and word boundary semantics.
#[derive(Debug)]
pub struct PerlRegex {
    regex: onig::Regex,
    names: HashMap<String, Vec<usize>>,
}

impl PerlRegex {
    /// Compiles a UTF-8 pattern. Anchors are line-relative unless (?-m) is set.
    pub fn new(source: &str, case_sensitive: bool, whole_word: bool) -> Result<Self, CompileError> {
        let mut options =
            RegexOptions::REGEX_OPTION_CAPTURE_GROUP | RegexOptions::REGEX_OPTION_NEGATE_SINGLELINE;
        if !case_sensitive {
            options |= RegexOptions::REGEX_OPTION_IGNORECASE;
        }
        let source = normalize(source)?;
        // With the cap at the VM's right edge, a lookahead would not see
        // past the selection. Check the *end* of the match instead, so the
        // engine can backtrack to a shorter match with full assertion context.
        const START: &str = "(?{vel_scope_start})";
        const END: &str = "(?{vel_scope_end})";
        // Oniguruma's Word class omits Join_Control, which regex-cursor includes.
        const WORD_BOUNDARY: &str = r"(?-i:(?<=[\w\x{200c}\x{200d}])(?![\w\x{200c}\x{200d}])|(?<![\w\x{200c}\x{200d}])(?=[\w\x{200c}\x{200d}]))";
        let pattern = if whole_word {
            format!("{START}{WORD_BOUNDARY}(?:{source}){WORD_BOUNDARY}{END}")
        } else {
            format!("{START}(?:{source}){END}")
        };
        let regex = onig::Regex::with_options(&pattern, options, &PERL)
            .map_err(|err| CompileError(err.to_string()))?;
        let mut names = HashMap::new();
        regex.foreach_name(|name, groups| {
            names.insert(
                name.to_owned(),
                groups.iter().map(|n| *n as usize).collect(),
            );
            true
        });
        Ok(Self { regex, names })
    }

    /// Creates reusable capture storage for a scan.
    pub fn matcher(&self) -> PerlMatcher<'_> {
        PerlMatcher {
            pattern: self,
            captures: onig::Region::with_capacity(self.regex.captures_len() + 1),
        }
    }
}

/// A single scan's reusable capture registers.
#[derive(Debug)]
pub struct PerlMatcher<'a> {
    pattern: &'a PerlRegex,
    captures: onig::Region,
}

impl PerlMatcher<'_> {
    /// Searches from start with full text context. Stops on cancellation or at the match-end bound.
    pub fn search(
        &mut self,
        text: &str,
        start: usize,
        end: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<(usize, usize)>, CompileError> {
        if start > end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return Err(CompileError("Invalid UTF-8 search range".into()));
        }
        let mut params = MatchParam::default();
        params.set_match_stack_limit(4 * 1024 * 1024);
        params.set_retry_limit_in_match(250_000);
        let mut limits = Limits {
            scope_end: end,
            cancelled,
            stopped: false,
        };
        // SAFETY: params is live and unique. The callback is synchronous, and
        // its stack data and the text stay live for the entire search below.
        unsafe {
            onig_sys::onig_set_retry_limit_in_search_of_match_param(
                params.as_raw(),
                64 * 1024 * 1024,
            );
            onig_sys::onig_set_progress_callout_of_match_param(params.as_raw(), Some(check_limits));
            onig_sys::onig_set_callout_user_data_of_match_param(
                params.as_raw(),
                (&raw mut limits).cast(),
            );
        }
        let found = self.pattern.regex.search_with_param(
            text,
            start,
            text.len(),
            SearchOptions::SEARCH_OPTION_NONE,
            Some(&mut self.captures),
            params,
        );
        if limits.stopped {
            return Ok(None);
        }
        found
            .map_err(|err| CompileError(err.to_string()))
            .map(|found| found.and_then(|_| self.captures.pos(0)))
    }

    /// Gets a capture by index or name after a successful search.
    pub fn group(&self, name: &str) -> Option<(usize, usize)> {
        if let Ok(index) = name.parse::<usize>() {
            self.captures.pos(index)
        } else {
            self.pattern
                .names
                .get(name)?
                .iter()
                .rev()
                .find_map(|&index| self.captures.pos(index))
        }
    }
}

struct Limits<'a> {
    scope_end: usize,
    cancelled: &'a dyn Fn() -> bool,
    stopped: bool,
}

unsafe extern "C" fn check_limits(
    args: *mut onig_sys::OnigCalloutArgs,
    data: *mut c_void,
) -> c_int {
    // SAFETY: Oniguruma supplies live callout arguments; data is installed by
    // PerlMatcher::search and points to its live, exclusively used limits.
    let (limits, current, start, content) = unsafe {
        let content = onig_sys::onig_get_contents_by_callout_args(args);
        let end = onig_sys::onig_get_contents_end_by_callout_args(args);
        if content.is_null() || end.is_null() {
            return 0;
        }
        // The VM substitutes a different base pointer on an empty input.
        let base = onig_sys::onig_get_string_by_callout_args(args).addr();
        (
            &mut *data.cast::<Limits<'_>>(),
            onig_sys::onig_get_current_by_callout_args(args).addr() - base,
            onig_sys::onig_get_start_by_callout_args(args).addr() - base,
            std::slice::from_raw_parts(content, end.addr() - content.addr()),
        )
    };
    const OK: c_int = onig_sys::OnigCalloutResult_ONIG_CALLOUT_SUCCESS as c_int;
    const FAIL: c_int = onig_sys::OnigCalloutResult_ONIG_CALLOUT_FAIL as c_int;
    match content {
        b"vel_scope_start" => {
            let cancelled = catch_unwind(AssertUnwindSafe(limits.cancelled)).unwrap_or(true);
            if start > limits.scope_end || cancelled {
                limits.stopped = true;
                onig_sys::ONIG_ABORT
            } else {
                OK
            }
        }
        b"vel_scope_end" if current > limits.scope_end => FAIL,
        _ => OK,
    }
}

// Oniguruma's native multiline ^ excludes the empty line after a final LF.
// Lower just that token, leaving classes, quoted literals and (?-m) alone.
fn normalize(source: &str) -> Result<String, CompileError> {
    let bytes = source.as_bytes();
    let mut result = String::with_capacity(source.len());
    let mut flags = (true, false); // m, x
    let mut groups = Vec::new();
    let mut i = 0;
    let mut tail = "";
    while i < bytes.len() {
        let start = i;
        i += 1;
        match bytes[start] {
            b'\\' if bytes.get(i) == Some(&b'Q') => {
                if let Some(end) = source[i + 1..].find(r"\E") {
                    i += end + 3;
                } else {
                    i = bytes.len();
                    tail = r"\E";
                }
            }
            b'\\' => {
                if let Some(c) = source[i..].chars().next() {
                    i += c.len_utf8();
                }
            }
            b'[' => {
                if bytes.get(i) == Some(&b'^') {
                    i += 1;
                }
                if bytes.get(i) == Some(&b']') {
                    i += 1;
                }
                while i < bytes.len() {
                    if bytes[i] == b']' {
                        i += 1;
                        break;
                    }
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i..].starts_with(b"[:")
                        && let Some(end) = bytes[i + 2..].windows(2).position(|b| b == b":]")
                    {
                        i += end + 4;
                    } else {
                        i += 1;
                    }
                }
            }
            b'#' if flags.1 => {
                if let Some(end) = source[i..].find('\n') {
                    i += end + 1;
                } else {
                    i = bytes.len();
                    tail = "\n";
                }
            }
            b'(' if bytes[i..].starts_with(b"?#") => {
                i += 2;
                while i < bytes.len() && bytes[i] != b')' {
                    i += if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        2
                    } else {
                        1
                    };
                }
                i = (i + 1).min(bytes.len());
            }
            b'(' if bytes[i..].starts_with(b"?R)") => {
                // Use the native group-zero call; Perl_NG can leave the
                // closing paren of (?R) unconsumed.
                result.push_str(r"\g<0>");
                i += 3;
                continue;
            }
            b'(' => {
                if let Some((end, new_flags, scoped)) = inline_flags(bytes, i, flags) {
                    if scoped {
                        groups.push(flags);
                    }
                    flags = new_flags;
                    i = end;
                } else {
                    groups.push(flags);
                }
            }
            b')' => {
                flags = groups
                    .pop()
                    .ok_or_else(|| CompileError("unmatched close parenthesis".into()))?;
            }
            b'^' if flags.0 => {
                result.push_str(r"(?:\A|(?<=\n))");
                continue;
            }
            b if !b.is_ascii() => {
                i = start + source[start..].chars().next().expect("UTF-8").len_utf8();
            }
            _ => {}
        }
        result.push_str(&source[start..i]);
    }
    if !groups.is_empty() {
        return Err(CompileError("unclosed group".into()));
    }
    result.push_str(tail);
    Ok(result)
}

fn inline_flags(
    bytes: &[u8],
    mut i: usize,
    mut flags: (bool, bool),
) -> Option<(usize, (bool, bool), bool)> {
    if bytes.get(i) != Some(&b'?') {
        return None;
    }
    i += 1;
    let start = i;
    let mut enabled = true;
    loop {
        match *bytes.get(i)? {
            b'-' => enabled = false,
            b'm' => flags.0 = enabled,
            b'x' => flags.1 = enabled,
            b'i' | b's' => {}
            c @ (b':' | b')') if i > start => return Some((i + 1, flags, c == b':')),
            _ => return None,
        }
        i += 1;
    }
}
