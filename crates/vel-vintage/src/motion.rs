use std::ops::Range;
use vel_buffer::{Region, VerticalLayout};
use vel_text::Text;

pub(crate) fn end(text: &Text, point: usize) -> usize {
    let row = text.byte_to_line(point.min(text.len_bytes()));
    let start = text.line_to_byte(row);
    start + text.line_len_bytes(row)
}
pub(crate) fn bol(text: &Text, point: usize) -> usize {
    text.line_to_byte(text.byte_to_line(point.min(text.len_bytes())))
}
pub(crate) fn first(text: &Text, point: usize) -> usize {
    let mut p = bol(text, point);
    let stop = end(text, p);
    while p < stop && matches!(text.char_at(p), ' ' | '\t') {
        p = text.next_grapheme_boundary(p);
    }
    p
}
pub(crate) fn normal(text: &Text, point: usize) -> usize {
    let mut p = text.floor_char_boundary(point.min(text.len_bytes()));
    if p < text.len_bytes() {
        p = text.prev_grapheme_boundary(text.next_grapheme_boundary(p));
    }
    let stop = end(text, p);
    if p >= stop && stop > bol(text, p) {
        text.prev_grapheme_boundary(stop)
    } else {
        p
    }
}
fn class(text: &Text, p: usize, big: bool) -> u8 {
    if p >= text.len_bytes() {
        return 0;
    }
    let ch = text.char_at(p);
    if ch.is_whitespace() {
        0
    } else if big || ch.is_alphanumeric() || ch == '_' {
        1
    } else {
        2
    }
}
fn word(text: &Text, mut p: usize, key: &str, count: usize) -> usize {
    let big = matches!(key, "W" | "B" | "E");
    for _ in 0..count {
        let before = p;
        match key {
            "w" | "W" => {
                let c = class(text, p, big);
                while p < text.len_bytes() && class(text, p, big) == c {
                    p = text.next_grapheme_boundary(p);
                }
                while p < text.len_bytes() && class(text, p, big) == 0 {
                    p = text.next_grapheme_boundary(p);
                }
            }
            "b" | "B" => {
                p = text.prev_grapheme_boundary(p);
                while p > 0 && class(text, p, big) == 0 {
                    p = text.prev_grapheme_boundary(p);
                }
                let c = class(text, p, big);
                while p > 0 {
                    let prev = text.prev_grapheme_boundary(p);
                    if class(text, prev, big) != c {
                        break;
                    }
                    p = prev;
                }
            }
            "e" | "E" => {
                p = text.next_grapheme_boundary(p);
                while p < text.len_bytes() && class(text, p, big) == 0 {
                    p = text.next_grapheme_boundary(p);
                }
                let c = class(text, p, big);
                while p < text.len_bytes() {
                    let next = text.next_grapheme_boundary(p);
                    if next == text.len_bytes() || class(text, next, big) != c {
                        break;
                    }
                    p = next;
                }
            }
            "ge" => {
                p = text.prev_grapheme_boundary(p);
                while p > 0 {
                    let next = text.next_grapheme_boundary(p);
                    if class(text, p, false) != 0
                        && (next == text.len_bytes()
                            || class(text, next, false) != class(text, p, false))
                    {
                        break;
                    }
                    p = text.prev_grapheme_boundary(p);
                }
            }
            _ => unreachable!(),
        }
        if p == before {
            break;
        }
    }
    p
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Motion {
    pub point: usize,
    pub inclusive: bool,
    pub linewise: bool,
    pub vertical: bool,
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn motion(
    text: &Text,
    r: Region,
    key: &str,
    count: usize,
    explicit: bool,
    layout: &dyn VerticalLayout,
    top: usize,
    height: usize,
) -> Option<Motion> {
    let p = r.b;
    let mut inclusive = false;
    let mut linewise = false;
    let mut vertical = false;
    let row = text.byte_to_line(p);
    let last = text.len_lines().saturating_sub(1);
    let point = match key {
        "alt+w" | "alt+b" => {
            let mut b = vel_buffer::Buffer::new(text.clone());
            let v = b.new_view();
            b.set_selection(
                v,
                vel_buffer::Selection::from_regions([vel_buffer::Region::caret(p)]),
            );
            for _ in 0..count {
                vel_buffer::ops::move_by(
                    &mut b,
                    v,
                    vel_buffer::ops::MoveUnit::Subwords,
                    key == "alt+w",
                    false,
                    None,
                );
            }
            b.view(v).sel().primary().expect("selection").b
        }
        "h" => {
            let mut n = p;
            for _ in 0..count {
                n = text.prev_grapheme_boundary(n).max(bol(text, p));
            }
            n
        }
        "l" => {
            let mut n = p;
            for _ in 0..count {
                n = text.next_grapheme_boundary(n).min(end(text, p));
            }
            normal(text, n)
        }
        "j" | "k" => {
            vertical = true;
            linewise = true;
            let line = if key == "j" {
                row.saturating_add(count).min(last)
            } else {
                row.saturating_sub(count)
            };
            normal(
                text,
                layout.offset_at(text, line, r.xpos.unwrap_or_else(|| layout.x_of(text, p))),
            )
        }
        "w" | "W" | "b" | "B" | "e" | "E" | "ge" => {
            inclusive = matches!(key, "e" | "E" | "ge");
            word(text, p, key, count)
        }
        "0" => bol(text, p),
        "^" => first(text, p),
        "$" => {
            inclusive = true;
            let line = row.saturating_add(count - 1).min(last);
            normal(text, end(text, text.line_to_byte(line)))
        }
        "gg" | "G" => {
            linewise = true;
            let line = if explicit {
                count.saturating_sub(1).min(last)
            } else if key == "gg" {
                0
            } else {
                last
            };
            first(text, text.line_to_byte(line))
        }
        "H" | "M" | "L" => {
            linewise = true;
            let bottom = top.saturating_add(height.saturating_sub(1)).min(last);
            let line = match key {
                "H" => top.saturating_add(count - 1).min(bottom),
                "L" => bottom.saturating_sub(count - 1).max(top.min(last)),
                _ => top.saturating_add(bottom.saturating_sub(top) / 2),
            };
            first(text, text.line_to_byte(line.min(last)))
        }
        "%" => {
            inclusive = true;
            if explicit {
                linewise = true;
                let line = (last + 1)
                    .saturating_mul(count.min(100))
                    .div_ceil(100)
                    .saturating_sub(1);
                first(text, text.line_to_byte(line))
            } else {
                paired(text, p)?
            }
        }
        "{" | "}" => {
            linewise = false;
            let mut line = row;
            for _ in 0..count {
                if key == "}" {
                    if line < last {
                        line += 1;
                    }
                    while line < last && !blank(text, line) {
                        line += 1;
                    }
                } else {
                    line = line.saturating_sub(1);
                    while line > 0 && !blank(text, line) {
                        line -= 1;
                    }
                }
            }
            text.line_to_byte(line)
        }
        "(" | ")" => {
            let starts = sentence_starts(text);
            let mut n = p;
            for _ in 0..count {
                n = if key == ")" {
                    starts
                        .iter()
                        .copied()
                        .find(|s| *s > n)
                        .unwrap_or(text.len_bytes())
                } else {
                    starts.iter().copied().rev().find(|s| *s < n).unwrap_or(0)
                };
            }
            n
        }
        _ => return None,
    };
    Some(Motion {
        point,
        inclusive,
        linewise,
        vertical,
    })
}
fn blank(text: &Text, row: usize) -> bool {
    text.line(row).chars().all(char::is_whitespace)
}
fn paired(text: &Text, p: usize) -> Option<usize> {
    let mut at = p;
    let stop = end(text, p);
    while at < stop && !"()[]{}".contains(text.char_at(at)) {
        at = text.next_grapheme_boundary(at);
    }
    if at == stop {
        return None;
    }
    let ch = text.char_at(at);
    let (open, close, forward) = match ch {
        '(' => ('(', ')', true),
        ')' => ('(', ')', false),
        '[' => ('[', ']', true),
        ']' => ('[', ']', false),
        '{' => ('{', '}', true),
        _ => ('{', '}', false),
    };
    let mut depth = 1;
    loop {
        if forward {
            if at >= text.len_bytes() {
                return None;
            }
            at = text.next_grapheme_boundary(at);
            if at >= text.len_bytes() {
                return None;
            }
        } else {
            if at == 0 {
                return None;
            }
            at = text.prev_grapheme_boundary(at);
        }
        let c = text.char_at(at);
        if c == if forward { open } else { close } {
            depth += 1;
        } else if c == if forward { close } else { open } {
            depth -= 1;
            if depth == 0 {
                return Some(at);
            }
        }
    }
}
pub(crate) fn find(
    text: &Text,
    p: usize,
    target: char,
    forward: bool,
    till: bool,
    count: usize,
) -> Option<usize> {
    let mut at = p;
    let range = bol(text, p)..end(text, p);
    for _ in 0..count {
        loop {
            if forward {
                at = text.next_grapheme_boundary(at);
                if at >= range.end {
                    return None;
                }
            } else {
                if at <= range.start {
                    return None;
                }
                at = text.prev_grapheme_boundary(at);
            }
            if text.char_at(at) == target {
                break;
            }
        }
    }
    Some(if till {
        if forward {
            text.prev_grapheme_boundary(at)
        } else {
            text.next_grapheme_boundary(at)
        }
    } else {
        at
    })
}
pub(crate) fn line_range(text: &Text, range: Range<usize>) -> Range<usize> {
    let start = bol(text, range.start);
    let last = if range.end > range.start {
        text.prev_grapheme_boundary(range.end)
    } else {
        range.end
    };
    let row = text.byte_to_line(last);
    start..if row + 1 < text.len_lines() {
        text.line_to_byte(row + 1)
    } else {
        text.len_bytes()
    }
}
pub(crate) fn object(
    text: &Text,
    p: usize,
    key: char,
    around: bool,
    count: usize,
) -> Option<Range<usize>> {
    if matches!(key, 'w' | 'W') {
        if text.is_empty() {
            return None;
        }
        let p = p.min(text.prev_grapheme_boundary(text.len_bytes()));
        let big = key == 'W';
        let c = class(text, p, big);
        let mut a = p;
        let mut b = text.next_grapheme_boundary(p);
        while a > 0 {
            let prev = text.prev_grapheme_boundary(a);
            if class(text, prev, big) != c {
                break;
            }
            a = prev;
        }
        while b < text.len_bytes() && class(text, b, big) == c {
            b = text.next_grapheme_boundary(b);
        }
        for _ in 1..count {
            while b < text.len_bytes() && class(text, b, big) == 0 {
                b = text.next_grapheme_boundary(b);
            }
            let c = class(text, b, big);
            while b < text.len_bytes() && class(text, b, big) == c {
                b = text.next_grapheme_boundary(b);
            }
        }
        if around {
            let old = b;
            while b < text.len_bytes() && text.char_at(b).is_whitespace() && text.char_at(b) != '\n'
            {
                b = text.next_grapheme_boundary(b);
            }
            if b == old {
                while a > 0 {
                    let prev = text.prev_grapheme_boundary(a);
                    if !text.char_at(prev).is_whitespace() || text.char_at(prev) == '\n' {
                        break;
                    }
                    a = prev;
                }
            }
        }
        return Some(a..b);
    }
    if key == 'p' {
        let mut row = text.byte_to_line(p);
        let mut last = row;
        while row > 0 && !blank(text, row - 1) {
            row -= 1;
        }
        while last + 1 < text.len_lines() && !blank(text, last + 1) {
            last += 1;
        }
        for _ in 1..count {
            while last + 1 < text.len_lines() && blank(text, last + 1) {
                last += 1;
            }
            while last + 1 < text.len_lines() && !blank(text, last + 1) {
                last += 1;
            }
        }
        if around {
            while last + 1 < text.len_lines() && blank(text, last + 1) {
                last += 1;
            }
        }
        return Some(
            text.line_to_byte(row)..if last + 1 < text.len_lines() {
                text.line_to_byte(last + 1)
            } else {
                text.len_bytes()
            },
        );
    }
    if key == 's' {
        let starts = sentence_starts(text);
        let a = starts.iter().copied().rev().find(|s| *s <= p).unwrap_or(0);
        let mut b = starts
            .iter()
            .copied()
            .filter(|s| *s > a)
            .nth(count - 1)
            .unwrap_or(text.len_bytes());
        if !around {
            while b > a {
                let prev = text.prev_grapheme_boundary(b);
                if !text.char_at(prev).is_whitespace() {
                    break;
                }
                b = prev;
            }
        }
        return Some(a..b);
    }
    if matches!(key, '"' | '\'' | '`') {
        let a = bol(text, p);
        let b = end(text, p);
        let mut escaped = false;
        let mut start = None;
        let mut pairs = Vec::new();
        for (offset, ch) in text.slice(a..b).to_string().char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == key {
                if let Some(open) = start.take() {
                    pairs.push((open, a + offset));
                } else {
                    start = Some(a + offset);
                }
            }
        }
        let (a, b) = pairs.into_iter().find(|(a, b)| *a <= p && p <= *b)?;
        return Some(if around { a..b + 1 } else { a + 1..b });
    }
    if key == 't' {
        return tag_object(text, p, around, count);
    }
    let (open, close) = match key {
        '(' | ')' | 'b' => ('(', ')'),
        '[' | ']' => ('[', ']'),
        '{' | '}' | 'B' => ('{', '}'),
        '<' | '>' => ('<', '>'),
        _ => return None,
    };
    let mut stack = Vec::new();
    let mut pairs = Vec::new();
    for (at, ch) in text.to_string().char_indices() {
        if ch == open {
            stack.push(at);
        } else if ch == close
            && let Some(a) = stack.pop()
            && a <= p
            && p <= at
        {
            pairs.push((a, at));
        }
    }
    pairs.sort_by_key(|(a, b)| b - a);
    let (a, b) = *pairs.get(count - 1)?;
    Some(if around { a..b + 1 } else { a + 1..b })
}
fn sentence_starts(text: &Text) -> Vec<usize> {
    let mut starts = vec![0];
    let mut p = 0;
    while p < text.len_bytes() {
        let ch = text.char_at(p);
        let mut next = text.next_grapheme_boundary(p);
        if ".!?".contains(ch) {
            while next < text.len_bytes() && ")]\"'".contains(text.char_at(next)) {
                next = text.next_grapheme_boundary(next);
            }
            if next < text.len_bytes() && text.char_at(next).is_whitespace() {
                while next < text.len_bytes() && text.char_at(next).is_whitespace() {
                    next = text.next_grapheme_boundary(next);
                }
                if next < text.len_bytes() {
                    starts.push(next);
                }
            }
        }
        p = next;
    }
    starts
}
fn tag_object(text: &Text, p: usize, around: bool, count: usize) -> Option<Range<usize>> {
    let source = text.to_string();
    let regex = regex::Regex::new(r"</?([A-Za-z][\w:.-]*)\b[^>]*>").ok()?;
    let mut stack = Vec::new();
    let mut pairs = Vec::new();
    for caps in regex.captures_iter(&source) {
        let tag = caps.get(0)?;
        let name = caps.get(1)?.as_str();
        if tag.as_str().starts_with("</") {
            if let Some(i) = stack.iter().rposition(|(n, _, _)| *n == name) {
                let (_, a, end) = stack.remove(i);
                if a <= p && p < tag.end() {
                    pairs.push((a, end, tag.start(), tag.end()));
                }
                stack.truncate(i);
            }
        } else if !tag.as_str().ends_with("/>") {
            stack.push((name, tag.start(), tag.end()));
        }
    }
    pairs.sort_by_key(|(a, _, _, b)| b - a);
    let (a, start, end, b) = *pairs.get(count - 1)?;
    Some(if around { a..b } else { start..end })
}
