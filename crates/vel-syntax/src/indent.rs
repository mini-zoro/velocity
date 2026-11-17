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
/// Whether whitespace belongs to string content or a heredoc, including its terminator.
/// Callers may restrict the stack to an embedded language when reindenting its code.
pub fn is_string_or_heredoc(scopes: &[crate::Scope]) -> bool {
    scopes.iter().any(|scope| {
        let name = scope.build_string();
        name.starts_with("string") || name.contains("heredoc")
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
/// Literal leading whitespace is kept byte-for-byte. Otherwise existing whitespace is used by
/// preserveIndent/unIndentedLinePattern and indentation-based languages with no decrease pattern
/// (where a dedent cannot be inferred from tokens).
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
    // A shell case can own a brace (case x { ... }); braces in its branch cannot end it.
    let mut case_bases: Vec<(usize, Option<usize>)> = Vec::new();
    let mut span_index = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_start_matches([' ', '\t']);
        let leading = line.len() - body.len();
        let old_indent = indentation(line, size);
        let first = offset + leading;
        while spans.get(span_index).is_some_and(|s| s.range.end <= offset) {
            span_index += 1;
        }
        let leading_span_index = span_index;
        while spans.get(span_index).is_some_and(|s| s.range.end <= first) {
            span_index += 1;
        }
        let raw_scopes = spans
            .get(span_index)
            .map_or(&[][..], |s| s.scopes.as_slice());
        let scopes = language_scopes(raw_scopes);
        // These bytes are literal content, independent of the requested indentation
        // style. In particular <<- strips tabs, not an equivalent width of spaces.
        let continued_literal = spans
            .get(leading_span_index)
            .is_some_and(|s| s.range.start < offset)
            || leading_span_index
                .checked_sub(1)
                .and_then(|i| spans.get(i))
                .is_some_and(|s| is_string_or_heredoc(language_scopes(&s.scopes)));
        let preserve_whitespace = spans
            .iter()
            .skip(leading_span_index)
            .take_while(|s| s.range.start < first.max(offset + 1))
            .any(|s| {
                is_string_or_heredoc(language_scopes(&s.scopes))
                    && (leading > 0 || continued_literal)
            });
        let mut code = line.as_bytes().to_vec();
        let mut lexical = code.clone();
        let mut candidates = Vec::new();
        let mut local = span_index;
        while let Some(span) = spans
            .get(local)
            .filter(|s| s.range.start < offset + line.len())
        {
            let a = span.range.start.max(offset) - offset;
            let b = span.range.end.min(offset + line.len()) - offset;
            let span_scopes = language_scopes(&span.scopes);
            let is_comment = span_scopes
                .iter()
                .any(|s| s.build_string().starts_with("comment"));
            let is_string = span_scopes
                .iter()
                .any(|s| s.build_string().starts_with("string"));
            let delimiter = span_scopes.iter().any(|s| {
                s.build_string()
                    .starts_with("punctuation.definition.string")
            });
            if is_comment || (is_string && !delimiter) {
                for byte in &mut lexical[a..b] {
                    if *byte != b'\n' && *byte != b'\r' {
                        *byte = b' ';
                    }
                }
            }
            if is_comment {
                for byte in &mut code[a..b] {
                    if *byte != b'\n' && *byte != b'\r' {
                        *byte = b' ';
                    }
                }
            } else if !line[a..b].trim().is_empty() && !candidates.contains(&span_scopes) {
                candidates.push(span_scopes);
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
        let starts_comment = scopes
            .iter()
            .any(|s| s.build_string().starts_with("comment"));
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
        let m = if leading_metadata
            .boolean("velocityIndentUseLeadingScope")
            .unwrap_or(false)
        {
            leading_metadata.clone()
        } else {
            metadata.for_stacks(&candidates)
        };
        let atomic_scope = m.string("velocityIndentAtomicScope");
        if let Some(prefix) = atomic_scope {
            for span in spans
                .iter()
                .skip(span_index)
                .take_while(|s| s.range.start < offset + line.len())
                .filter(|s| {
                    language_scopes(&s.scopes)
                        .iter()
                        .any(|scope| scope.build_string().starts_with(prefix))
                })
            {
                let a = span.range.start.max(offset) - offset;
                let b = span.range.end.min(offset + line.len()) - offset;
                for byte in &mut lexical[a..b] {
                    if !matches!(*byte, b'\n' | b'\r') {
                        *byte = b' ';
                    }
                }
            }
        }
        if let Some(ignore) = m.string("velocityIndentIgnoreScope") {
            let baseline = raw_scopes
                .iter()
                .filter(|s| s.build_string().starts_with(ignore))
                .count();
            for span in spans
                .iter()
                .skip(span_index)
                .take_while(|s| s.range.start < offset + line.len())
            {
                if span
                    .scopes
                    .iter()
                    .filter(|s| s.build_string().starts_with(ignore))
                    .count()
                    > baseline
                {
                    let a = span.range.start.max(offset) - offset;
                    let b = span.range.end.min(offset + line.len()) - offset;
                    for byte in &mut lexical[a..b] {
                        if *byte != b'\n' && *byte != b'\r' {
                            *byte = b' ';
                        }
                    }
                }
            }
        }
        let lexical = String::from_utf8(lexical).expect("scope spans cover UTF-8 characters");
        let blank = body.trim().is_empty();
        let preserve = (starts_comment
            && leading_metadata
                .boolean("velocityPreserveLeadingComments")
                .unwrap_or(false))
            || leading_metadata.boolean("preserveIndent").unwrap_or(false)
            || (comment
                && !leading_metadata
                    .boolean("velocityReindentComments")
                    .unwrap_or(false));
        let excluded =
            metadata.matches_pattern(&leading_metadata, "unIndentedLinePattern", &lexical);
        let mut balanced = lexical.as_bytes().to_vec();
        let mut pairs = Vec::new();
        for (i, c) in lexical.char_indices() {
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
        let decrease = metadata.matches_pattern(&m, "decreaseIndentPattern", &balanced)
            || metadata.matches_pattern(&m, "velocityDecreaseIndentPattern", &lexical);
        let increase = (metadata.matches_pattern(&m, "increaseIndentPattern", lexical.trim_end())
            || metadata.matches_pattern(&m, "velocityIncreaseIndentPattern", &lexical))
            && !metadata.matches_pattern(&m, "velocityDisableIncreaseIndentPattern", &lexical);
        let one_line =
            metadata.matches_pattern(&m, "bracketIndentNextLinePattern", lexical.trim_end());
        let disable =
            metadata.matches_pattern(&m, "disableIndentNextLinePattern", lexical.trim_end());
        let closing = lexical
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
        if !preserve
            && !excluded
            && metadata.matches_pattern(&m, "velocityKeepIndentPattern", &code)
        {
            indent = next;
        }
        let block_start = metadata.matches_pattern(&m, "velocityIndentBlockStartPattern", &lexical);
        let inline_end = block_start
            && metadata.matches_pattern(&m, "velocityIndentBlockInlineEndPattern", &lexical);
        // A case completed on this line is one statement in the enclosing block;
        // its final esac must not apply the generic decrease/increase rules to that block.
        if !preserve && !excluded && inline_end {
            indent = next;
        }
        let block_end = metadata.matches_pattern(&m, "velocityIndentBlockEndPattern", &code)
            && (closing != Some('}')
                || case_bases.last().is_some_and(|(_, owned)| {
                    owned.is_some() && *owned == brackets.iter().rposition(|(ch, _, _)| *ch == '{')
                }));
        if !preserve
            && !excluded
            && block_end
            && let Some((base, _)) = case_bases.pop()
        {
            indent = base;
        }
        if !preserve && !excluded && body.starts_with('{') && temporary.is_some() {
            indent = temporary.take().unwrap();
        }
        // A language whose blocks are defined by whitespace must retain explicit dedents.
        if !preserve
            && !excluded
            && !blank
            && m.boolean("velocityIndentationBlocks").unwrap_or(false)
            && old_indent < indent
            && closing.is_none()
        {
            indent = old_indent;
        }
        if blank || preserve_whitespace {
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
        let bracket_depth_before = brackets.len();
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
                    language_scopes(&span.scopes).iter().any(|s| {
                        let name = s.build_string();
                        name.starts_with("comment")
                            || name.starts_with("string")
                            || atomic_scope.is_some_and(|prefix| name.starts_with(prefix))
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
            if brackets.len() > before
                && metadata.matches_pattern(&m, "velocityNoBracketIndentPattern", &code)
            {
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
        if !preserve
            && !excluded
            && metadata.matches_pattern(&m, "velocityResetIndentPattern", &code)
        {
            next = indent;
        }
        // A balanced brace-style case on one line has already ended. Parameter
        // expansions on a normal `case ... in` line cannot satisfy this ending.
        let inline_braced_block = block_start
            && lexical.trim_end().ends_with('}')
            && lexical.contains('{')
            && brackets.len() == bracket_depth_before;
        let inline_end = inline_end || inline_braced_block;
        if !preserve && !excluded && block_start && !inline_end {
            let owned_brace = brackets
                .iter()
                .rposition(|(ch, _, _)| *ch == '{')
                .filter(|index| *index >= bracket_depth_before);
            case_bases.push((indent, owned_brace));
        }
        if !preserve && !excluded && (block_end || block_start && inline_end) {
            next = indent;
        }
        offset += line.len();
    }
    out
}

/// An embedded syntax is code even when its host is a template string or comment.
/// Only lexical scopes within the innermost language may suppress its brackets.
fn language_scopes(scopes: &[crate::Scope]) -> &[crate::Scope] {
    let root = scopes
        .iter()
        .rposition(|scope| {
            let name = scope.build_string();
            name.starts_with("source.") || name.starts_with("text.")
        })
        .unwrap_or(0);
    &scopes[root..]
}
