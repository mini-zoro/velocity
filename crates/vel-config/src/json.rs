//! Lenient JSON with byte spans and lossless single-key edits.
use serde_json::Value;
use std::{collections::BTreeMap, ops::Range};

/// Parsed JSON value with source spans indexed by RFC 6901 JSON pointers.
#[derive(Clone, Debug)]
pub struct Document {
    source: String,
    trivia: String,
    /// Semantic value, without comments or trailing commas.
    pub value: Value,
    /// Spans include the value token, excluding surrounding trivia.
    pub spans: BTreeMap<String, Range<usize>>,
}
impl Document {
    /// Accepts a UTF-8 BOM, both JSON comment styles, and trailing commas.
    pub fn parse(source: &str) -> Result<Self, String> {
        let mut bytes = source.as_bytes().to_vec();
        let mut i = if source.starts_with('\u{feff}') {
            bytes[..3].fill(b' ');
            3
        } else {
            0
        };
        while i < bytes.len() {
            if bytes[i] == b'"' {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            } else if bytes.get(i..i + 2) == Some(b"//") {
                while i < bytes.len() && bytes[i] != b'\n' {
                    bytes[i] = b' ';
                    i += 1;
                }
            } else if bytes.get(i..i + 2) == Some(b"/*") {
                bytes[i..i + 2].fill(b' ');
                i += 2;
                loop {
                    if i + 1 >= bytes.len() {
                        return Err("unterminated block comment".into());
                    }
                    if bytes.get(i..i + 2) == Some(b"*/") {
                        bytes[i..i + 2].fill(b' ');
                        i += 2;
                        break;
                    }
                    if bytes[i] != b'\n' && bytes[i] != b'\r' {
                        bytes[i] = b' ';
                    }
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        let trivia = String::from_utf8(bytes.clone()).map_err(|e| e.to_string())?;
        // Strings are opaque, including commas and comment-looking text.
        i = 0;
        while i < bytes.len() {
            if bytes[i] == b'"' {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'"' {
                        break;
                    }
                    i += 1;
                }
            } else if bytes[i] == b',' {
                let next = bytes[i + 1..]
                    .iter()
                    .copied()
                    .find(|b| !b.is_ascii_whitespace());
                if matches!(next, Some(b'}' | b']')) {
                    bytes[i] = b' ';
                }
            }
            i += 1;
        }
        let clean = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        let value: Value = serde_json::from_str(&clean).map_err(|e| e.to_string())?;
        let mut spans = BTreeMap::new();
        scan(&clean, &mut 0, "", &mut spans)?;
        Ok(Self {
            source: source.into(),
            trivia,
            value,
            spans,
        })
    }
    /// Exact unmodified source, including BOM and trivia.
    pub fn source(&self) -> &str {
        &self.source
    }
    /// Replaces just a root setting's value, or inserts a missing setting.
    /// Existing comments, indentation, key spelling and unrelated values survive.
    pub fn set(&self, key: &str, value: &Value) -> Result<String, String> {
        let object = self.value.as_object().ok_or("settings must be an object")?;
        let token = serde_json::to_string(value).map_err(|e| e.to_string())?;
        let mut source = self.source.clone();
        if let Some(span) = self.spans.get(&format!("/{}", escape(key))) {
            source.replace_range(span.clone(), &token);
        } else {
            let end = self.spans[""].end - 1;
            // Detect a trailing comma in the original, after removing comments only.
            let last_value = self
                .spans
                .iter()
                .filter(|(p, _)| p.starts_with('/') && p.matches('/').count() == 1)
                .map(|(_, s)| s.end)
                .max();
            let comma =
                last_value.is_some_and(|at| self.trivia[at..end].trim_start().starts_with(','));
            let prefix = if !object.is_empty() && !comma {
                ","
            } else {
                ""
            };
            let multiline = self.source.contains('\n');
            let whitespace = if multiline { "\n    " } else { " " };
            source.insert_str(
                end,
                &format!(
                    "{prefix}{whitespace}{}: {token}{}",
                    serde_json::to_string(key).map_err(|e| e.to_string())?,
                    if multiline { "\n" } else { " " }
                ),
            );
        }
        // Return only valid edits; malformed comments must never corrupt a file.
        Self::parse(&source)?;
        Ok(source)
    }
}
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}
fn ws(s: &str, at: &mut usize) {
    while s.as_bytes().get(*at).is_some_and(u8::is_ascii_whitespace) {
        *at += 1;
    }
}
fn string_end(s: &str, at: &mut usize) {
    *at += 1;
    while *at < s.len() {
        match s.as_bytes()[*at] {
            b'\\' => *at += 2,
            b'"' => {
                *at += 1;
                return;
            }
            _ => *at += 1,
        }
    }
}
fn scan(
    s: &str,
    at: &mut usize,
    pointer: &str,
    spans: &mut BTreeMap<String, Range<usize>>,
) -> Result<(), String> {
    ws(s, at);
    let start = *at;
    match s.as_bytes()[*at] {
        b'{' => {
            *at += 1;
            ws(s, at);
            while s.as_bytes()[*at] != b'}' {
                let key_start = *at;
                string_end(s, at);
                let key: String =
                    serde_json::from_str(&s[key_start..*at]).map_err(|e| e.to_string())?;
                ws(s, at);
                *at += 1;
                scan(s, at, &format!("{pointer}/{}", escape(&key)), spans)?;
                ws(s, at);
                if s.as_bytes()[*at] == b',' {
                    *at += 1;
                    ws(s, at);
                }
            }
            *at += 1;
        }
        b'[' => {
            *at += 1;
            ws(s, at);
            let mut index = 0;
            while s.as_bytes()[*at] != b']' {
                scan(s, at, &format!("{pointer}/{index}"), spans)?;
                index += 1;
                ws(s, at);
                if s.as_bytes()[*at] == b',' {
                    *at += 1;
                    ws(s, at);
                }
            }
            *at += 1;
        }
        b'"' => string_end(s, at),
        _ => {
            while *at < s.len() && !b",]} \t\r\n".contains(&s.as_bytes()[*at]) {
                *at += 1;
            }
        }
    }
    spans.insert(pointer.into(), start..*at);
    Ok(())
}
