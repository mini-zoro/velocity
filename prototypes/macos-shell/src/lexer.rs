//! A deliberately small Rust-like, single-line lexer, independent of rendering.
use std::ops::Range;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Keyword,
    String,
    Comment,
    Number,
    Identifier,
}
impl Kind {
    pub fn color(self) -> [f32; 4] {
        match self {
            Self::Plain | Self::Identifier => [0.80, 0.83, 0.88, 1.],
            Self::Keyword => [0.74, 0.58, 0.98, 1.],
            Self::String => [0.57, 0.80, 0.61, 1.],
            Self::Comment => [0.43, 0.50, 0.56, 1.],
            Self::Number => [0.95, 0.69, 0.43, 1.],
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub bytes: Range<usize>,
    pub kind: Kind,
}
pub fn lex(s: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let start = i;
        let c = s[i..].chars().next().unwrap();
        let kind = if s[i..].starts_with("//") {
            i = s.len();
            Kind::Comment
        } else if c == '"' {
            i += 1;
            let mut escaped = false;
            while i < s.len() {
                let c = s[i..].chars().next().unwrap();
                i += c.len_utf8();
                if c == '"' && !escaped {
                    break;
                }
                if c == '\\' {
                    escaped = !escaped;
                } else {
                    escaped = false;
                }
            }
            Kind::String
        } else if c.is_ascii_digit() {
            i += 1;
            while i < s.len()
                && (s.as_bytes()[i].is_ascii_alphanumeric() || b"_.".contains(&s.as_bytes()[i]))
            {
                i += 1;
            }
            Kind::Number
        } else if c.is_alphabetic() || c == '_' {
            i += c.len_utf8();
            while i < s.len() {
                let c = s[i..].chars().next().unwrap();
                if !c.is_alphanumeric() && c != '_' {
                    break;
                }
                i += c.len_utf8();
            }
            match &s[start..i] {
                "fn" | "let" | "mut" | "pub" | "struct" | "enum" | "impl" | "use" | "mod"
                | "if" | "else" | "for" | "while" | "loop" | "match" | "return" | "const"
                | "static" | "true" | "false" | "self" | "Self" | "async" | "await" | "move"
                | "unsafe" | "trait" | "where" | "type" | "in" => Kind::Keyword,
                _ => Kind::Identifier,
            }
        } else {
            i += c.len_utf8();
            Kind::Plain
        };
        out.push(Token {
            bytes: start..i,
            kind,
        });
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strings_contain_comments_and_escaped_quotes() {
        let s = r#"let 中文 = "a\"//b"; // 你好"#;
        let tokens = lex(s);
        assert_eq!(tokens.iter().filter(|t| t.kind == Kind::String).count(), 1);
        assert_eq!(&s[tokens.last().unwrap().bytes.clone()], "// 你好");
        assert!(tokens.iter().any(|t| t.kind == Kind::Keyword));
    }
    #[test]
    fn tokens_cover_utf8_without_gaps() {
        for s in ["", "🦀 中文 é", "0xff + 1_000", "\"unfinished"] {
            let tokens = lex(s);
            let mut next = 0;
            for t in tokens {
                assert_eq!(t.bytes.start, next);
                assert!(s.is_char_boundary(t.bytes.start) && s.is_char_boundary(t.bytes.end));
                next = t.bytes.end;
            }
            assert_eq!(next, s.len());
        }
    }
}
