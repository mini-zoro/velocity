//! Mouse selection gestures with Sublime's `drag_select` semantics.
//!
//! - A click selects a unit (caret, word after a double click, line after a triple click or a
//!   click on the line numbers); dragging extends the selection by whole units from that origin.
//! - ⌘ adds the gesture's region to the existing selection, ⌘⇧ subtracts it, ⇧ extends from the
//!   current anchor.
//! - ⌥ (or the middle button) makes a column selection: one region per visual row between the
//!   press and the pointer.
//! - While the button is held outside the text area the view scrolls, faster the farther out the
//!   pointer is, and the selection follows.
use crate::{editor::Editor, view::View};
use std::time::Instant;
use vel_buffer::{Region, Selection};
use vel_platform::Point;

/// Selection unit of a gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Characters (single click).
    Char,
    /// Words, using the editor's double-click rules.
    Word,
    /// Whole lines including the line break.
    Line,
}

/// How the gesture combines with the selection that existed before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combine {
    /// Replace the selection.
    Replace,
    /// Add to it (⌘).
    Add,
    /// Remove from it (⌘⇧).
    Subtract,
}

/// An in-progress mouse selection, from button press to release.
#[derive(Clone, Debug)]
pub struct DragSelect {
    unit: Unit,
    combine: Combine,
    column: bool,
    /// Selection before an additive or subtractive gesture.
    base: Selection,
    /// Region selected by the press (caret, word or line), as `begin..end`; for an extending
    /// press, a caret at the anchor.
    origin: Region,
    /// Column selection anchor in document coordinates (x and y include the scroll offset).
    anchor: Point,
    /// Last pointer position in view coordinates, re-applied while auto-scrolling.
    pointer: Point,
    /// Last auto-scroll step, for frame-rate independent scrolling.
    scrolled: Option<Instant>,
}

/// The character under view point `point`, given the nearest boundary `at`: the boundary before
/// `at` when the pointer is on the left half of the preceding character of the same row.
fn under_pointer(view: &View, editor: &Editor, point: Point, at: usize) -> usize {
    if at == 0 {
        return at;
    }
    let prev = editor.text.prev_grapheme_boundary(at);
    let (here, before) = (view.rect_at(editor, at), view.rect_at(editor, prev));
    if before.y == here.y && here.x > point.x {
        prev
    } else {
        at
    }
}
/// Text byte for a unit at `at`, as a non-directional region.
fn unit_at(editor: &mut Editor, unit: Unit, at: usize) -> Region {
    match unit {
        Unit::Char => {
            let at = editor.text.floor_char_boundary(at);
            Region::caret(at)
        }
        Unit::Word => {
            editor.select_word_under(at);
            let r = editor.regions()[0];
            Region::new(r.begin(), r.end())
        }
        Unit::Line => {
            editor.select_line(at);
            let r = editor.regions()[0];
            Region::new(r.begin(), r.end())
        }
    }
}

