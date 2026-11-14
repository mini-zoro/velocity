use crate::{Error, Result, transform::Transform};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    Field(u32),
    Variable(String),
}
#[derive(Debug, Clone)]
pub(crate) enum Node {
    Text(String),
    Value(Key, Option<Vec<Node>>, Option<Transform>),
}

pub(crate) fn parse(source: &str) -> Result<Vec<Node>> {
    let mut parser = Parser {
        source,
        at: 0,
        depth: 0,
    };
    parser.nodes(false)
}
struct Parser<'a> {
    source: &'a str,
    at: usize,
    depth: usize,
}
impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.source[self.at..].chars().next()
    }
    fn take(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += c.len_utf8();
        Some(c)
    }
    fn nodes(&mut self, nested: bool) -> Result<Vec<Node>> {
        self.depth += 1;
        if self.depth > 128 {
            return Err(Error::Parse("snippet nesting exceeds 128".into()));
        }
        let mut nodes = Vec::new();
        let mut text = String::new();
        while let Some(c) = self.take() {
            if nested && c == '}' {
                self.depth -= 1;
                if !text.is_empty() {
                    nodes.push(Node::Text(text));
                }
                return Ok(nodes);
            }
            if c == '\\' {
                if self
                    .peek()
                    .is_some_and(|c| matches!(c, '$' | '}' | '\\' | '`'))
                {
                    text.push(self.take().unwrap());
                } else {
                    text.push(c);
                }
            } else if c == '$' {
                let braced = self.peek() == Some('{');
                if braced {
                    self.take();
                }
                let start = self.at;
                let numeric = self.peek().is_some_and(|c| c.is_ascii_digit());
                while self.peek().is_some_and(|c| {
                    if numeric {
                        c.is_ascii_digit()
                    } else {
                        c.is_ascii_alphanumeric() || c == '_'
                    }
                }) {
                    self.take();
                }
                if start == self.at {
                    if braced {
                        return Err(Error::Parse("empty placeholder".into()));
                    }
                    text.push('$');
                    continue;
                }
                let name = &self.source[start..self.at];
                let key = if name.bytes().all(|b| b.is_ascii_digit()) {
                    Key::Field(
                        name.parse()
                            .map_err(|_| Error::Parse("field number overflow".into()))?,
                    )
                } else {
                    Key::Variable(name.into())
                };
                if !text.is_empty() {
                    nodes.push(Node::Text(std::mem::take(&mut text)));
                }
                let mut default = None;
                let mut transform = None;
                if braced {
                    match self.take() {
                        Some('}') => (),
                        Some(':') => default = Some(self.nodes(true)?),
                        Some('/') => {
                            let pattern = self.delimited()?;
                            let format = self.delimited()?;
                            let start = self.at;
                            while self.peek().is_some_and(|c| c != '}') {
                                self.take();
                            }
                            let options = &self.source[start..self.at];
                            if self.take() != Some('}') {
                                return Err(Error::Parse("unterminated transformation".into()));
                            }
                            transform = Some(Transform::new(&pattern, format, options)?);
                        }
                        _ => return Err(Error::Parse("invalid placeholder suffix".into())),
                    }
                }
                nodes.push(Node::Value(key, default, transform));
            } else {
                text.push(c);
            }
        }
        self.depth -= 1;
        if nested {
            return Err(Error::Parse("unterminated placeholder".into()));
        }
        if !text.is_empty() {
            nodes.push(Node::Text(text));
        }
        Ok(nodes)
    }
    fn delimited(&mut self) -> Result<String> {
        let mut out = String::new();
        while let Some(c) = self.take() {
            if c == '/' {
                return Ok(out);
            }
            if c == '\\' && self.peek() == Some('/') {
                out.push(self.take().unwrap());
            } else {
                out.push(c);
                if c == '\\'
                    && let Some(next) = self.take()
                {
                    out.push(next);
                }
            }
        }
        Err(Error::Parse("unterminated transformation segment".into()))
    }
}
