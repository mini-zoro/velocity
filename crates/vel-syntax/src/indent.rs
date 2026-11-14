//! Metadata-driven reindentation. Coordinates and output are UTF-8; tab stops are explicit.
use crate::{
    MetadataSet, ScopeStack, ScopedSpan, SyntaxReference, SyntaxSet, parser::InterpreterState,
};

/// Indentation style used by the reindent command.
#[derive(Debug, Clone, Copy)]
pub struct IndentOptions {
    /// Visual columns per indent step and tab stop (at least one).
    pub tab_size: usize,
    /// Emit spaces instead of tabs.
    pub spaces: bool,
}
impl Default for IndentOptions {
    fn default() -> Self {
        Self {
            tab_size: 4,
            spaces: true,
        }
    }
}
/// Counts only leading spaces/tabs; tabs advance to the next stop.
pub fn indentation(line: &str, tab_size: usize) -> usize {
    let size = tab_size.max(1);
    line.chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .fold(0, |n, c| {
            if c == '\t' {
                n + size - n % size
            } else {
                n + 1
            }
        })
}
/// Parses a complete document, applying branch corrections before returning scopes.
pub fn scoped_spans(
    set: &SyntaxSet,
    syntax: &SyntaxReference,
    text: &str,
) -> Result<Vec<ScopedSpan>, String> {
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    let mut ops = vec![Vec::new(); lines.len()];
    let mut state = InterpreterState::new(syntax);
    for (row, line) in lines.iter().enumerate() {
        let output = state.parse_line(line, set).map_err(|e| e.to_string())?;
        ops[row] = output.ops;
        for (n, corrected) in output.replayed {
            if let Some(old) = n.checked_sub(1).and_then(|n| ops.get_mut(n)) {
                *old = corrected;
            }
        }
    }
    let mut stack = ScopeStack::new();
    let mut out = Vec::new();
    let mut offset = 0;
    for (line, ops) in lines.iter().zip(ops) {
        let mut at = 0;
        for (byte, op) in ops {
            if byte > at {
                out.push(ScopedSpan {
                    range: offset + at..offset + byte,
                    scopes: stack.as_slice().to_vec(),
                });
            }
            stack.apply(&op).map_err(|e| e.to_string())?;
            at = byte;
        }
        if at < line.len() {
            out.push(ScopedSpan {
                range: offset + at..offset + line.len(),
                scopes: stack.as_slice().to_vec(),
            });
        }
        offset += line.len();
    }
    Ok(out)
}
/// Reindents complete lines using scope-selected patterns and lexical bracket balancing.
/// Existing whitespace is used only by preserveIndent/unIndentedLinePattern or indentation-based
/// languages with no decrease pattern (where a dedent cannot be inferred from tokens).
pub fn reindent(
    text: &str,
    spans: &[ScopedSpan],
    metadata: &MetadataSet,
    options: IndentOptions,
) -> String {
    let size = options.tab_size.max(1);
    let mut out = String::with_capacity(text.len());
    let mut offset = 0;
    let mut next = 0usize;
    let mut temporary = None;
    let mut brackets: Vec<(char, usize, bool)> = Vec::new();
    let mut case_bases = Vec::new();
    let mut span_index = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_start_matches([' ', '\t']);
        let leading = line.len() - body.len();
        let old_indent = indentation(line, size);
        let first = offset + leading;
        while spans.get(span_index).is_some_and(|s| s.range.end <= first) {
            span_index += 1;
        }
        let scopes = spans
            .get(span_index)
            .map_or(&[][..], |s| s.scopes.as_slice());
        let mut code = line.as_bytes().to_vec();
        let mut candidates = Vec::new();
        let mut local = span_index;
        while let Some(span) = spans
            .get(local)
            .filter(|s| s.range.start < offset + line.len())
        {
            let a = span.range.start.max(offset) - offset;
            let b = span.range.end.min(offset + line.len()) - offset;
            let is_comment = span
                .scopes
                .iter()
                .any(|s| s.build_string().starts_with("comment"));
            if is_comment {
                for byte in &mut code[a..b] {
                    if *byte != b'\n' && *byte != b'\r' {
                        *byte = b' ';
                    }
                }
            } else if !line[a..b].trim().is_empty() && !candidates.contains(&span.scopes.as_slice())
            {
                candidates.push(span.scopes.as_slice());
            }
            local += 1;
        }
        let code = String::from_utf8(code).expect("comment spans cover complete UTF-8 characters");
        let comment = code.trim().is_empty() && !body.trim().is_empty();
        if candidates.is_empty() {
            candidates.push(scopes);
        }
        // Preservation applies to the line's leading scope. A heredoc delimiter
        // later in a shell statement must not suppress that statement's block rule.
        let leading_metadata = metadata.for_stacks(&[candidates[0]]);
        // Leading whitespace carries the context before a closing token pops it.
        // Include that context when selecting increase/decrease patterns (e.g. a
        // shell clause terminator is still indented with its clause body).
        if leading > 0
            && scopes.iter().any(|scope| {
                scope
                    .build_string()
                    .starts_with("punctuation.terminator.clause")
            })
            && let Some(span) = spans
                .iter()
                .take(span_index + 1)
                .rfind(|span| span.range.start <= offset && offset < span.range.end)
        {
            candidates.push(&span.scopes);
        }
        let m = metadata.for_stacks(&candidates);
        let blank = body.trim().is_empty();
        let sql_comment = comment
            && scopes
                .iter()
                .any(|s| s.build_string().starts_with("source.sql"));
        let preserve = leading_metadata.boolean("preserveIndent").unwrap_or(false)
            || (comment && !sql_comment);
        let excluded = metadata.matches_pattern(&leading_metadata, "unIndentedLinePattern", &code);
        let mut balanced = code.as_bytes().to_vec();
        let mut pairs = Vec::new();
        for (i, c) in code.char_indices() {
            if c == '(' {
                pairs.push(i);
            } else if c == ')'
                && let Some(open) = pairs.pop()
            {
                balanced[open] = b' ';
                balanced[i] = b' ';
            }
        }
        let balanced = String::from_utf8(balanced).expect("only ASCII parentheses replaced");
        let decrease = metadata.matches_pattern(&m, "decreaseIndentPattern", &balanced);
        let increase = metadata.matches_pattern(&m, "increaseIndentPattern", code.trim_end());
        let one_line =
            metadata.matches_pattern(&m, "bracketIndentNextLinePattern", code.trim_end());
        let disable = metadata.matches_pattern(&m, "disableIndentNextLinePattern", code.trim_end());
        let closing = code
            .trim_start()
            .chars()
            .next()
            .filter(|c| matches!(c, ')' | ']' | '}'));
        let closing_bracket = closing.and_then(|ch| {
            brackets
                .iter()
                .rposition(|(open, _, _)| {
                    matches!((*open, ch), ('(', ')') | ('[', ']') | ('{', '}'))
                })
                .map(|i| {
                    if ch == ')' {
                        old_indent.clamp(brackets[i].1, brackets[i].1 + size)
                    } else {
                        brackets[i].1
                    }
                })
        });
        let mut indent = if preserve || excluded {
            old_indent
        } else if blank {
            0
        } else {
            closing_bracket.unwrap_or_else(|| {
                if decrease {
                    next.saturating_sub(size)
                } else {
                    next
                }
            })
        };
        if !preserve && !excluded && code.trim() == "?>" {
            indent = next;
        }
        let shell = scopes
            .iter()
            .any(|scope| scope.build_string().starts_with("source.shell"));
        let keyword = code.split_whitespace().next().unwrap_or("");
        if !preserve && !excluded && shell && keyword == "esac" {
            indent = case_bases
                .pop()
                .unwrap_or_else(|| indent.saturating_sub(size));
        }
        if !preserve && !excluded && body.starts_with('{') && temporary.is_some() {
            indent = temporary.take().unwrap();
        }
        // A language whose blocks are defined by whitespace must retain explicit dedents.
        if !preserve
            && !excluded
            && !blank
            && scopes.iter().any(|s| s.build_string() == "source.python")
            && old_indent < indent
            && closing.is_none()
        {
            indent = old_indent;
        }
        if blank {
            out.push_str(line);
        } else {
            if options.spaces {
                out.extend(std::iter::repeat_n(' ', indent));
            } else {
                out.extend(std::iter::repeat_n('\t', indent / size));
                out.extend(std::iter::repeat_n(' ', indent % size));
            }
            out.push_str(body);
        }
        if !blank && !excluded && !preserve {
            let before = brackets.len();
            let mut closed_condition = None;
            let mut condition_end = None;
            let mut closed_base = None;
            let mut auto_open = false;
            let mut local_index = span_index;
            for (at, ch) in body.char_indices() {
                let byte = first + at;
                while spans.get(local_index).is_some_and(|s| s.range.end <= byte) {
                    local_index += 1;
                }
                let ignored = spans.get(local_index).is_some_and(|span| {
                    span.scopes.iter().any(|s| {
                        let name = s.build_string();
                        name.starts_with("comment") || name.starts_with("string")
                    })
                });
                if ignored {
                    continue;
                }
                match ch {
                    '(' | '[' | '{' => {
                        let automatic = match ch {
                            '(' => m.boolean("indentParens").unwrap_or(true),
                            '[' => m.boolean("indentSquareBrackets").unwrap_or(false),
                            _ => true,
                        };
                        auto_open |= automatic;
                        brackets.push((
                            ch,
                            if ch == '{' {
                                closed_condition.or(closed_base).unwrap_or(indent)
                            } else {
                                indent
                            },
                            one_line && ch == '(',
                        ));
                    }
                    ')' | ']' | '}' => {
                        if let Some(i) = brackets.iter().rposition(|(open, _, _)| {
                            matches!((*open, ch), ('(', ')') | ('[', ']') | ('{', '}'))
                        }) {
                            closed_base = Some(
                                closed_base
                                    .map_or(brackets[i].1, |old: usize| old.min(brackets[i].1)),
                            );
                            if brackets[i].2 {
                                closed_condition = Some(brackets[i].1);
                                condition_end = Some(leading + at + ch.len_utf8());
                            }
                            brackets.truncate(i);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(base) = temporary.take() {
                next = base;
            } else {
                next = indent;
            }
            if increase || (brackets.len() > before && auto_open) {
                next = if brackets.len() > before
                    || (auto_open
                        && closed_base.is_some_and(|base| {
                            brackets.last().is_some_and(|(_, saved, _)| *saved == base)
                        }))
                {
                    brackets
                        .last()
                        .map_or(indent + size, |(_, base, _)| base + size)
                } else {
                    indent + size
                };
            }
            if brackets.len() > before && body.starts_with("extern ") {
                brackets.truncate(before);
                next = indent;
            } else if brackets.len() < before {
                next = closed_base.unwrap_or(indent);
            }
            let needs_body = condition_end.is_none_or(|end| code[end..].trim().is_empty());
            if let Some(base) = closed_condition
                && needs_body
            {
                temporary = Some(base);
                next = base + size;
            }
            if one_line && needs_body && !disable && !increase && brackets.len() == before {
                temporary = Some(indent);
                next = indent + size;
            }
        }
        if !preserve && !excluded && matches!(code.trim(), "<?php" | "<?") {
            next = indent;
        }
        if !preserve
            && !excluded
            && shell
            && keyword == "case"
            && !code.split_whitespace().any(|word| word == "esac")
        {
            case_bases.push(indent);
        }
        if !preserve
            && !excluded
            && scopes
                .iter()
                .any(|s| s.build_string().starts_with("source.shell"))
            && code.split_whitespace().any(|s| s == "esac")
        {
            next = indent;
        }
        offset += line.len();
    }
    out
}