impl DragSelect {
    /// Starts a gesture at view point `point` (byte `at`) and applies the press.
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        view: &View,
        editor: &mut Editor,
        point: Point,
        at: usize,
        unit: Unit,
        combine: Combine,
        extend: bool,
        column: bool,
    ) -> Self {
        let base = Selection::from_regions(editor.regions());
        let origin = if extend && combine == Combine::Replace && unit == Unit::Char {
            Region::caret(editor.anchor)
        } else if unit == Unit::Word {
            let at = under_pointer(view, editor, point, at);
            unit_at(editor, unit, at)
        } else {
            unit_at(editor, unit, at)
        };
        let mut drag = Self {
            unit,
            combine,
            column,
            base,
            origin,
            anchor: Point {
                x: point.x + view.scroll_x,
                y: point.y + view.scroll_y,
            },
            pointer: point,
            scrolled: None,
        };
        drag.update(view, editor, point);
        drag
    }

    /// Extends the gesture to view point `point`.
    pub fn update(&mut self, view: &View, editor: &mut Editor, point: Point) {
        self.pointer = point;
        let regions = if self.column {
            self.column_regions(view, editor, point)
        } else {
            let mut at = view.hit_test(editor, point);
            if self.unit == Unit::Word {
                at = under_pointer(view, editor, point, at);
            }
            let current = unit_at(editor, self.unit, at);
            vec![if current.begin() >= self.origin.begin() {
                Region::new(self.origin.begin(), current.end())
            } else {
                Region::new(self.origin.end(), current.begin())
            }]
        };
        let selection = match self.combine {
            Combine::Replace => Selection::from_regions(regions),
            Combine::Add => {
                let mut selection = self.base.clone();
                for r in regions {
                    selection.add(r);
                }
                selection
            }
            Combine::Subtract => {
                let mut selection = self.base.clone();
                for r in regions {
                    selection.subtract(r);
                }
                selection
            }
        };
        editor.set_selection(selection);
    }

    /// One region per visual row between the press and `point`, between the two x positions.
    /// Rows that end before both x positions are skipped unless every row does (a column of
    /// carets past the line ends).
    fn column_regions(&self, view: &View, editor: &mut Editor, point: Point) -> Vec<Region> {
        let row_of = |y: f64| {
            ((y - view.tab_height - 4.) / view.line_height)
                .floor()
                .max(0.)
        };
        let first = row_of(self.anchor.y) as usize;
        let last = row_of(point.y + view.scroll_y) as usize;
        let total = view.total(editor).max(1);
        let (first, last) = (first.min(total - 1), last.min(total - 1));
        let (anchor_x, pointer_x) = (self.anchor.x - view.scroll_x, point.x);
        let mut inside = Vec::new();
        let mut past_end = Vec::new();
        let rows: Box<dyn Iterator<Item = usize>> = if last >= first {
            Box::new(first..=last)
        } else {
            Box::new((last..=first).rev())
        };
        for row in rows {
            let y = view.tab_height + 4. + (row as f64 + 0.5) * view.line_height - view.scroll_y;
            let a = view.hit_test(editor, Point { x: anchor_x, y });
            let b = view.hit_test(editor, Point { x: pointer_x, y });
            let line = editor.text.byte_to_line(a);
            let end = editor.text.line_to_byte(line) + editor.text.line_len_bytes(line);
            let end_x = view.rect_at(editor, end).x;
            let region = Region::new(a, b);
            if anchor_x.min(pointer_x) > end_x + view.cell / 2. {
                past_end.push(region);
            } else {
                inside.push(region);
            }
        }
        if inside.is_empty() { past_end } else { inside }
    }

    /// Scrolls when the pointer is outside the text area and re-applies the gesture. Returns
    /// whether anything changed; the caller keeps requesting frames while it returns true.
    pub fn autoscroll(&mut self, view: &mut View, editor: &mut Editor, now: Instant) -> bool {
        let top = view.tab_height;
        let bottom = view.size.height;
        let left = view.gutter(editor);
        let right = view.right(editor);
        let p = self.pointer;
        let dy = if p.y < top {
            p.y - top
        } else if p.y > bottom {
            p.y - bottom
        } else {
            0.
        };
        let dx = if p.x < left && view.scroll_x > 0. {
            p.x - left
        } else if p.x > right {
            p.x - right
        } else {
            0.
        };
        if dx == 0. && dy == 0. {
            self.scrolled = None;
            return false;
        }
        let elapsed = self
            .scrolled
            .map_or(1. / 60., |t| now.duration_since(t).as_secs_f64().min(0.05));
        self.scrolled = Some(now);
        // Speed grows with the distance outside: ~14 lines/s just past the edge.
        let speed = |d: f64| d.signum() * (d.abs() + 8.) * 12. * elapsed;
        let before = (view.scroll_x, view.scroll_y);
        view.scroll(editor, speed(dx), speed(dy));
        if before == (view.scroll_x, view.scroll_y) {
            return false;
        }
        let pointer = self.pointer;
        self.update(view, editor, pointer);
        true
    }
}

