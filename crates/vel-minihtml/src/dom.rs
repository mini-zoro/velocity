//! Bounded, forgiving fragment parser; script-like content is inert.
use crate::{
    Error,
    css::{Declaration, Rule, parse_declarations, parse_stylesheet},
};
use std::collections::BTreeMap;

/// Documented minihtml tags (plus span, br and img from the engine's subset contract).
pub const TAGS: &[&str] = &[
    "html", "head", "style", "body", "h1", "h2", "h3", "h4", "h5", "h6", "div", "p", "ul", "ol",
    "li", "b", "strong", "i", "em", "u", "big", "small", "a", "code", "var", "tt", "span", "br",
    "img",
];
#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub tag: String,
    pub attrs: BTreeMap<String, String>,
    pub declarations: Vec<Declaration>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub text: String,
}
impl Node {
    fn element(tag: &str, parent: Option<usize>) -> Self {
        Self {
            tag: tag.into(),
            attrs: BTreeMap::new(),
            declarations: Vec::new(),
            parent,
            children: Vec::new(),
            text: String::new(),
        }
    }
}
/// Parsed markup and styles. Unknown tags are transparent inline elements; scripts are hidden.
#[derive(Debug, Clone)]
pub struct Document {
    pub(crate) nodes: Vec<Node>,
    pub(crate) rules: Vec<Rule>,
    pub(crate) inline: bool,
}
impl Document {
    /// Parses a full document or fragment, creating implicit html/body containers as needed.
    pub fn parse(html: &str) -> Result<Self, Error> {
        Self::parse_mode(html, false)
    }
    /// Parses completion/quick-panel details: only a, b, strong, i, em, u, tt and code format text.
    /// Other tags are transparent; style/head/script content is discarded.
    pub fn parse_inline(html: &str) -> Result<Self, Error> {
        Self::parse_mode(html, true)
    }
    /// DOM size, useful for performance diagnostics.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    fn parse_mode(html: &str, inline: bool) -> Result<Self, Error> {
        if html.len() > 4 * 1024 * 1024 {
            return Err(Error::Limit("4 MiB document"));
        }
        let mut doc = Self {
            nodes: vec![Node::element("html", None), Node::element("body", Some(0))],
            rules: Vec::new(),
            inline,
        };
        doc.nodes[0].children.push(1);
        let mut stack = vec![0, 1];
        let mut rest = html;
        while !rest.is_empty() {
            if doc.nodes.len() >= 100_000 {
                return Err(Error::Limit("100000 nodes"));
            }
            if stack.len() > 256 {
                return Err(Error::Limit("256 nested elements"));
            }
            if rest.starts_with("<!--") {
                rest = rest.find("-->").map_or("", |at| &rest[at + 3..]);
                continue;
            }
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                let parent = *stack.last().unwrap();
                let mut node = Node::element("", Some(parent));
                node.text = html_escape::decode_html_entities(&rest[..end]).into_owned();
                let id = doc.nodes.len();
                doc.nodes.push(node);
                doc.nodes[parent].children.push(id);
                rest = &rest[end..];
                continue;
            }
            let Some(end) = tag_end(rest) else {
                let parent = *stack.last().unwrap();
                let mut node = Node::element("", Some(parent));
                node.text = rest.into();
                let id = doc.nodes.len();
                doc.nodes.push(node);
                doc.nodes[parent].children.push(id);
                break;
            };
            let token = rest[1..end].trim();
            rest = &rest[end + 1..];
            if token.starts_with(['!', '?']) {
                continue;
            }
            if let Some(close) = token.strip_prefix('/') {
                let close = close.trim().to_ascii_lowercase();
                if let Some(at) = stack.iter().rposition(|id| doc.nodes[*id].tag == close) {
                    stack.truncate(at.max(1));
                }
                if stack.len() == 1 {
                    stack.push(1);
                }
                continue;
            }
            let at = token
                .find(|c: char| c.is_whitespace() || c == '/')
                .unwrap_or(token.len());
            let tag = token[..at].to_ascii_lowercase();
            if tag.is_empty() {
                continue;
            }
            let attrs = attributes(&token[at..]);
            if ["style", "script"].contains(&tag.as_str()) || (inline && tag == "head") {
                let closing = format!("</{tag}");
                if let Some(end) = find_ascii_case(rest, &closing) {
                    if tag == "style" && !inline {
                        doc.rules.extend(parse_stylesheet(&rest[..end]));
                    }
                    rest = rest[end..].find('>').map_or("", |at| &rest[end + at + 1..]);
                } else {
                    rest = "";
                }
                continue;
            }
            if inline && !["a", "b", "strong", "i", "em", "u", "tt", "code"].contains(&tag.as_str())
            {
                continue;
            }
            if tag == "html" {
                doc.nodes[0].attrs.extend(attrs);
                continue;
            }
            if tag == "body" {
                doc.nodes[1].attrs.extend(attrs);
                stack = vec![0, 1];
                doc.nodes[1].declarations = doc.nodes[1]
                    .attrs
                    .get("style")
                    .map(|s| parse_declarations(s))
                    .unwrap_or_default();
                continue;
            }
            if tag == "li"
                && let Some(at) = stack.iter().rposition(|id| doc.nodes[*id].tag == "li")
            {
                // Only close a sibling item, not an outer item containing a nested list.
                if !stack[at + 1..]
                    .iter()
                    .any(|id| ["ul", "ol"].contains(&doc.nodes[*id].tag.as_str()))
                {
                    stack.truncate(at);
                }
            }
            if ["p", "div", "ul", "ol", "h1", "h2", "h3", "h4", "h5", "h6"].contains(&tag.as_str())
                && doc.nodes[*stack.last().unwrap()].tag == "p"
            {
                stack.pop();
            }
            let parent = if tag == "head" {
                0
            } else {
                *stack.last().unwrap()
            };
            let mut node = Node::element(&tag, Some(parent));
            node.declarations = if inline {
                Vec::new()
            } else {
                attrs
                    .get("style")
                    .map(|s| parse_declarations(s))
                    .unwrap_or_default()
            };
            node.attrs = attrs;
            let id = doc.nodes.len();
            doc.nodes.push(node);
            doc.nodes[parent].children.push(id);
            if !token.ends_with('/')
                && !["br", "img", "hr", "input", "meta", "link"].contains(&tag.as_str())
            {
                stack.push(id);
            }
        }
        doc.nodes[0].declarations = doc.nodes[0]
            .attrs
            .get("style")
            .map(|s| parse_declarations(s))
            .unwrap_or_default();
        Ok(doc)
    }
}
fn find_ascii_case(text: &str, needle: &str) -> Option<usize> {
    text.as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}
