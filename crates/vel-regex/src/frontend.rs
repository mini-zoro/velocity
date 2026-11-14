//! Conservative Oniguruma candidate front end, adapted from the in-repository
//! sregex prototype. These expressions are supersets on ASCII haystacks only.
//! Exact spans, assertions, full folding and Unicode are resolved by Oniguruma.
use regex_syntax::hir::{Class, ClassBytes, ClassBytesRange, Hir, HirKind, Look};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Dfa,
    Boundary,
    Backtracking,
}

#[derive(Debug, Clone)]
pub struct Guard {
    pub positive: bool,
    pub leading: bool,
    pub expr: String,
}

#[derive(Debug, Clone)]
pub struct Translation {
    pub class: Kind,
    pub core: Option<String>,
    pub guards: Vec<Guard>,
    pub reason: String,
}

// Strip Onig's x-mode ourselves. Rust's x-mode also ignores spaces INSIDE
// classes; Oniguruma's does not. Onig (?m) means dot-all, i.e. Rust (?s).
pub fn normalize(p: &str) -> Result<String, String> {
    let b = p.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    let mut extended = false;
    let mut stack = vec![];
    let mut classes = 0usize;
    while i < b.len() {
        if b[i] == b'\\' {
            if i + 1 == b.len() {
                out.push('\\');
                break;
            }
            let c = b[i + 1];
            if !c.is_ascii() {
                let ch = p[i + 1..].chars().next().unwrap();
                out.push(ch);
                i += 1 + ch.len_utf8();
                continue;
            }
            match c {
                b'h' => out.push_str(if classes > 0 {
                    "0-9A-Fa-f"
                } else {
                    "[0-9A-Fa-f]"
                }),
                b'H' => out.push_str("[^0-9A-Fa-f]"),
                b'Z' => out.push_str("(?=\\n?\\z)"),
                // Syntax::ruby() has no GNU word-start/word-end escapes.
                // Rust interprets these as word assertions, so unescape them.
                b'<' | b'>' => out.push(c as char),
                b'w' => out.push_str("[\\p{Alphabetic}\\p{M}\\p{Nd}\\p{Pc}]"),
                b'W' => out.push_str("[^\\p{Alphabetic}\\p{M}\\p{Nd}\\p{Pc}]"),
                // Ruby accepts escaped whitespace/# even without x; Rust accepts
                // literal whitespace, but rejects escapes of non-ASCII chars.
                b' ' | b'\t' | b'\n' | b'\r' => out.push(c as char),
                _ => {
                    out.push('\\');
                    out.push(c as char);
                }
            }
            i += 2;
            continue;
        }
        if classes > 0
            && ["--", "~~", "||"]
                .iter()
                .any(|operator| p[i..].starts_with(operator))
        {
            // These are Rust class operators, but literal text or invalid in
            // Onig's Ruby syntax. Rust compilation alone does not prove equivalence.
            return Err("Rust-only character-class operator".into());
        }
        if classes > 0
            && b[i] == b'['
            && b.get(i + 1) == Some(&b':')
            && let Some(end) = p[i + 2..].find(":]")
        {
            let mut name = &p[i + 2..i + 2 + end];
            let neg = name.starts_with('^');
            if neg {
                name = &name[1..];
            }
            let property = match name {
                "alpha" => Some("Alphabetic"),
                "alnum" => Some("ONIG_ALNUM"),
                "lower" => Some("Lowercase"),
                "upper" => Some("Uppercase"),
                "digit" => Some("Nd"),
                "space" => Some("White_Space"),
                "blank" => Some("ONIG_BLANK"),
                "cntrl" => Some("Cc"),
                "punct" => Some("ONIG_PUNCT"),
                "word" => Some("ONIG_WORD"),
                "ascii" => Some("ASCII"),
                "xdigit" => Some("ONIG_HEX"),
                // graph/print have subtle Unicode rules; leave to Onig.
                _ => None,
            };
            if let Some(prop) = property {
                let body = match prop {
                    "ONIG_ALNUM" => "\\p{Alphabetic}\\p{Nd}",
                    "ONIG_BLANK" => "\\t\\p{Zs}",
                    "ONIG_PUNCT" => "\\p{P}\\p{S}",
                    "ONIG_WORD" => "\\p{Alphabetic}\\p{M}\\p{Nd}\\p{Pc}",
                    "ONIG_HEX" => "0-9A-Fa-f",
                    _ => "",
                };
                if body.is_empty() {
                    out.push_str(if neg { "\\P{" } else { "\\p{" });
                    out.push_str(prop);
                    out.push('}');
                } else {
                    out.push_str(if neg { "[^" } else { "[" });
                    out.push_str(body);
                    out.push(']');
                }
                i += end + 4;
                continue;
            }
            return Err(format!("unsupported POSIX class: {name}"));
        }
        if b[i] == b'[' {
            classes += 1;
            out.push('[');
            i += 1;
            continue;
        }
        if b[i] == b']' && classes > 0 {
            classes -= 1;
            out.push(']');
            i += 1;
            continue;
        }
        if classes == 0 {
            if p[i..].starts_with("{,")
                && let Some(end) = p[i + 2..].find('}')
            {
                let n = &p[i + 2..i + 2 + end];
                if !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) {
                    out.push_str("{0,");
                    out.push_str(n);
                    out.push('}');
                    i += end + 3;
                    continue;
                }
            }
            if extended && b[i].is_ascii_whitespace() {
                i += 1;
                continue;
            }
            if extended && b[i] == b'#' {
                i += memchr::memchr(b'\n', &b[i..]).unwrap_or(b.len() - i);
                continue;
            }
            if p[i..].starts_with("(?#") {
                let mut j = i + 3;
                while j < b.len() && b[j] != b')' {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                if j == b.len() {
                    return Err("unterminated comment group".into());
                }
                i = j + 1;
                continue;
            }
            if b[i] == b'(' {
                if p[i..].starts_with("(?") {
                    let mut j = i + 2;
                    while j < b.len() && (b[j].is_ascii_alphabetic() || b[j] == b'-') {
                        j += 1;
                    }
                    if j > i + 2 && matches!(b.get(j), Some(b':' | b')')) {
                        let flags = &p[i + 2..j];
                        if !flags
                            .bytes()
                            .all(|c| matches!(c, b'i' | b'm' | b'x' | b'-'))
                        {
                            return Err(format!("unsupported Onig flags: {flags}"));
                        }
                        let old_x = extended;
                        let mut enable = true;
                        let mut yes = String::new();
                        let mut no = String::new();
                        for f in flags.bytes() {
                            if f == b'-' {
                                enable = false;
                                continue;
                            }
                            if f == b'x' {
                                extended = enable;
                                continue;
                            }
                            let mapped = if f == b'm' { 's' } else { f as char };
                            if enable {
                                yes.push(mapped);
                            } else {
                                no.push(mapped);
                            }
                        }
                        let mapped = if no.is_empty() {
                            yes
                        } else {
                            format!("{yes}-{no}")
                        };
                        if b[j] == b':' {
                            stack.push(old_x);
                            if mapped.is_empty() {
                                out.push_str("(?:");
                            } else {
                                out.push_str(&format!("(?{mapped}:"));
                            }
                        } else if !mapped.is_empty() {
                            out.push_str(&format!("(?{mapped})"));
                        }
                        i = j + 1;
                        continue;
                    }
                }
                stack.push(extended);
            } else if b[i] == b')' {
                extended = stack.pop().ok_or("unbalanced parentheses")?;
            }
        }
        let c = p[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    if !stack.is_empty() || classes > 0 {
        return Err("unbalanced group/class (or literal closing bracket)".into());
    }
    Ok(out)
}

// Span of each atom at the outermost concatenation level. Alternation is kept
// conservative: a boundary assertion on just one branch cannot be peeled.
fn atoms(p: &str) -> Option<Vec<(usize, usize)>> {
    let b = p.as_bytes();
    let mut i = 0;
    let mut out = vec![];
    while i < b.len() {
        let start = i;
        match b[i] {
            b'|' => return None,
            b'(' => {
                let mut depth = 1;
                let mut class = 0;
                i += 1;
                while i < b.len() && depth > 0 {
                    match b[i] {
                        b'\\' => {
                            i += 2;
                            continue;
                        }
                        b'[' => class += 1,
                        b']' if class > 0 => class -= 1,
                        b'(' if class == 0 => depth += 1,
                        b')' if class == 0 => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                if depth != 0 {
                    return None;
                }
            }
            b'[' => {
                let mut depth = 1;
                i += 1;
                while i < b.len() && depth > 0 {
                    match b[i] {
                        b'\\' => {
                            i += 2;
                            continue;
                        }
                        b'[' => depth += 1,
                        b']' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'\\' => {
                i += 2;
                if i > b.len() {
                    return None;
                }
            }
            _ => {
                i += p[i..].chars().next()?.len_utf8();
            }
        }
        // Attach any quantifier; never peel a quantified assertion.
        if matches!(b.get(i), Some(b'*' | b'+' | b'?')) {
            i += 1;
            if matches!(b.get(i), Some(b'?' | b'+')) {
                i += 1;
            }
        } else if b.get(i) == Some(&b'{')
            && let Some(end) = p[i..].find('}')
        {
            i += end + 1;
            if matches!(b.get(i), Some(b'?' | b'+')) {
                i += 1;
            }
        }
        out.push((start, i));
    }
    Some(out)
}

fn outer(p: &str) -> Option<(&str, &str)> {
    let a = atoms(p)?;
    if a.len() != 1 || a[0] != (0, p.len()) || !p.ends_with(')') {
        return None;
    }
    if p.starts_with("(?:") {
        return Some((&p[3..p.len() - 1], ""));
    }
    if p.starts_with("(?") {
        let colon = p.find(':')?;
        let f = &p[2..colon];
        if !f.is_empty() && f.bytes().all(|c| matches!(c, b'i' | b's' | b'-')) {
            return Some((&p[colon + 1..p.len() - 1], f));
        }
    } else if p.starts_with('(') {
        return Some((&p[1..p.len() - 1], ""));
    }
    None
}

fn guard(p: &str, leading: bool) -> Option<Guard> {
    if !p.ends_with(')') {
        return None;
    }
    let positive = if p.starts_with("(?=") {
        true
    } else if p.starts_with("(?!") {
        false
    } else {
        return None;
    };
    Some(Guard {
        positive,
        leading,
        expr: p[3..p.len() - 1].into(),
    })
}

fn wrap(s: &str, wrappers: &[String]) -> String {
    wrappers.iter().rev().fold(s.to_string(), |s, f| {
        if f.is_empty() {
            format!("(?:{s})")
        } else {
            format!("(?{f}:{s})")
        }
    })
}

fn regular(s: &str) -> bool {
    regex_syntax::ParserBuilder::new()
        .multi_line(true)
        .build()
        .parse(s)
        .is_ok()
}

pub fn translate(original: &str) -> Translation {
    let normalized = match normalize(original) {
        Ok(p) => p,
        Err(reason) => {
            return Translation {
                class: Kind::Backtracking,
                core: None,
                guards: vec![],
                reason,
            };
        }
    };
    let mut t = Translation {
        class: Kind::Backtracking,
        core: None,
        guards: vec![],
        reason: "internal assertion/backreference/unsupported construct".into(),
    };
    // regex-syntax accepts a++ as nested repetition. In Onig this is
    // possessive repetition, whose backtracking semantics are different.
    let mut escaped = false;
    let mut depth = 0usize;
    let mut previous = 0u8;
    for c in normalized.bytes() {
        if escaped {
            escaped = false;
            previous = 0;
            continue;
        }
        if c == b'\\' {
            escaped = true;
            previous = 0;
            continue;
        }
        if c == b'[' {
            depth += 1;
        } else if c == b']' && depth > 0 {
            depth -= 1;
        }
        if depth == 0 && c == b'+' && matches!(previous, b'*' | b'+' | b'?' | b'}') {
            t.reason = "possessive quantifier".into();
            return t;
        }
        previous = c;
    }
    if regular(&normalized) {
        t.class = Kind::Dfa;
        t.core = Some(normalized);
        t.reason.clear();
        return t;
    }
    let mut inner = normalized.as_str();
    let mut wrappers = vec![];
    while let Some((s, f)) = outer(inner) {
        wrappers.push(f.to_string());
        inner = s;
    }
    let Some(a) = atoms(inner) else {
        return t;
    };
    let mut lo = 0;
    let mut hi = a.len();
    let mut guards = vec![];
    while lo < hi {
        let (s, e) = a[lo];
        if let Some(g) = guard(&inner[s..e], true) {
            guards.push(g);
            lo += 1;
        } else {
            break;
        }
    }
    while lo < hi {
        let (s, e) = a[hi - 1];
        if let Some(g) = guard(&inner[s..e], false) {
            guards.push(g);
            hi -= 1;
        } else {
            break;
        }
    }
    if guards.is_empty() {
        return t;
    }
    let core = wrap(
        if lo == hi {
            ""
        } else {
            &inner[a[lo].0..a[hi - 1].1]
        },
        &wrappers,
    );
    for g in &mut guards {
        g.expr = wrap(&g.expr, &wrappers);
    }
    if regular(&core) && guards.iter().all(|g| regular(&g.expr)) {
        t.class = Kind::Boundary;
        t.core = Some(core);
        t.guards = guards;
        t.reason.clear();
    }
    t
}

// Restrict Unicode classes to the ASCII domain, and use ASCII boundaries. A candidate
// is never used on a non-ASCII haystack: Onig's word class (Alphabetic, Mark, Nd, Pc)
// differs from Rust's Unicode boundary class, notably for ²³¹¼½¾ and join controls.
fn ascii(hir: Hir) -> Hir {
    match hir.into_kind() {
        HirKind::Class(Class::Unicode(class)) => Hir::class(Class::Bytes(ClassBytes::new(
            class.iter().filter_map(|r| {
                let start = u32::from(r.start());
                (start < 128)
                    .then(|| ClassBytesRange::new(start as u8, u32::from(r.end()).min(127) as u8))
            }),
        ))),
        HirKind::Look(look) => Hir::look(match look {
            Look::WordUnicode => Look::WordAscii,
            Look::WordUnicodeNegate => Look::WordAsciiNegate,
            _ => look,
        }),
        HirKind::Repetition(mut repetition) => {
            repetition.sub = Box::new(ascii(*repetition.sub));
            Hir::repetition(repetition)
        }
        HirKind::Capture(mut capture) => {
            capture.sub = Box::new(ascii(*capture.sub));
            Hir::capture(capture)
        }
        HirKind::Concat(parts) => Hir::concat(parts.into_iter().map(ascii).collect()),
        HirKind::Alternation(parts) => Hir::alternation(parts.into_iter().map(ascii).collect()),
        kind => match kind {
            HirKind::Empty => Hir::empty(),
            HirKind::Literal(literal) => Hir::literal(literal.0),
            HirKind::Class(class) => Hir::class(class),
            _ => unreachable!("all HIR kinds handled"),
        },
    }
}

#[derive(Debug)]
pub struct Candidate {
    pub hir: Hir,
    pub exact: bool,
    pub guards: Vec<(bool, bool, Hir)>,
    pub captures: Option<Vec<(u32, Offset, Offset)>>,
    pub start_lf: bool,
    pub zero_width: bool,
}

pub fn literals(hir: &Hir) -> Option<Vec<Vec<u8>>> {
    let result = match hir.kind() {
        HirKind::Literal(literal) => vec![literal.0.to_vec()],
        HirKind::Capture(capture) => literals(&capture.sub)?,
        HirKind::Alternation(parts) => {
            let mut result = Vec::new();
            for part in parts {
                result.extend(literals(part)?);
                if result.len() > 64 {
                    return None;
                }
            }
            result
        }
        HirKind::Concat(parts) => {
            let mut result = vec![Vec::new()];
            for part in parts {
                let suffixes = literals(part)?;
                if result.len() * suffixes.len() > 64 {
                    return None;
                }
                result = result
                    .iter()
                    .flat_map(|prefix| {
                        suffixes.iter().map(move |suffix| {
                            let mut literal = prefix.clone();
                            literal.extend_from_slice(suffix);
                            literal
                        })
                    })
                    .collect();
            }
            result
        }
        _ => return None,
    };
    result
        .iter()
        .all(|literal| !literal.is_empty())
        .then_some(result)
}

#[derive(Debug, Clone, Copy)]
pub enum Offset {
    Start(usize),
    End(usize),
}
impl Offset {
    fn add(self, n: usize) -> Option<Self> {
        Some(match self {
            Self::Start(at) => Self::Start(at.checked_add(n)?),
            Self::End(at) => Self::End(at.checked_sub(n)?),
        })
    }
    fn sub(self, n: usize) -> Option<Self> {
        Some(match self {
            Self::Start(at) => Self::Start(at.checked_sub(n)?),
            Self::End(at) => Self::End(at.checked_add(n)?),
        })
    }
    pub fn resolve(self, start: usize, end: usize) -> usize {
        match self {
            Self::Start(n) => start + n,
            Self::End(n) => end - n,
        }
    }
}
fn fixed(hir: &Hir) -> Option<usize> {
    let min = hir.properties().minimum_len()?;
    (hir.properties().maximum_len() == Some(min)).then_some(min)
}
fn capture_layout(
    hir: &Hir,
    start: Option<Offset>,
    end: Option<Offset>,
    out: &mut Vec<(u32, Offset, Offset)>,
) -> Option<()> {
    if hir.properties().explicit_captures_len() == 0 {
        return Some(());
    }
    match hir.kind() {
        HirKind::Capture(capture) => {
            out.push((capture.index, start?, end?));
            capture_layout(&capture.sub, start, end, out)
        }
        HirKind::Concat(parts) => {
            let mut before = Some(0usize);
            for (i, part) in parts.iter().enumerate() {
                let after = parts[i + 1..]
                    .iter()
                    .try_fold(0usize, |n, h| n.checked_add(fixed(h)?));
                let begin = start
                    .and_then(|s| s.add(before?))
                    .or_else(|| end?.sub(after?.checked_add(fixed(part)?)?));
                let finish = end
                    .and_then(|e| e.sub(after?))
                    .or_else(|| start?.add(before?.checked_add(fixed(part)?)?));
                capture_layout(part, begin, finish, out)?;
                before = before.and_then(|n| n.checked_add(fixed(part)?));
            }
            Some(())
        }
        _ => None,
    }
}
fn has_captures(source: &str) -> bool {
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut class = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'[' => class += 1,
            b']' if class > 0 => class -= 1,
            b'(' if class == 0
                && (bytes.get(i + 1) != Some(&b'?')
                    || (source[i..].starts_with("(?<")
                        && !source[i..].starts_with("(?<=")
                        && !source[i..].starts_with("(?<!"))
                    || source[i..].starts_with("(?'")) =>
            {
                return true;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

// Only single-byte, side-effect-free predicates are interpreted here. All other
// assertions retain Onig verification. The bit mask denotes accepted lengths 0/1.
fn predicate(hir: &Hir, line: &[u8], at: usize) -> u8 {
    match hir.kind() {
        HirKind::Empty => 1,
        HirKind::Literal(literal) if literal.0.len() == 1 => {
            if line.get(at) == literal.0.first() {
                2
            } else {
                0
            }
        }
        HirKind::Class(Class::Bytes(class)) => {
            if line
                .get(at)
                .is_some_and(|b| class.iter().any(|r| r.start() <= *b && *b <= r.end()))
            {
                2
            } else {
                0
            }
        }
        HirKind::Look(look) => {
            let word =
                |byte: Option<&u8>| byte.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
            let before = at.checked_sub(1).and_then(|p| line.get(p));
            let after = line.get(at);
            let yes = match look {
                Look::Start => at == 0,
                Look::End => at == line.len(),
                Look::StartLF => {
                    (at == 0 || before == Some(&b'\n'))
                        && !(at == line.len() && line.ends_with(b"\n"))
                }
                Look::EndLF => at == line.len() || after == Some(&b'\n'),
                Look::WordAscii => word(before) != word(after),
                Look::WordAsciiNegate => word(before) == word(after),
                _ => false,
            };
            u8::from(yes)
        }
        HirKind::Alternation(parts) => parts
            .iter()
            .fold(0, |mask, h| mask | predicate(h, line, at)),
        HirKind::Concat(parts) => parts.iter().fold(1, |mask, h| {
            let mut next = 0;
            if mask & 1 != 0 {
                next |= predicate(h, line, at);
            }
            if mask & 2 != 0 {
                next |= (predicate(h, line, at + 1) & 1) << 1;
            }
            next
        }),
        _ => 0,
    }
}
fn bounded(hir: &Hir) -> bool {
    match hir.kind() {
        HirKind::Empty | HirKind::Class(Class::Bytes(_)) => true,
        HirKind::Literal(literal) => literal.0.len() == 1,
        HirKind::Look(look) => matches!(
            look,
            Look::Start
                | Look::End
                | Look::StartLF
                | Look::EndLF
                | Look::WordAscii
                | Look::WordAsciiNegate
        ),
        HirKind::Concat(parts) | HirKind::Alternation(parts) => {
            parts.iter().all(bounded) && hir.properties().maximum_len().is_some_and(|n| n <= 1)
        }
        _ => false,
    }
}
impl Candidate {
    pub fn guards_match(&self, line: &str, start: usize, end: usize) -> bool {
        self.guards.iter().all(|(positive, leading, hir)| {
            (predicate(hir, line.as_bytes(), if *leading { start } else { end }) != 0) == *positive
        })
    }
}
pub fn candidate(source: &str) -> Option<Candidate> {
    // Onig supports multi-character case folding even on ASCII input (ß -> ss).
    // Keep these rules in the reference engine rather than guessing equivalence.
    if source.contains(r"\u")
        || (!source.is_ascii()
            && source.split("(?").skip(1).any(|s| {
                s.chars()
                    .take_while(|c| c.is_ascii_alphabetic() || *c == '-')
                    .any(|c| c == 'i')
            }))
    {
        return None;
    }
    let translation = translate(source);
    let Some(mut core) = translation.core else {
        let normalized = normalize(source).ok()?;
        let relaxed = relax(&normalized)?;
        let hir = regex_syntax::ParserBuilder::new()
            .multi_line(true)
            .build()
            .parse(&relaxed)
            .ok()?;
        if hir.properties().minimum_len()? == 0 {
            return None;
        }
        return Some(Candidate {
            hir: ascii(hir),
            exact: false,
            guards: Vec::new(),
            captures: None,
            start_lf: false,
            zero_width: false,
        });
    };
    let parser = || regex_syntax::ParserBuilder::new().multi_line(true).build();
    let hir = ascii(parser().parse(&core).ok()?);
    let start_lf = hir.properties().look_set().contains(Look::StartLF);
    let mut exact = translation.class == Kind::Dfa;
    let capture_free = !has_captures(source);
    let captures = if exact {
        let mut layout = Vec::new();
        capture_layout(
            &hir,
            Some(Offset::Start(0)),
            Some(Offset::End(0)),
            &mut layout,
        )
        .map(|()| layout)
    } else {
        None
    };
    if translation.guards.is_empty() {
        return Some(Candidate {
            hir,
            exact,
            guards: Vec::new(),
            captures,
            start_lf,
            zero_width: false,
        });
    }
    let mut guards = Vec::new();
    for guard in &translation.guards {
        let test = ascii(parser().parse(&guard.expr).ok()?);
        if !bounded(&test) {
            break;
        }
        guards.push((guard.positive, guard.leading, test));
    }
    if !translation.guards.is_empty()
        && guards.len() == translation.guards.len()
        && hir.properties().maximum_len() != Some(0)
    {
        exact = true;
        return Some(Candidate {
            hir: ascii(hir),
            exact,
            guards,
            captures: capture_free.then(Vec::new),
            start_lf,
            zero_width: false,
        });
    }
    guards.clear();
    // A positive trailing guard can be consumed by the candidate machine: its
    // end is never reported. This includes bounded whitespace/word assertions.
    // Assertion-only rules use a positive guard as their locator, avoiding an
    // epsilon alternative that would nominate them at every byte of every line.
    for guard in &translation.guards {
        if guard.positive && (!guard.leading || hir.properties().maximum_len() == Some(0)) {
            core = format!("(?:{core})(?:{})", guard.expr);
            // Independent assertions all start at the same core boundary. Consuming
            // more than one sequentially would require disjoint text and lose matches.
            break;
        }
    }
    let candidate = parser().parse(&core).ok()?;
    let zero_width = capture_free
        && translation.guards.len() == 1
        && translation.guards[0].positive
        && hir.properties().maximum_len() == Some(0);
    let start_lf = candidate.properties().look_set().contains(Look::StartLF);
    Some(Candidate {
        hir: ascii(candidate),
        exact: exact || zero_width,
        guards,
        captures: if zero_width {
            Some(Vec::new())
        } else {
            captures
        },
        start_lf,
        zero_width,
    })
}

// Removing internal assertions produces a language superset, not a translated
// exact matcher. The original Onig pattern always verifies these candidates.
fn relax(source: &str) -> Option<String> {
    if source.contains(r"\G") || source.contains(r"\K") {
        return None;
    }
    let bytes = source.as_bytes();
    let mut out = String::new();
    let mut at = 0;
    let mut class = 0usize;
    while at < bytes.len() {
        if bytes[at] == b'\\' {
            out.push('\\');
            at += 1;
            let ch = source[at..].chars().next()?;
            out.push(ch);
            at += ch.len_utf8();
            continue;
        }
        if bytes[at] == b'[' {
            class += 1;
        }
        if bytes[at] == b']' && class > 0 {
            class -= 1;
        }
        if class == 0
            && ["(?=", "(?!", "(?<=", "(?<!"]
                .iter()
                .any(|prefix| source[at..].starts_with(prefix))
        {
            let mut depth = 1;
            let mut inner_class = 0;
            at += 3;
            while at < bytes.len() && depth > 0 {
                match bytes[at] {
                    b'\\' => {
                        at += 2;
                        continue;
                    }
                    b'[' => inner_class += 1,
                    b']' if inner_class > 0 => inner_class -= 1,
                    b'(' if inner_class == 0 => depth += 1,
                    b')' if inner_class == 0 => depth -= 1,
                    _ => {}
                }
                at += 1;
            }
            if depth != 0 {
                return None;
            }
            out.push_str("(?:)");
            continue;
        }
        let ch = source[at..].chars().next()?;
        out.push(ch);
        at += ch.len_utf8();
    }
    Some(out)
}
