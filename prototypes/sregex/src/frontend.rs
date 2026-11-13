//! Conservative Oniguruma-to-Rust syntax front end. Unrecognized constructs
//! stay in Oniguruma; compilation success is NOT by itself a proof of semantic
//! equivalence (notably Unicode word boundaries/case folding).
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum Class { Dfa, Boundary, Backtracking }

#[derive(Debug, Clone, Serialize)]
pub struct Guard {
    pub positive: bool,
    pub leading: bool,
    pub expr: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Translation {
    pub class: Class,
    pub normalized: String,
    pub core: Option<String>,
    pub guards: Vec<Guard>,
    pub reason: String,
    pub position_dependent: bool,
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
            if i + 1 == b.len() { out.push('\\'); break; }
            let c = b[i + 1];
            if !c.is_ascii() {
                let ch=p[i+1..].chars().next().unwrap();out.push(ch);i+=1+ch.len_utf8();continue;
            }
            match c {
                b'h' => out.push_str(if classes > 0 { "0-9A-Fa-f" } else { "[0-9A-Fa-f]" }),
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
                _ => { out.push('\\'); out.push(c as char); }
            }
            i += 2; continue;
        }
        if classes > 0 && b[i] == b'[' && b.get(i + 1) == Some(&b':') {
            if let Some(end) = p[i + 2..].find(":]") {
                let mut name = &p[i + 2..i + 2 + end];
                let neg = name.starts_with('^'); if neg { name = &name[1..]; }
                let property = match name {
                    "alpha" => Some("Alphabetic"), "alnum" => Some("ONIG_ALNUM"),
                    "lower" => Some("Lowercase"), "upper" => Some("Uppercase"),
                    "digit" => Some("Nd"), "space" => Some("White_Space"),
                    "blank" => Some("ONIG_BLANK"), "cntrl" => Some("Cc"),
                    "punct" => Some("ONIG_PUNCT"), "word" => Some("ONIG_WORD"),
                    "ascii" => Some("ASCII"), "xdigit" => Some("ONIG_HEX"),
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
                    if body.is_empty() { out.push_str(if neg { "\\P{" } else { "\\p{" }); out.push_str(prop); out.push('}'); }
                    else { out.push_str(if neg { "[^" } else { "[" }); out.push_str(body); out.push(']'); }
                    i += end + 4; continue;
                }
                return Err(format!("unsupported POSIX class: {name}"));
            }
        }
        if b[i] == b'[' { classes += 1; out.push('['); i += 1; continue; }
        if b[i] == b']' && classes > 0 { classes -= 1; out.push(']'); i += 1; continue; }
        if classes == 0 {
            if p[i..].starts_with("{,") {
                if let Some(end)=p[i+2..].find('}') {
                    let n=&p[i+2..i+2+end];
                    if !n.is_empty() && n.bytes().all(|c|c.is_ascii_digit()) {
                        out.push_str("{0,");out.push_str(n);out.push('}');i+=end+3;continue;
                    }
                }
            }
            if extended && b[i].is_ascii_whitespace() { i += 1; continue; }
            if extended && b[i] == b'#' {
                i += memchr::memchr(b'\n', &b[i..]).unwrap_or(b.len() - i);
                continue;
            }
            if p[i..].starts_with("(?#") {
                let mut j = i + 3;
                while j < b.len() && b[j] != b')' { if b[j] == b'\\' { j += 1; } j += 1; }
                if j == b.len() { return Err("unterminated comment group".into()); }
                i = j + 1; continue;
            }
            if b[i] == b'(' {
                if p[i..].starts_with("(?") {
                    let mut j = i + 2;
                    while j < b.len() && (b[j].is_ascii_alphabetic() || b[j] == b'-') { j += 1; }
                    if j > i + 2 && matches!(b.get(j), Some(b':' | b')')) {
                        let flags = &p[i + 2..j];
                        if !flags.bytes().all(|c| matches!(c, b'i' | b'm' | b'x' | b'-')) {
                            return Err(format!("unsupported Onig flags: {flags}"));
                        }
                        let old_x = extended;
                        let mut enable = true;
                        let mut yes = String::new(); let mut no = String::new();
                        for f in flags.bytes() {
                            if f == b'-' { enable = false; continue; }
                            if f == b'x' { extended = enable; continue; }
                            let mapped = if f == b'm' { 's' } else { f as char };
                            if enable { yes.push(mapped); } else { no.push(mapped); }
                        }
                        let mapped = if no.is_empty() { yes } else { format!("{yes}-{no}") };
                        if b[j] == b':' {
                            stack.push(old_x);
                            if mapped.is_empty() { out.push_str("(?:"); } else { out.push_str(&format!("(?{mapped}:")); }
                        } else if !mapped.is_empty() { out.push_str(&format!("(?{mapped})")); }
                        i = j + 1; continue;
                    }
                }
                stack.push(extended);
            } else if b[i] == b')' {
                extended = stack.pop().ok_or("unbalanced parentheses")?;
            }
        }
        let c = p[i..].chars().next().unwrap(); out.push(c); i += c.len_utf8();
    }
    if !stack.is_empty() || classes > 0 { return Err("unbalanced group/class (or literal closing bracket)".into()); }
    Ok(out)
}

