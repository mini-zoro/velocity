//! Goto Anything's file, location, symbol and fuzzy-word suffixes.

/// Destination inside the selected file (one-based scalar coordinates).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Start of file, or keep the existing selection.
    File,
    /// One-based row and column; zero is normalized to one.
    Line(usize, usize),
    /// Symbol-provider hook; no syntax dependency is required.
    Symbol(String),
    /// Fuzzy words in the current/selected file.
    Word(String),
}
/// Parsed query. An empty file restricts navigation to the current document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// Fuzzy file component.
    pub file: String,
    /// In-file destination.
    pub target: Target,
}
impl Query {
    /// Parses the public Goto Anything vocabulary without interpreting a drive colon.
    pub fn parse(input: &str) -> Self {
        for (marker, symbol) in [('@', true), ('#', false)] {
            if let Some((file, term)) = input.split_once(marker) {
                return Self {
                    file: file.into(),
                    target: if symbol {
                        Target::Symbol(term.into())
                    } else {
                        Target::Word(term.into())
                    },
                };
            }
        }
        if let Some((prefix, last)) = input.rsplit_once(':')
            && let Ok(last) = last.parse::<usize>()
        {
            let (file, row, column) = if let Some((file, row)) = prefix.rsplit_once(':')
                && let Ok(row) = row.parse::<usize>()
            {
                (file, row.max(1), last.max(1))
            } else {
                (prefix, last.max(1), 1)
            };
            return Self {
                file: file.into(),
                target: Target::Line(row, column),
            };
        }
        Self {
            file: input.into(),
            target: Target::File,
        }
    }
}

/// Syntax lane seam. Offsets are UTF-8 byte positions in the provided snapshot.
pub trait Symbols: Send + Sync + std::fmt::Debug {
    /// Returns symbol captions and byte positions in source order.
    fn symbols(&self, text: &vel_text::Text) -> Vec<(String, usize)>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grammar_and_colons() {
        for (query, file, target) in [
            ("main.rs", "main.rs", Target::File),
            ("main:12", "main", Target::Line(12, 1)),
            ("src/main.rs:12:4", "src/main.rs", Target::Line(12, 4)),
            (":0", "", Target::Line(1, 1)),
            ("@parse", "", Target::Symbol("parse".into())),
            ("lib@parse", "lib", Target::Symbol("parse".into())),
            ("#word", "", Target::Word("word".into())),
            ("C:\\src\\main.rs:9", "C:\\src\\main.rs", Target::Line(9, 1)),
            ("name:part", "name:part", Target::File),
        ] {
            assert_eq!(
                Query::parse(query),
                Query {
                    file: file.into(),
                    target
                }
            );
        }
    }
}