/// Moving (or with ⌥ at release, copying) the selected text with the mouse (`drag_text`).
#[derive(Clone, Debug)]
pub struct DragText {
    press: Point,
    at: usize,
    source: Region,
    moved: bool,
}

impl DragText {
    /// Starts when a plain press lands inside the only non-empty selection and `drag_text` is on.
    pub fn begin(editor: &Editor, point: Point, at: usize) -> Option<Self> {
        let regions = editor.regions();
        let [source] = regions.as_slice() else {
            return None;
        };
        (editor.settings.bool("drag_text", true)
            && !editor.read_only
            && !source.empty()
            && source.begin() <= at
            && at < source.end())
        .then_some(Self {
            press: point,
            at,
            source: Region::new(source.begin(), source.end()),
            moved: false,
        })
    }

    /// Follows the pointer, showing the drop position once it moved past a small threshold.
    pub fn update(&mut self, view: &mut View, editor: &Editor, point: Point) {
        self.moved |= (point.x - self.press.x).hypot(point.y - self.press.y) > 4.;
        if self.moved {
            view.drop_caret = Some(view.hit_test(editor, point));
        }
    }

    /// Releases at `point`: a click without movement places the caret; a drop outside the
    /// source moves (or copies) the text there as one undo step and selects it.
    pub fn finish(self, view: &mut View, editor: &mut Editor, point: Point, copy: bool) {
        view.drop_caret = None;
        if !self.moved {
            editor.set_caret(self.at, false);
            return;
        }
        let drop = view.hit_test(editor, point);
        let source = self.source;
        if source.begin() <= drop && drop <= source.end() {
            return;
        }
        let text = editor.text.slice(source.to_range()).to_string();
        let len = text.len();
        let start = if !copy && drop > source.end() {
            drop - source.size()
        } else {
            drop
        };
        let mut edits = vec![(drop..drop, text)];
        if !copy {
            edits.push((source.to_range(), String::new()));
        }
        editor.apply_edits(
            edits,
            Some(Selection::from_regions([Region::new(start, start + len)])),
            vel_buffer::EditKind::Other,
        );
        editor.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use serde_json::json;
    use vel_platform::Size;

    fn setup(text: &str) -> (Workspace, View) {
        let workspace = Workspace::new(vel_text::Text::from(text), None, None, false).unwrap();
        let mut view = View::new(
            Size {
                width: 600.,
                height: 200.,
            },
            8.,
            12.,
        );
        view.scene(workspace.editor());
        (workspace, view)
    }
    /// View point at the middle of `line`, `column` characters into the text.
    fn at(view: &View, workspace: &Workspace, line: usize, column: usize) -> Point {
        let e = workspace.editor();
        let byte = e.text.line_to_byte(line) + column;
        let r = view.rect_at(e, byte);
        Point {
            x: r.x + 1.,
            y: r.y + view.line_height / 2.,
        }
    }
    fn ranges(workspace: &Workspace) -> Vec<(usize, usize)> {
        workspace
            .editor()
            .regions()
            .iter()
            .map(|r| (r.a, r.b))
            .collect()
    }

    #[test]
    fn double_click_drag_extends_by_words_in_both_directions() {
        let (mut workspace, view) = setup("alpha beta gamma delta\n");
        let press = at(&view, &workspace, 0, 7); // inside "beta"
        let byte = view.hit_test(workspace.editor(), press);
        let mut drag = DragSelect::begin(
            &view,
            workspace.editor_mut(),
            press,
            byte,
            Unit::Word,
            Combine::Replace,
            false,
            false,
        );
        assert_eq!(ranges(&workspace), vec![(6, 10)]);
        let forward = at(&view, &workspace, 0, 13); // inside "gamma"
        drag.update(&view, workspace.editor_mut(), forward);
        assert_eq!(ranges(&workspace), vec![(6, 16)]);
        let backward = at(&view, &workspace, 0, 2); // inside "alpha"
        drag.update(&view, workspace.editor_mut(), backward);
        assert_eq!(ranges(&workspace), vec![(10, 0)]);
    }
    #[test]
    fn double_click_selects_the_character_run_under_the_pointer() {
        let (mut workspace, view) = setup("foo bar\n");
        // Right half of the last "o": the nearest boundary is after it, the word is still "foo".
        let o = at(&view, &workspace, 0, 2);
        let press = Point {
            x: o.x + view.cell * 0.7,
            y: o.y,
        };
        let byte = view.hit_test(workspace.editor(), press);
        assert_eq!(byte, 3);
        DragSelect::begin(
            &view,
            workspace.editor_mut(),
            press,
            byte,
            Unit::Word,
            Combine::Replace,
            false,
            false,
        );
        assert_eq!(ranges(&workspace), vec![(0, 3)]);
        // On the space itself the run of spaces is selected.
        let space = at(&view, &workspace, 0, 3);
        let byte = view.hit_test(workspace.editor(), space);
        DragSelect::begin(
            &view,
            workspace.editor_mut(),
            space,
            byte,
            Unit::Word,
            Combine::Replace,
            false,
            false,
        );
        assert_eq!(ranges(&workspace), vec![(3, 4)]);
    }
    #[test]
    fn triple_click_drag_extends_by_lines() {
        let (mut workspace, view) = setup("one\ntwo\nthree\nfour\n");
        let press = at(&view, &workspace, 1, 1);
        let byte = view.hit_test(workspace.editor(), press);
        let mut drag = DragSelect::begin(
            &view,
            workspace.editor_mut(),
            press,
            byte,
            Unit::Line,
            Combine::Replace,
            false,
            false,
        );
        assert_eq!(ranges(&workspace), vec![(4, 8)]);
        let p = at(&view, &workspace, 2, 2);
        drag.update(&view, workspace.editor_mut(), p);
        assert_eq!(ranges(&workspace), vec![(4, 14)]);
        let p = at(&view, &workspace, 0, 0);
        drag.update(&view, workspace.editor_mut(), p);
        assert_eq!(ranges(&workspace), vec![(8, 0)]);
    }
    #[test]
    fn command_drag_adds_and_command_shift_drag_subtracts() {
        let (mut workspace, view) = setup("aaaa bbbb cccc\n");
        workspace
            .editor_mut()
            .set_selection(Selection::from_regions([Region::new(0, 4)]));
        let press = at(&view, &workspace, 0, 10);
        let byte = view.hit_test(workspace.editor(), press);
        let mut drag = DragSelect::begin(
            &view,
            workspace.editor_mut(),
            press,
            byte,
            Unit::Char,
            Combine::Add,
            false,
            false,
        );
        let p = at(&view, &workspace, 0, 14);
        drag.update(&view, workspace.editor_mut(), p);
        assert_eq!(ranges(&workspace), vec![(0, 4), (10, 14)]);
        let press = at(&view, &workspace, 0, 1);
        let byte = view.hit_test(workspace.editor(), press);
        let mut drag = DragSelect::begin(
            &view,
            workspace.editor_mut(),
            press,
            byte,
            Unit::Char,
            Combine::Subtract,
            false,
            false,
        );
        let p = at(&view, &workspace, 0, 3);
        drag.update(&view, workspace.editor_mut(), p);
        assert_eq!(ranges(&workspace), vec![(0, 1), (3, 4), (10, 14)]);
    }
    #[test]
    fn option_drag_makes_one_region_per_row_and_skips_short_rows() {
        let (mut workspace, view) = setup("abcdef\nab\nabcdef\nabcdef\n");
        let press = at(&view, &workspace, 0, 3);
        let byte = view.hit_test(workspace.editor(), press);
        let mut drag = DragSelect::begin(
            &view,
            workspace.editor_mut(),
            press,
            byte,
            Unit::Char,
            Combine::Replace,
            false,
            true,
        );
        let p = at(&view, &workspace, 3, 5);
        drag.update(&view, workspace.editor_mut(), p);
        // Row 1 ("ab") ends before both columns and is skipped, like Sublime.
        assert_eq!(ranges(&workspace), vec![(3, 5), (13, 15), (20, 22)]);
    }
    #[test]
    fn dragging_below_the_view_scrolls_and_extends() {
        let text: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let (mut workspace, mut view) = setup(&text);
        let press = at(&view, &workspace, 0, 0);
        let byte = view.hit_test(workspace.editor(), press);
        let mut drag = DragSelect::begin(
            &view,
            workspace.editor_mut(),
            press,
            byte,
            Unit::Char,
            Combine::Replace,
            false,
            false,
        );
        let below = Point {
            x: press.x,
            y: view.size.height + 40.,
        };
        drag.update(&view, workspace.editor_mut(), below);
        let start = Instant::now();
        let mut now = start;
        for _ in 0..30 {
            now += std::time::Duration::from_millis(16);
            assert!(drag.autoscroll(&mut view, workspace.editor_mut(), now));
        }
        assert!(view.scroll_y > 100.);
        let caret = workspace.editor().regions()[0].b;
        let line = workspace.editor().text.byte_to_line(caret);
        assert!(line > 10, "selection follows the scroll, at line {line}");
        // Back inside the text area: no more scrolling.
        drag.update(&view, workspace.editor_mut(), press);
        assert!(!drag.autoscroll(&mut view, workspace.editor_mut(), now));
    }
    #[test]
    fn dragging_a_selection_moves_or_copies_it_as_one_undo_step() {
        let (mut workspace, mut view) = setup("one two three\n");
        workspace
            .editor_mut()
            .set_selection(Selection::from_regions([Region::new(0, 4)]));
        let press = at(&view, &workspace, 0, 1);
        let byte = view.hit_test(workspace.editor(), press);
        let mut drag = DragText::begin(workspace.editor(), press, byte).expect("inside");
        let target = at(&view, &workspace, 0, 13);
        drag.update(&mut view, workspace.editor(), target);
        assert_eq!(view.drop_caret, Some(13));
        drag.finish(&mut view, workspace.editor_mut(), target, false);
        assert_eq!(workspace.editor().text.to_string(), "two threeone \n");
        assert_eq!(ranges(&workspace), vec![(9, 13)]);
        assert_eq!(view.drop_caret, None);
        workspace.editor_mut().run("undo", &json!({})).unwrap();
        assert_eq!(workspace.editor().text.to_string(), "one two three\n");
        // Copy with ⌥ at release, dropping before the source.
        workspace
            .editor_mut()
            .set_selection(Selection::from_regions([Region::new(8, 13)]));
        let press = at(&view, &workspace, 0, 10);
        let byte = view.hit_test(workspace.editor(), press);
        let mut drag = DragText::begin(workspace.editor(), press, byte).expect("inside");
        let target = at(&view, &workspace, 0, 0);
        drag.update(&mut view, workspace.editor(), target);
        drag.finish(&mut view, workspace.editor_mut(), target, true);
        assert_eq!(workspace.editor().text.to_string(), "threeone two three\n");
        assert_eq!(ranges(&workspace), vec![(0, 5)]);
    }
    #[test]
    fn click_inside_a_selection_without_moving_places_the_caret() {
        let (mut workspace, mut view) = setup("one two three\n");
        workspace
            .editor_mut()
            .set_selection(Selection::from_regions([Region::new(0, 7)]));
        let press = at(&view, &workspace, 0, 2);
        let byte = view.hit_test(workspace.editor(), press);
        let drag = DragText::begin(workspace.editor(), press, byte).expect("inside");
        drag.finish(&mut view, workspace.editor_mut(), press, false);
        assert_eq!(ranges(&workspace), vec![(2, 2)]);
        assert_eq!(workspace.editor().text.to_string(), "one two three\n");
        // Outside the selection or with drag_text off it does not start.
        assert!(DragText::begin(workspace.editor(), press, 9).is_none());
    }
}
