use crate::{Error, Result};
use vel_regex::{Regex, Region};

#[derive(Debug, Clone)]
pub(crate) struct Transform {
    regex: Regex,
    format: String,
    global: bool,
}
impl Transform {
    pub(crate) fn new(pattern: &str, format: String, options: &str) -> Result<Self> {
        if options.chars().any(|c| !"gimxs".contains(c)) {
            return Err(Error::Parse("unsupported transformation flag".into()));
        }
        // Oniguruma's m is dotall; Boost's m enables line anchors, already the Onig default.
        let flags =
            options.replace(['g', 'm', 's'], "") + if options.contains('s') { "m" } else { "" };
        let pattern = if flags.is_empty() {
            pattern.into()
        } else {
            format!("(?{flags}){pattern}")
        };
        let mut depth = 0usize;
        let mut escaped = false;
        for c in format.chars() {
            if escaped {
                escaped = false;
                continue;
            }
            if c == '\\' {
                escaped = true;
            } else if c == '(' {
                depth += 1;
                if depth > 128 {
                    return Err(Error::Parse("format nesting exceeds 128".into()));
                }
            } else if c == ')' {
                depth = depth.saturating_sub(1);
            }
        }
        let regex = Regex::new(pattern);
        if let Some(error) = regex.compile_error() {
            return Err(Error::Parse(error.to_string()));
        }
        Ok(Self {
            regex,
            format,
            global: options.contains('g'),
        })
    }
    pub(crate) fn apply(&self, input: &str) -> String {
        let mut output = String::new();
        let mut copied = 0;
        let mut at = 0;
        let mut empty = false;
        let mut captures = Region::new();
        loop {
            let found = self
                .regex
                .search(input, at, input.len(), Some(&mut captures), !empty);
            let pos = found.then(|| captures.pos(0)).flatten();
            if empty && pos.is_none_or(|(a, _)| a != at) {
                let Some(c) = input[at..].chars().next() else {
                    break;
                };
                at += c.len_utf8();
                empty = false;
                continue;
            }
            let Some((start, end)) = pos else {
                break;
            };
            output.push_str(&input[copied..start]);
            output.push_str(&format_replacement(&self.format, input, &captures));
            copied = end;
            if !self.global {
                break;
            }
            at = end;
            empty = start == end;
        }
        output.push_str(&input[copied..]);
        output
    }
}
#[derive(Default)]
struct Case {
    persistent: Option<bool>,
    next: Option<bool>,
}
fn append(out: &mut String, value: &str, case: &mut Case) {
    for c in value.chars() {
        match case.next.take().or(case.persistent) {
            Some(true) => out.extend(c.to_uppercase()),
            Some(false) => out.extend(c.to_lowercase()),
            None => out.push(c),
        }
    }
}
fn capture<'a>(n: usize, input: &'a str, captures: &Region) -> &'a str {
    captures.pos(n).map_or("", |(a, b)| &input[a..b])
}
fn number(chars: &[char], at: &mut usize) -> usize {
    let mut n: usize = 0;
    while let Some(d) = chars.get(*at).and_then(|c| c.to_digit(10)) {
        n = n.saturating_mul(10).saturating_add(d as usize);
        *at += 1;
    }
    n
}
fn format_replacement(format: &str, input: &str, captures: &Region) -> String {
    let chars: Vec<_> = format.chars().collect();
    let mut output = String::new();
    render(
        &chars,
        &mut 0,
        input,
        captures,
        &mut output,
        &mut Case::default(),
        false,
        false,
    );
    output
}
#[allow(clippy::too_many_arguments)]
fn render(
    chars: &[char],
    at: &mut usize,
    input: &str,
    captures: &Region,
    out: &mut String,
    case: &mut Case,
    grouped: bool,
    branch: bool,
) {
    while let Some(&c) = chars.get(*at) {
        if (grouped && c == ')') || (branch && c == ':') {
            break;
        }
        *at += 1;
        match c {
            '\\' => {
                let Some(&next) = chars.get(*at) else {
                    out.push('\\');
                    break;
                };
                *at += 1;
                match next {
                    'U' => case.persistent = Some(true),
                    'L' => case.persistent = Some(false),
                    'E' => case.persistent = None,
                    'u' => case.next = Some(true),
                    'l' => case.next = Some(false),
                    '0'..='9' => {
                        *at -= 1;
                        let n = number(chars, at);
                        append(out, capture(n, input, captures), case);
                    }
                    'n' => append(out, "\n", case),
                    't' => append(out, "\t", case),
                    'r' => append(out, "\r", case),
                    other => append(out, &other.to_string(), case),
                }
            }
            '$' => {
                if chars.get(*at) == Some(&'{') {
                    *at += 1;
                    let n = number(chars, at);
                    if chars.get(*at) == Some(&'}') {
                        *at += 1;
                    }
                    append(out, capture(n, input, captures), case);
                } else if chars.get(*at).is_some_and(char::is_ascii_digit) {
                    let n = number(chars, at);
                    append(out, capture(n, input, captures), case);
                } else {
                    match chars.get(*at) {
                        Some('$') => {
                            *at += 1;
                            append(out, "$", case);
                        }
                        Some('&') => {
                            *at += 1;
                            append(out, capture(0, input, captures), case);
                        }
                        Some('`') => {
                            *at += 1;
                            let start = captures.pos(0).map_or(0, |p| p.0);
                            append(out, &input[..start], case);
                        }
                        Some('\'') => {
                            *at += 1;
                            let end = captures.pos(0).map_or(input.len(), |p| p.1);
                            append(out, &input[end..], case);
                        }
                        Some('+') => {
                            *at += 1;
                            let last = (1..captures.len())
                                .rev()
                                .find(|&n| captures.pos(n).is_some())
                                .unwrap_or(0);
                            append(out, capture(last, input, captures), case);
                        }
                        _ => append(out, "$", case),
                    }
                }
            }
            '(' => {
                if chars.get(*at) == Some(&'?') {
                    *at += 1;
                    let n = number(chars, at);
                    // Accept both documented TextMate (?1:yes:no) and Boost (?1yes:no).
                    if chars.get(*at) == Some(&':') {
                        *at += 1;
                    }
                    let mut yes = String::new();
                    let mut yes_case = Case {
                        persistent: case.persistent,
                        next: case.next,
                    };
                    render(
                        chars,
                        at,
                        input,
                        captures,
                        &mut yes,
                        &mut yes_case,
                        true,
                        true,
                    );
                    let mut no = String::new();
                    let mut no_case = Case {
                        persistent: case.persistent,
                        next: case.next,
                    };
                    if chars.get(*at) == Some(&':') {
                        *at += 1;
                        render(
                            chars,
                            at,
                            input,
                            captures,
                            &mut no,
                            &mut no_case,
                            true,
                            false,
                        );
                    }
                    if captures.pos(n).is_some() {
                        out.push_str(&yes);
                        *case = yes_case;
                    } else {
                        out.push_str(&no);
                        *case = no_case;
                    }
                } else {
                    render(chars, at, input, captures, out, case, true, false);
                }
                if chars.get(*at) == Some(&')') {
                    *at += 1;
                }
            }
            other => append(out, &other.to_string(), case),
        }
    }
}