fn tag_end(text: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in text.char_indices().skip(1) {
        if quote == Some(c) {
            quote = None;
        } else if quote.is_none() {
            if c == '\'' || c == '"' {
                quote = Some(c);
            } else if c == '>' {
                return Some(i);
            }
        }
    }
    None
}
fn attributes(mut text: &str) -> BTreeMap<String, String> {
    let mut attrs = BTreeMap::new();
    while !text.trim_start().is_empty() {
        text = text.trim_start();
        if text == "/" {
            break;
        }
        let end = text
            .find(|c: char| c.is_whitespace() || c == '=' || c == '/')
            .unwrap_or(text.len());
        if end == 0 {
            text = &text[1..];
            continue;
        }
        let name = text[..end].to_ascii_lowercase();
        text = text[end..].trim_start();
        let mut value = "";
        if let Some(tail) = text.strip_prefix('=') {
            text = tail.trim_start();
            if text.starts_with(['\'', '"']) {
                let quote = text.as_bytes()[0] as char;
                text = &text[1..];
                let end = text.find(quote).unwrap_or(text.len());
                value = &text[..end];
                text = text.get(end + 1..).unwrap_or("");
            } else {
                let end = text.find(char::is_whitespace).unwrap_or(text.len());
                value = &text[..end];
                text = &text[end..];
            }
        }
        if [
            "class", "id", "style", "href", "title", "src", "width", "height", "alt",
        ]
        .contains(&name.as_str())
        {
            attrs
                .entry(name)
                .or_insert_with(|| html_escape::decode_html_entities(value).into_owned());
        }
    }
    attrs
}
