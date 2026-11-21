//! Necessary conditions for exact-start probes of the bundled JS lookaheads.
//!
//! Recognize the expanded pattern, not the syntax/context name. A different
//! identifier definition or assertion structure stays on the general path.
use crate::frontend;

const START: &str = r"(?:[_$\p{L}\p{Nl}]|(?:\\u(?:\h{4}|\{\h+\})))";
const PART: &str =
    r"(?:[_$\p{L}\p{Nl}\p{Mn}\p{Mc}\p{Nd}\p{Pc}\x{200C}\x{200D}]|(?:\\u(?:\h{4}|\{\h+\})))";
const TRIVIA: &str = r"(?:(?:\s|(?:/\*(?:(?:[^*]|\*(?!/))*)\*/))*)";
const CALL: &str = r"(?=\s*(?:\?\.\s*)?\()";

#[derive(Debug)]
pub(crate) enum IdentifierGuard {
    Assignment,
    Call,
    Reserved(Vec<Box<[u8]>>),
}

fn after_identifier(source: &str) -> Option<&str> {
    source
        .strip_prefix("(?:")?
        .strip_prefix(START)?
        .strip_prefix(PART)?
        .strip_prefix("*(?!")?
        .strip_prefix(PART)?
        .strip_prefix("))")
}

impl IdentifierGuard {
    pub(crate) fn extract(source: &str) -> Option<Self> {
        if let Some(rest) = source.strip_prefix("(?=").and_then(|s| s.strip_suffix(')')) {
            if let Some(tail) = after_identifier(rest) {
                return frontend::guard_starts_with_assignment(tail).then_some(Self::Assignment);
            }
            let prefix = rest
                .strip_suffix(')')?
                .strip_suffix(PART)?
                .strip_suffix("(?!")?;
            if !prefix.starts_with("(?x:") {
                return None;
            }
            let normalized = frontend::normalize(prefix).ok()?;
            let words = normalized.strip_prefix("(?:")?.strip_suffix(')')?;
            let mut literals = Vec::new();
            for word in words.split('|') {
                if word.is_empty() || !word.bytes().all(|b| b.is_ascii_lowercase()) {
                    return None;
                }
                literals.push(Box::from(word.as_bytes()));
            }
            literals.sort_unstable();
            return Some(Self::Reserved(literals));
        }
        let tail = after_identifier(source.strip_prefix('(')?)?.strip_prefix(')')?;
        (tail.strip_prefix(TRIVIA)? == CALL).then_some(Self::Call)
    }

    pub(crate) fn maybe(&self, bytes: &[u8]) -> bool {
        let Some(&first) = bytes.first() else {
            return false;
        };
        let special = |b: u8| !b.is_ascii() || b == b'\\';
        if special(first) {
            return true;
        }
        if !first.is_ascii_alphabetic() && !matches!(first, b'_' | b'$') {
            return false;
        }
        let mut end = 1;
        while let Some(&b) = bytes.get(end) {
            if special(b) {
                // Unicode parts, identifier escapes and Unicode whitespace
                // stay with Oniguruma.
                return true;
            }
            if !b.is_ascii_alphanumeric() && !matches!(b, b'_' | b'$') {
                break;
            }
            end += 1;
        }
        if let Self::Reserved(words) = self {
            let word = &bytes[..end];
            return words.binary_search_by(|w| w.as_ref().cmp(word)).is_ok();
        }
        let mut rest = &bytes[end..];
        while matches!(rest.first(), Some(b' ' | b'\t'..=b'\r')) {
            rest = &rest[1..];
        }
        match rest.first().copied() {
            Some(b) if !b.is_ascii() => true,
            Some(b'=') => matches!(self, Self::Assignment),
            Some(b'(') => matches!(self, Self::Call),
            Some(b'?') => matches!(self, Self::Call) && rest.starts_with(b"?."),
            Some(b'/') => matches!(self, Self::Call) && rest.starts_with(b"/*"),
            _ => false,
        }
    }
}

#[cfg(test)]
#[test]
fn exact_start_probes_keep_native_results_for_identifier_guards() {
    use crate::{OnigBackend, Regex, RegexBackend};

    let ident = format!("(?:{START}{PART}*(?!{PART}))");
    let assignment = format!(r"(?={ident}(?x:(?=\s* = \s* (?:function|async))))");
    let call = format!("({ident}){TRIVIA}{CALL}");
    let reserved = format!("(?=(?x: function | in | instanceof)(?!{PART}))");
    for (source, must_reject) in [
        (assignment.as_str(), "name ("),
        (call.as_str(), "name = function ("),
        (reserved.as_str(), "functionName"),
    ] {
        let guard = IdentifierGuard::extract(source).unwrap();
        assert!(!guard.maybe(must_reject.as_bytes()));
        let native = OnigBackend::default().compile(source, true).unwrap();
        let regex = Regex::new(source.into());
        for line in [
            "",
            "name = function(",
            "name == function",
            "name /*a*/ (",
            "name /**/.bad",
            "name ?.\t(",
            "name \u{a0}(",
            r"\u006eame = function",
            r"name\u{fe}(",
            "über (",
            "name \u{b}= async",
            "name\u{200d} = function",
            "123 (",
            "function",
            "functionName",
            "function!",
            "in",
            "inside",
            "instanceof obj",
        ] {
            for start in line.char_indices().map(|(at, _)| at).chain([line.len()]) {
                assert_eq!(
                    regex.find_impl::<true>(line, start, line.len(), None, true),
                    native.match_at(line, start).unwrap(),
                    "{source} {line:?}:{start}"
                );
            }
        }
    }
    assert!(IdentifierGuard::extract(&format!(r"(?={ident}(?x:(?=\s* = \s* f| a)))")).is_none());
    assert!(IdentifierGuard::extract(&format!(r"(?={ident}(?x:(?=\s* =? \s* f)))")).is_none());
    assert!(IdentifierGuard::extract(&format!(r"(?={ident}(?xi:(?=\s* = \s* f)))")).is_none());
}
