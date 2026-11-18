//! Scope-gated native completions. Called by the snapshot worker, not by the input event.

#[path = "markup_data.rs"]
mod data;

use super::{CompletionFormat, CompletionItem, Invocation, Kind, KindId};
use std::sync::LazyLock;
use vel_scope::{Scope, Selector};
use vel_text::Text;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Slot {
    Tag { after_open: bool, has_end: bool },
    Attribute { tag: String, assigned: bool },
    HtmlValue { tag: String, name: String },
    CssProperty { assigned: bool, terminated: bool },
    CssValue(String),
}

const LOOK_BEHIND: usize = 2048;

// The HTML package's default_completions_selector controls where its completions
// are offered. Keep Markdown prose out, but allow an actual HTML syntax in a fence.
// https://github.com/sublimehq/Packages/blob/master/HTML/HTML.sublime-settings
static DEFAULT_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "source.css, text.html - (text.html.markdown - markup.raw.code-fence text.html)",
    )
    .expect("built-in markup completion selector")
});

pub(super) fn selector(setting: Option<&str>) -> Option<std::borrow::Cow<'static, Selector>> {
    match setting {
        Some(setting) => Selector::parse(setting).ok().map(std::borrow::Cow::Owned),
        None => Some(std::borrow::Cow::Borrowed(&DEFAULT_SELECTOR)),
    }
}

fn ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':')
}

fn token_start(s: &str, allowed: impl Fn(char) -> bool) -> usize {
    s.trim_end_matches(allowed).len()
}

/// Inspect only a small suffix of the published, exact context. In particular a CSS
/// selector never becomes a property position merely because there is an earlier brace.
pub(super) fn slot(
    text: &Text,
    at: usize,
    stack: &[Scope],
    completion_selector: &Selector,
) -> Option<(Slot, usize)> {
    let mut css = false;
    let mut html = false;
    let mut tag = false;
    let mut string = false;
    let mut declaration = false;
    let mut selector = false;
    for scope in stack {
        let name = scope.build_string();
        let has = |head: &str| {
            name == head || name.strip_prefix(head).is_some_and(|s| s.starts_with('.'))
        };
        if has("comment") {
            return None;
        }
        if has("text.html") || has("source.css") || has("source") {
            html = has("text.html");
            css = has("source.css");
        }
        tag |= has("meta.tag");
        string |= has("string");
        declaration |= has("meta.property-list.css");
        selector |= has("meta.selector.css");
    }
    if !html && (!css || !declaration || selector || string) {
        return None;
    }
    if !completion_selector.matches(stack) {
        return None;
    }
    let base = text.ceil_char_boundary(at.saturating_sub(LOOK_BEHIND));
    let before = text.slice(base..at).to_string();
    let next = text
        .slice(at..text.floor_char_boundary((at + 64).min(text.len_bytes())))
        .chars()
        .find(|c| !matches!(c, ' ' | '\t'));
    let (slot, start) = if css {
        css_slot(&before, next)?
    } else {
        html_slot(&before, next, tag, string)?
    };
    Some((slot, base + start))
}

fn css_slot(before: &str, after: Option<char>) -> Option<(Slot, usize)> {
    // https://www.sublimetext.com/3 (3019): no built-in CSS rows in selectors.
    let start = before.rfind(['{', '}', ';']).map_or(0, |p| p + 1);
    let expr = &before[start..];
    let name = |c: char| c.is_ascii_alphanumeric() || c == '-';
    if let Some((prop, values)) = expr.split_once(':') {
        let prop = prop.trim();
        if prop.is_empty() || !prop.chars().all(name) {
            return None;
        }
        return Some((
            Slot::CssValue(prop.to_ascii_lowercase()),
            before.len() - values.len() + token_start(values, name),
        ));
    }
    let prefix = expr.trim_start();
    prefix.chars().all(name).then_some((
        Slot::CssProperty {
            assigned: after == Some(':'),
            terminated: after == Some(';'),
        },
        before.len() - prefix.len(),
    ))
}

fn html_slot(
    before: &str,
    after: Option<char>,
    in_tag: bool,
    in_string: bool,
) -> Option<(Slot, usize)> {
    let mut open = None;
    let mut quote = None;
    for (i, ch) in before.char_indices() {
        match (open, quote, ch) {
            (Some(_), Some(q), c) if c == q => quote = None,
            (Some(_), None, '\'' | '"') => quote = Some(ch),
            (_, None, '<') => open = Some(i),
            (Some(_), None, '>') => open = None,
            _ => {}
        }
    }
    if let Some(i) = open {
        let tail = &before[i + 1..];
        let len = tail.find(|c| !ident(c)).unwrap_or(tail.len());
        if len == tail.len() && !in_string {
            return Some((
                Slot::Tag {
                    after_open: true,
                    has_end: after == Some('>'),
                },
                i + 1,
            ));
        }
        if !in_tag
            || len == 0
            || !tail
                .as_bytes()
                .get(len)
                .is_some_and(u8::is_ascii_whitespace)
        {
            return None;
        }
        let tag = tail[..len].to_ascii_lowercase();
        return attributes(tail, len, tag, after).map(|(slot, start)| (slot, i + 1 + start));
    }
    if in_tag || in_string {
        return None;
    }
    let start = token_start(before, ident);
    (start < before.len()).then_some((
        Slot::Tag {
            after_open: false,
            has_end: false,
        },
        start,
    ))
}

