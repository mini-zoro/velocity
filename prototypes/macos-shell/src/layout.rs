//! Logical-point geometry. Rasterization alone converts points to physical pixels.
use crate::{
    editor::{Cursor, Editor, byte_at_cell, cells},
    lexer::Kind,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
#[derive(Clone, Debug)]
pub struct TextRun {
    pub text: String,
    pub x: f32,
    pub baseline: f32,
    pub color: [f32; 4],
    pub gutter: bool,
}
#[derive(Clone, Debug)]
pub struct ColoredRect {
    pub rect: Rect,
    pub color: [f32; 4],
}
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub text: Vec<TextRun>,
    pub rects: Vec<ColoredRect>,
    pub ime_rect: Rect,
    pub first_row: usize,
    pub visible_rows: usize,
}
#[derive(Clone, Debug)]
pub struct View {
    pub width: f32,
    pub height: f32,
    pub cell: f32,
    pub line_height: f32,
    pub ascent: f32,
    pub scroll_y: f32,
    pub scroll_x: f32,
    pub gutter: f32,
}
impl View {
    pub fn new(width: f32, height: f32, cell: f32, ascent: f32) -> Self {
        Self {
            width,
            height,
            cell,
            line_height: 20.,
            ascent,
            scroll_y: 0.,
            scroll_x: 0.,
            gutter: 64.,
        }
    }
    pub fn rows(&self) -> usize {
        ((self.height - 8.).max(1.) / self.line_height).ceil() as usize + 1
    }
    pub fn fully_visible_rows(&self) -> usize {
        ((self.height - 8.).max(1.) / self.line_height)
            .floor()
            .max(1.) as usize
    }
    pub fn clamp_scroll(&mut self, lines: usize) {
        self.scroll_y = self.scroll_y.clamp(
            0.,
            (lines as f32 * self.line_height - (self.height - 8.)).max(0.),
        );
        self.scroll_x = self.scroll_x.max(0.);
    }
    pub fn scroll(&mut self, x: f32, y: f32, lines: usize) {
        self.scroll_x += x;
        self.scroll_y += y;
        self.clamp_scroll(lines);
    }
    pub fn reveal(&mut self, e: &Editor) {
        let top = e.cursor.line as f32 * self.line_height;
        let bottom = top + self.line_height;
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + self.height - 8. {
            self.scroll_y = bottom - self.height + 8.;
        }
        let preedit = e
            .ime
            .selection
            .map_or(0, |(start, _)| cells(&e.ime.preedit[..start]));
        let x = (cells(&e.lines[e.cursor.line][..e.cursor.byte]) + preedit) as f32 * self.cell;
        if x < self.scroll_x {
            self.scroll_x = x;
        } else if x + self.cell > self.scroll_x + self.width - self.gutter - 16. {
            self.scroll_x = x + self.cell - self.width + self.gutter + 16.;
        }
        self.clamp_scroll(e.lines.len());
    }
    pub fn hit_test(&self, e: &Editor, x: f32, y: f32) -> Cursor {
        let row = (((y - 4. + self.scroll_y) / self.line_height)
            .floor()
            .max(0.) as usize)
            .min(e.lines.len() - 1);
        let col = ((x - self.gutter - 8. + self.scroll_x) / self.cell)
            .round()
            .max(0.) as usize;
        Cursor {
            line: row,
            byte: byte_at_cell(&e.lines[row], col),
        }
    }
    pub fn cursor_rect(&self, e: &Editor) -> Rect {
        let advance = e
            .ime
            .selection
            .map_or(0, |(start, _)| cells(&e.ime.preedit[..start]));
        Rect {
            x: self.gutter
                + 8.
                + (cells(&e.lines[e.cursor.line][..e.cursor.byte]) + advance) as f32 * self.cell
                - self.scroll_x,
            y: 4. + e.cursor.line as f32 * self.line_height - self.scroll_y,
            w: 1.,
            h: self.line_height,
        }
    }
    /// winit expects top-left logical client coordinates; clip offscreen candidate anchors.
    pub fn candidate_rect(&self, e: &Editor) -> Rect {
        let r = self.cursor_rect(e);
        Rect {
            x: r.x
                .clamp(self.gutter.min(self.width - 1.), (self.width - 1.).max(0.)),
            y: r.y.clamp(0., (self.height - self.line_height).max(0.)),
            ..r
        }
    }
    pub fn scene(&self, e: &mut Editor) -> Scene {
        let first = (self.scroll_y / self.line_height).floor() as usize;
        let end = (first + self.rows()).min(e.lines.len());
        let mut s = Scene {
            ime_rect: self.candidate_rect(e),
            first_row: first,
            visible_rows: end.saturating_sub(first),
            ..Scene::default()
        };
        let cursor_y = 4. + e.cursor.line as f32 * self.line_height - self.scroll_y;
        s.rects.push(ColoredRect {
            rect: Rect {
                x: self.gutter,
                y: cursor_y,
                w: (self.width - self.gutter).max(0.),
                h: self.line_height,
            },
            color: [0.105, 0.125, 0.15, 1.],
        });
        s.rects.push(ColoredRect {
            rect: Rect {
                x: 0.,
                y: 0.,
                w: self.gutter,
                h: self.height,
            },
            color: [0.065, 0.075, 0.09, 1.],
        });
        let selection = e.selection();
        for row in first..end {
            let y = 4. + row as f32 * self.line_height - self.scroll_y;
            let baseline = y + self.ascent + 2.;
            let line_number = (row + 1).to_string();
            s.text.push(TextRun {
                text: line_number.clone(),
                x: self.gutter - 12. - line_number.len() as f32 * self.cell,
                baseline,
                color: [0.38, 0.44, 0.51, 1.],
                gutter: true,
            });
            if let Some((a, b)) = selection
                && a.line <= row
                && row <= b.line
            {
                let start = if row == a.line {
                    cells(&e.lines[row][..a.byte])
                } else {
                    0
                };
                let end = if row == b.line {
                    cells(&e.lines[row][..b.byte])
                } else {
                    cells(&e.lines[row]) + 1
                };
                s.rects.push(ColoredRect {
                    rect: Rect {
                        x: self.gutter + 8. + start as f32 * self.cell - self.scroll_x,
                        y,
                        w: (end - start) as f32 * self.cell,
                        h: self.line_height,
                    },
                    color: [0.19, 0.29, 0.42, 1.],
                });
            }
            let tokens = e.line_tokens(row).to_vec();
            let line = &e.lines[row];
            let mut col = 0;
            let mut token_index = 0;
            let composing = row == e.cursor.line && e.ime.composing();
            let mut inserted = false;
            for (byte, g) in line.grapheme_indices(true) {
                if composing && !inserted && byte >= e.cursor.byte {
                    self.add_preedit(&mut s, e, col, y, baseline);
                    col += cells(&e.ime.preedit);
                    inserted = true;
                }
                while token_index + 1 < tokens.len() && tokens[token_index].bytes.end <= byte {
                    token_index += 1;
                }
                let color = tokens
                    .get(token_index)
                    .map_or(Kind::Plain, |t| t.kind)
                    .color();
                self.add_grapheme(&mut s, g, col, baseline, color, false);
                col += if g == "\t" {
                    4 - col % 4
                } else {
                    UnicodeWidthStr::width(g)
                };
                if self.gutter + 8. + col as f32 * self.cell - self.scroll_x
                    > self.width + self.cell * 2.
                {
                    break;
                }
            }
            if composing && !inserted && e.cursor.byte == line.len() {
                self.add_preedit(&mut s, e, col, y, baseline);
            }
        }
        if !e.ime.composing() || e.ime.selection.is_some() {
            s.rects.push(ColoredRect {
                rect: self.cursor_rect(e),
                color: [0.91, 0.94, 1., 1.],
            });
        }
        s
    }
    fn add_grapheme(
        &self,
        s: &mut Scene,
        g: &str,
        col: usize,
        baseline: f32,
        color: [f32; 4],
        gutter: bool,
    ) {
        let x = self.gutter + 8. + col as f32 * self.cell - self.scroll_x;
        if g != "\t"
            && g != " "
            && x + UnicodeWidthStr::width(g) as f32 * self.cell >= self.gutter
            && x < self.width
        {
            s.text.push(TextRun {
                text: g.to_owned(),
                x,
                baseline,
                color,
                gutter,
            });
        }
    }
    fn add_preedit(&self, s: &mut Scene, e: &Editor, col: usize, y: f32, baseline: f32) {
        let x = self.gutter + 8. + col as f32 * self.cell - self.scroll_x;
        let w = cells(&e.ime.preedit) as f32 * self.cell;
        if let Some((a, b)) = e.ime.selection
            && a != b
        {
            let lo = a.min(b);
            let hi = a.max(b);
            s.rects.push(ColoredRect {
                rect: Rect {
                    x: x + cells(&e.ime.preedit[..lo]) as f32 * self.cell,
                    y,
                    w: cells(&e.ime.preedit[lo..hi]) as f32 * self.cell,
                    h: self.line_height,
                },
                color: [0.26, 0.32, 0.44, 1.],
            });
        }
        let mut c = col;
        for g in e.ime.preedit.graphemes(true) {
            self.add_grapheme(s, g, c, baseline, [0.98, 0.86, 0.57, 1.], false);
            c += UnicodeWidthStr::width(g);
        }
        s.rects.push(ColoredRect {
            rect: Rect {
                x,
                y: y + self.line_height - 2.,
                w,
                h: 1.,
            },
            color: [0.98, 0.86, 0.57, 1.],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_scroll_and_hidpi_candidate_geometry() {
        let mut e = Editor::from_text("你abc\nnext");
        e.cursor.byte = 3;
        e.ime.update("hao".into(), Some((2, 2)));
        let mut v = View::new(400., 100., 8., 13.);
        v.scroll_y = 3.5;
        let r = v.cursor_rect(&e);
        assert_eq!(r.x, 104.);
        assert_eq!(r.y, 0.5);
        assert_eq!(r.h, 20.);
        let physical = [r.x * 2., r.y * 2.];
        assert_eq!(physical, [208., 1.]);
        let s = v.scene(&mut e);
        assert!(s.text.iter().any(|t| t.text == "h"));
        assert!(s.rects.iter().any(|r| r.rect.h == 1.));
    }
    #[test]
    fn hit_test_and_reveal_respect_unicode_and_gutter() {
        let mut e = Editor::from_text("你abc\n\nx");
        let mut v = View::new(400., 40., 8., 13.);
        assert_eq!(v.hit_test(&e, 88., 5.).byte, 3);
        e.cursor = Cursor { line: 2, byte: 1 };
        v.reveal(&e);
        assert!(v.scroll_y > 0.);
        assert!(v.cursor_rect(&e).y + 20. <= v.height);
    }
    #[test]
    fn huge_line_is_clipped_and_selection_is_visible() {
        let mut e = Editor::from_text(&"abc".repeat(100000));
        e.anchor = Some(Cursor::default());
        e.cursor.byte = 3;
        let v = View::new(400., 100., 8., 13.);
        let s = v.scene(&mut e);
        assert!(s.text.len() < 60);
        assert!(s.rects.iter().any(|r| r.color == [0.19, 0.29, 0.42, 1.]));
    }
    #[test]
    fn candidate_anchor_clips_when_scrolled_away() {
        let e = Editor::from_text("a\nb\nc");
        let mut v = View::new(400., 20., 8., 13.);
        v.scroll_y = 500.;
        let r = v.candidate_rect(&e);
        assert_eq!(r.y, 0.);
        assert_eq!(v.rows(), 2);
    }
}
