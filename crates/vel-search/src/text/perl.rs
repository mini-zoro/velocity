use super::{Options, Replacement, expand_format, preserve};
use regex_syntax::{Error, ast::ErrorKind};
use std::ops::Range;
use vel_regex::{PerlMatcher, PerlRegex};
use vel_text::Text;

// Arbitrary assertions and recursion can depend on either edge of the file.
// Never silently match on a truncated window.
pub(crate) const MAX_INPUT: usize = 16 * 1024 * 1024;
const MAX_PATTERN: usize = 16 * 1024;
const MAX_MATCHES: usize = 256 * 1024;
const MAX_REPLACEMENT_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn check_size(bytes: u64) -> Result<(), String> {
    if bytes > MAX_INPUT as u64 {
        Err("Advanced regular expressions are limited to 16 MiB per buffer or file".into())
    } else {
        Ok(())
    }
}

// Only retry errors at a token known to need Perl. Oniguruma's parser
// accepts some mistakes as literals, so a general fallback would hide them.
pub(super) fn unsupported(error: &Error, pattern: &str) -> bool {
    let Error::Parse(error) = error else {
        return false;
    };
    let offset = error.span().start.offset;
    match error.kind() {
        ErrorKind::UnsupportedLookAround | ErrorKind::UnsupportedBackreference => true,
        ErrorKind::EscapeUnrecognized => {
            let rest = &pattern[offset..];
            [r"\G", r"\k<", r"\k'", r"\g<", r"\g'"]
                .iter()
                .any(|prefix| rest.starts_with(prefix))
        }
        ErrorKind::FlagUnrecognized => {
            let Some(group) = pattern[..offset].rfind("(?") else {
                return false;
            };
            let rest = &pattern[group + 2..];
            [">", "(", "&", "R)", "P=", "P>"]
                .iter()
                .any(|prefix| rest.starts_with(prefix))
                || rest
                    .trim_start_matches(['+', '-'])
                    .starts_with(|c: char| c.is_ascii_digit())
        }
        _ => false,
    }
}

pub(super) fn compile(query: &str, options: Options) -> Result<PerlRegex, String> {
    if query.len() > MAX_PATTERN {
        return Err("Advanced regular expressions are limited to 16 KiB of pattern text".into());
    }
    PerlRegex::new(query, options.case_sensitive, options.whole_word)
        .map_err(|error| error.to_string())
}

pub(crate) fn materialize(
    text: &Text,
    cancelled: &impl Fn() -> bool,
) -> Result<Option<String>, String> {
    check_size(text.len_bytes() as u64)?;
    let mut input = String::with_capacity(text.len_bytes());
    for chunk in text.chunks() {
        if cancelled() {
            return Ok(None);
        }
        input.push_str(chunk);
    }
    Ok(Some(input))
}

struct Scan<'a> {
    matcher: PerlMatcher<'a>,
    text: &'a str,
    at: Option<usize>,
    end: usize,
}

impl<'a> Scan<'a> {
    fn new(regex: &'a PerlRegex, text: &'a str, range: &Range<usize>) -> Self {
        Self {
            matcher: regex.matcher(),
            text,
            at: Some(range.start),
            end: range.end,
        }
    }

    fn next(&mut self, cancelled: &impl Fn() -> bool) -> Result<Option<Range<usize>>, String> {
        let Some(at) = self.at.filter(|_| !cancelled()) else {
            return Ok(None);
        };
        let result = self.matcher.search(self.text, at, self.end, cancelled);
        if cancelled() {
            return Ok(None);
        }
        let Some((start, end)) =
            result.map_err(|e| format!("Advanced regex search stopped: {e}"))?
        else {
            return Ok(None);
        };
        if !(at <= start && start <= end && end <= self.end)
            || !self.text.is_char_boundary(start)
            || !self.text.is_char_boundary(end)
        {
            return Err("Advanced regex returned an invalid match range".into());
        }
        self.at = if start == end {
            // Exactly one scalar, including on Unicode text and at EOF.
            self.text[end..]
                .chars()
                .next()
                .map(|c| end + c.len_utf8())
                .filter(|&next| next <= self.end)
        } else {
            Some(end)
        };
        Ok(Some(start..end))
    }
}

pub(crate) fn find(
    regex: &PerlRegex,
    text: &str,
    ranges: &[Range<usize>],
    cancelled: &impl Fn() -> bool,
) -> Result<Option<Vec<Range<usize>>>, String> {
    let mut hits = Vec::new();
    Ok(visit(regex, text, ranges, cancelled, |hit| hits.push(hit))?.then_some(hits))
}

pub(super) fn visit(
    regex: &PerlRegex,
    text: &str,
    ranges: &[Range<usize>],
    cancelled: &impl Fn() -> bool,
    mut visit: impl FnMut(Range<usize>),
) -> Result<bool, String> {
    check_size(text.len() as u64)?;
    let mut count = 0;
    for range in ranges {
        let mut scan = Scan::new(regex, text, range);
        while let Some(hit) = scan.next(cancelled)? {
            if count == MAX_MATCHES {
                return Err("Advanced regex search exceeded 262,144 matches".into());
            }
            count += 1;
            visit(hit);
        }
        if cancelled() {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn replacements(
    regex: &PerlRegex,
    text: &str,
    template: &str,
    ranges: &[Range<usize>],
    only: Option<&Range<usize>>,
    preserve_case: bool,
    cancelled: &impl Fn() -> bool,
) -> Result<Option<Vec<Replacement>>, String> {
    let mut edits = Vec::new();
    let mut remaining = MAX_REPLACEMENT_BYTES;
    let too_large = || "Advanced regex replacement exceeded 16 MiB of replacement text".to_owned();
    if template.len() > MAX_PATTERN {
        return Err("Advanced regex replacement templates are limited to 16 KiB".into());
    }
    for range in ranges {
        let mut scan = Scan::new(regex, text, range);
        while let Some(hit) = scan.next(cancelled)? {
            if let Some(only) = only
                && &hit != only
            {
                if hit.start > only.start {
                    break;
                }
                continue;
            }
            if edits.len() == MAX_MATCHES {
                return Err("Advanced regex replacement exceeded 262,144 matches".into());
            }
            let formatted = expand_format::<true>(template, remaining, |name| {
                scan.matcher
                    .group(name)
                    .and_then(|(a, b)| text.get(a..b))
                    .unwrap_or_default()
                    .to_owned()
            })
            .ok_or_else(too_large)?;
            let replacement = if preserve_case {
                preserve(&text[hit.clone()], &formatted)
            } else {
                formatted
            };
            remaining = remaining
                .checked_sub(replacement.len())
                .ok_or_else(too_large)?;
            edits.push(Replacement {
                range: hit,
                text: replacement,
            });
            if only.is_some() {
                return Ok(Some(edits));
            }
        }
        if cancelled() {
            return Ok(None);
        }
    }
    edits.sort_by_key(|r| (r.range.start, r.range.end));
    edits.dedup_by(|a, b| a.range == b.range);
    Ok(Some(edits))
}