fn skip_space(t: &str, at: usize) -> usize {
    t.len() - t[at..].trim_start().len()
}

fn attributes(t: &str, mut at: usize, tag: String, after: Option<char>) -> Option<(Slot, usize)> {
    loop {
        at = skip_space(t, at);
        let begin = at;
        at += t[at..].find(|c| !ident(c)).unwrap_or(t.len() - at);
        if at == t.len() {
            return Some((
                Slot::Attribute {
                    tag,
                    assigned: after == Some('='),
                },
                begin,
            ));
        }
        if at == begin {
            return None;
        }
        let name = &t[begin..at];
        at = skip_space(t, at);
        if t.as_bytes().get(at) != Some(&b'=') {
            continue;
        }
        at = skip_space(t, at + 1);
        let quote = t
            .as_bytes()
            .get(at)
            .copied()
            .filter(|b| matches!(b, b'\'' | b'"'));
        let begin_value = at + usize::from(quote.is_some());
        let end = match quote {
            Some(q) => t[begin_value..].find(char::from(q)),
            None => t[begin_value..].find(char::is_whitespace),
        };
        if let Some(end) = end {
            at = begin_value + end + usize::from(quote.is_some());
        } else {
            let start = begin_value
                + token_start(&t[begin_value..], |c| {
                    ident(c) || matches!(c, '/' | '.' | '+')
                });
            return Some((
                Slot::HtmlValue {
                    tag,
                    name: name.to_ascii_lowercase(),
                },
                start,
            ));
        }
    }
}

fn lookup(table: &[(&'static str, &'static str)], name: &str) -> &'static str {
    table
        .binary_search_by_key(&name, |&(k, _)| k)
        .map_or("", |i| table[i].1)
}

impl Slot {
    pub(super) fn items(&self, prefix: &str, invocation: Invocation) -> Vec<CompletionItem> {
        let mut items = Vec::new();
        let prefix = prefix.to_ascii_lowercase();
        let mut add = |name: &str, label: &str, category: KindId, completion: String, snippet| {
            items.push(CompletionItem {
                trigger: name.into(),
                annotation: label.into(),
                completion,
                completion_format: if snippet {
                    CompletionFormat::Snippet
                } else {
                    CompletionFormat::Text
                },
                kind: Kind {
                    category,
                    letter: String::new(),
                    name: label.into(),
                },
                details: String::new(),
            });
        };
        let matches = |name: &str| {
            name.get(..prefix.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(&prefix))
        };
        match self {
            // ST3 3033: tag completion is available outside tags as well.
            // https://www.sublimetext.com/3
            Self::Tag {
                after_open,
                has_end,
            } => {
                for &(name, _) in data::HTML_ELEMENTS {
                    if !matches(name)
                        || !after_open && matches!(invocation, Invocation::Tab) && name != prefix
                    {
                        continue;
                    }
                    let open = if *after_open { "" } else { "<" };
                    let void = data::HTML_VOID
                        .split_ascii_whitespace()
                        .any(|tag| tag == name);
                    let (content, snippet) = if *has_end {
                        (name.into(), false)
                    } else if void {
                        (format!("{open}{name}>"), false)
                    } else {
                        (format!("{open}{name}>$0</{name}>"), true)
                    };
                    add(name, "HTML element", KindId::Markup, content, snippet);
                }
            }
            Self::Attribute { tag, assigned } => {
                for name in data::HTML_GLOBAL
                    .split_ascii_whitespace()
                    .chain(lookup(data::HTML_ELEMENTS, tag).split_ascii_whitespace())
                    .filter(|name| matches(name))
                {
                    let content = if *assigned {
                        name.into()
                    } else {
                        format!("{name}=\"$1\"$0")
                    };
                    add(name, "HTML attribute", KindId::Variable, content, !assigned);
                }
            }
            Self::HtmlValue { tag, name } => {
                let specific = lookup(data::HTML_VALUES, &format!("{tag}:{name}"));
                let values = if specific.is_empty() {
                    lookup(data::HTML_VALUES, &format!("*:{name}"))
                } else {
                    specific
                };
                for keyword in values
                    .split_ascii_whitespace()
                    .filter(|v| v.to_ascii_lowercase().starts_with(&prefix))
                {
                    add(
                        keyword,
                        "HTML value",
                        KindId::Keyword,
                        keyword.into(),
                        false,
                    );
                }
            }
            Self::CssProperty {
                assigned,
                terminated,
            } => {
                for &(name, _) in data::CSS_PROPERTIES
                    .iter()
                    .filter(|(name, _)| matches(name))
                {
                    let content = if *assigned {
                        name.into()
                    } else {
                        format!("{name}: $0{}", if *terminated { "" } else { ";" })
                    };
                    add(name, "CSS property", KindId::Variable, content, !assigned);
                }
            }
            Self::CssValue(prop) => {
                // https://www.w3.org/TR/css-cascade-5/#defaulting-keywords
                for keyword in lookup(data::CSS_PROPERTIES, prop)
                    .split_ascii_whitespace()
                    .chain(["inherit", "initial", "revert", "revert-layer", "unset"])
                    .filter(|v| matches(v))
                {
                    add(keyword, "CSS value", KindId::Keyword, keyword.into(), false);
                }
            }
        }
        items
    }
}