/// Onig ^ does not match the phantom empty line after a terminal newline.
/// fancy supports the negative assertion needed to express that difference.
pub fn for_fancy(normalized:&str)->String {
    let mut out=String::new();let mut chars=normalized.chars();let mut class=0usize;
    while let Some(c)=chars.next(){
        if c=='\\'{out.push(c);if let Some(n)=chars.next(){out.push(n);}continue;}
        if c=='['{class+=1;}else if c==']' && class>0{class-=1;}
        if c=='^' && class==0{out.push_str("(?:\\A|^(?!\\z))");}else{out.push(c);}
    }out
}

// Span of each atom at the outermost concatenation level. Alternation is kept
// conservative: a boundary assertion on just one branch cannot be peeled.
fn atoms(p: &str) -> Option<Vec<(usize, usize)>> {
    let b = p.as_bytes(); let mut i = 0; let mut out = vec![];
    while i < b.len() {
        let start = i;
        match b[i] {
            b'|' => return None,
            b'(' => {
                let mut depth = 1; let mut class = 0; i += 1;
                while i < b.len() && depth > 0 {
                    match b[i] {
                        b'\\' => { i += 2; continue; }
                        b'[' => class += 1,
                        b']' if class > 0 => class -= 1,
                        b'(' if class == 0 => depth += 1,
                        b')' if class == 0 => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                if depth != 0 { return None; }
            }
            b'[' => {
                let mut depth = 1; i += 1;
                while i < b.len() && depth > 0 {
                    match b[i] { b'\\' => {i += 2; continue;}, b'[' => depth += 1, b']' => depth -= 1, _ => {} }
                    i += 1;
                }
            }
            b'\\' => { i += 2; if i > b.len() { return None; } }
            _ => { i += p[i..].chars().next()?.len_utf8(); }
        }
        // Attach any quantifier; never peel a quantified assertion.
        if matches!(b.get(i), Some(b'*' | b'+' | b'?')) { i += 1; if matches!(b.get(i), Some(b'?' | b'+')) { i += 1; } }
        else if b.get(i) == Some(&b'{') {
            if let Some(end) = p[i..].find('}') { i += end + 1; if matches!(b.get(i),Some(b'?'|b'+')) { i += 1; } }
        }
        out.push((start, i));
    }
    Some(out)
}

fn outer(p: &str) -> Option<(&str, &str)> {
    let a = atoms(p)?;
    if a.len() != 1 || a[0] != (0, p.len()) || !p.ends_with(')') { return None; }
    if p.starts_with("(?:") { return Some((&p[3..p.len()-1], "")); }
    if p.starts_with("(?") {
        let colon = p.find(':')?; let f = &p[2..colon];
        if !f.is_empty() && f.bytes().all(|c| matches!(c, b'i'|b's'|b'-')) { return Some((&p[colon+1..p.len()-1],f)); }
    } else if p.starts_with('(') { return Some((&p[1..p.len()-1], "")); }
    None
}

fn guard(p: &str, leading: bool) -> Option<Guard> {
    if !p.ends_with(')') { return None; }
    let positive = if p.starts_with("(?=") { true } else if p.starts_with("(?!") { false } else { return None; };
    Some(Guard { positive, leading, expr: p[3..p.len()-1].into() })
}

fn wrap(s: &str, wrappers: &[String]) -> String {
    wrappers.iter().rev().fold(s.to_string(), |s,f| if f.is_empty() {format!("(?:{s})")} else {format!("(?{f}:{s})")})
}

fn regular(s: &str) -> bool {
    regex_syntax::ParserBuilder::new().multi_line(true).build().parse(s).is_ok()
}

pub fn translate(original: &str) -> Translation {
    let dependent = original.contains("\\G") || original.contains("\\K");
    let normalized = match normalize(original) {
        Ok(p) => p,
        Err(reason) => return Translation {class: Class::Backtracking, normalized: original.into(),core:None, guards:vec![],reason,position_dependent:dependent},
    };
    let mut t = Translation {class:Class::Backtracking,normalized:normalized.clone(),core:None,guards:vec![],reason:"internal assertion/backreference/unsupported construct".into(),position_dependent:dependent};
    // regex-syntax accepts a++ as nested repetition. In Onig this is
    // possessive repetition, whose backtracking semantics are different.
    let mut escaped=false;let mut depth=0usize;let mut previous=0u8;
    for c in normalized.bytes() {
        if escaped {escaped=false;previous=0;continue;}
        if c==b'\\' {escaped=true;previous=0;continue;}
        if c==b'['{depth+=1;}else if c==b']' && depth>0{depth-=1;}
        if depth==0 && c==b'+' && matches!(previous,b'*'|b'+'|b'?'|b'}') {
            t.reason="possessive quantifier".into();return t;
        }
        previous=c;
    }
    if regular(&normalized) { t.class=Class::Dfa; t.core=Some(normalized);t.reason.clear();return t; }
    let mut inner = normalized.as_str(); let mut wrappers = vec![];
    while let Some((s,f)) = outer(inner) { wrappers.push(f.to_string()); inner=s; }
    let Some(a) = atoms(inner) else { return t; };
    let mut lo=0;let mut hi=a.len();let mut guards=vec![];
    while lo < hi {
        let (s,e)=a[lo]; if let Some(g)=guard(&inner[s..e], true) {guards.push(g);lo+=1;} else {break;}
    }
    while lo < hi {
        let (s,e)=a[hi-1]; if let Some(g)=guard(&inner[s..e], false) {guards.push(g);hi-=1;} else {break;}
    }
    if guards.is_empty() { return t; }
    let core = wrap(if lo == hi {""} else {&inner[a[lo].0..a[hi-1].1]}, &wrappers);
    for g in &mut guards {g.expr=wrap(&g.expr,&wrappers);}
    if regular(&core) && guards.iter().all(|g|regular(&g.expr)) {
        t.class=Class::Boundary;t.core=Some(core);t.guards=guards;t.reason.clear();
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn onig_flags_classes_and_comments() {
        assert_eq!(normalize("(?x) a [ #] \\h # hi\n (?m:b.)").unwrap(),"a[ #][0-9A-Fa-f](?s:b.)");
        assert_eq!(normalize(r"[[:alpha:][:digit:]_]").unwrap(),r"[\p{Alphabetic}\p{Nd}_]");
    }
    #[test] fn boundary_classification_is_conservative() {
        for p in ["(?=a)","(?!a)b+","a+(?=b)","(?i:(?=a)a+(?!b))"] {assert_eq!(translate(p).class,Class::Boundary,"{p}");}
        for p in ["a(?=b)c","(?=a)|b",r"(a)\1",r"\Gfoo","a++"] {assert_eq!(translate(p).class,Class::Backtracking,"{p}");}
        assert_eq!(translate(r"\b\h+\b").class,Class::Dfa);
    }
}
