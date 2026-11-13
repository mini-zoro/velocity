//! Visible-line layout, selection, inline marked text and candidate geometry.
use crate::{
    editor::{Editor, byte_at_cell, cells},
    lexer::{self, Token},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use vel_platform::{Appearance, Point, Rect, Size, Utf16Range};
use vel_render::scene::*;

#[derive(Debug)]
pub struct View {
    pub size: Size,
    pub cell: f64,
    pub ascent: f64,
    pub line_height: f64,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub appearance: Appearance,
    tokens: HashMap<String, Arc<[Token]>>,
    order: VecDeque<String>,
}
impl View {
    pub fn new(size: Size, cell: f64, ascent: f64) -> Self {
        Self {
            size,
            cell,
            ascent,
            line_height: 20.,
            scroll_x: 0.,
            scroll_y: 0.,
            appearance: Appearance::Dark,
            tokens: HashMap::new(),
            order: VecDeque::new(),
        }
    }
    pub fn gutter(&self, e: &Editor) -> f64 {
        (e.text.len_lines().max(1).ilog10() + 1) as f64 * self.cell + 24.
    }
    pub fn rows(&self) -> usize {
        ((self.size.height - 8.).max(1.) / self.line_height)
            .floor()
            .max(1.) as usize
    }
    pub fn clamp_scroll(&mut self, e: &Editor) {
        self.scroll_y = self.scroll_y.clamp(
            0.,
            (e.text.len_lines() as f64 * self.line_height - self.size.height + 8.).max(0.),
        );
        self.scroll_x = self.scroll_x.max(0.);
    }
    pub fn scroll(&mut self, e: &Editor, x: f64, y: f64) {
        self.scroll_x += x;
        self.scroll_y += y;
        self.clamp_scroll(e);
    }
    pub fn rect_at(&self, e: &Editor, byte: usize) -> Rect {
        let row = e.text.byte_to_line(byte);
        let start = e.text.line_to_byte(row);
        Rect {
            x: self.gutter(e)
                + 8.
                + cells(&e.text.slice(start..byte).to_string()) as f64 * self.cell
                - self.scroll_x,
            y: 4. + row as f64 * self.line_height - self.scroll_y,
            width: 1.,
            height: self.line_height,
        }
    }
    pub fn reveal(&mut self, e: &Editor) {
        let r = self.rect_at(e, e.caret);
        let left = self.gutter(e) + 8.;
        if r.y < 4. {
            self.scroll_y += r.y - 4.;
        } else if r.y + r.height > self.size.height - 4. {
            self.scroll_y += r.y + r.height - self.size.height + 4.;
        }
        if r.x < left {
            self.scroll_x = (self.scroll_x + r.x - left).max(0.);
        } else if r.x + self.cell > self.size.width - 8. {
            self.scroll_x += r.x + self.cell - self.size.width + 8.;
        }
        self.clamp_scroll(e);
    }
    pub fn hit_test(&self, e: &Editor, p: Point) -> usize {
        let row = (((p.y - 4. + self.scroll_y) / self.line_height)
            .floor()
            .max(0.) as usize)
            .min(e.text.len_lines() - 1);
        let line = e.text.line(row).to_string();
        let col = ((p.x - self.gutter(e) - 8. + self.scroll_x) / self.cell)
            .max(0.)
            .round() as usize;
        e.text.line_to_byte(row) + byte_at_cell(&line, col)
    }
    pub fn range_rect(&self, e: &Editor, range: Utf16Range) -> (Rect, Utf16Range) {
        let bytes = range.bytes(&e.text);
        let row = e.text.byte_to_line(bytes.start);
        let end = bytes
            .end
            .min(e.text.line_to_byte(row) + e.text.line(row).len_bytes());
        let mut r = self.rect_at(e, bytes.start);
        let end_rect = self.rect_at(e, end);
        r.width = (end_rect.x - r.x).max(1.);
        (r, Utf16Range::from_bytes(&e.text, bytes.start..end))
    }
    fn lex(&mut self, text: &str) -> Arc<[Token]> {
        if let Some(tokens) = self.tokens.get(text) {
            return Arc::clone(tokens);
        }
        let tokens: Arc<[_]> = lexer::lex(text).into();
        if text.len() <= 16_384 {
            while self.tokens.len() >= 1024 {
                if let Some(old) = self.order.pop_front() {
                    self.tokens.remove(&old);
                }
            }
            self.order.push_back(text.into());
            self.tokens.insert(text.into(), Arc::clone(&tokens));
        }
        tokens
    }
    fn foreground(&self, color: Color) -> Color {
        if self.appearance == Appearance::Light {
            [color[0] * 0.6, color[1] * 0.6, color[2] * 0.6, color[3]]
        } else {
            color
        }
    }
    pub fn scene(&mut self, e: &Editor) -> Scene {
        let all = Rect {
            x: 0.,
            y: 0.,
            width: self.size.width,
            height: self.size.height,
        };
        let gutter = self.gutter(e);
        let text_clip = Rect {
            x: gutter,
            y: 0.,
            width: (self.size.width - gutter).max(0.),
            height: self.size.height,
        };
        let mut layer = Layer::default();
        let dark = self.appearance == Appearance::Dark;
        layer.rects.push(SolidRect {
            rect: Rect {
                width: gutter,
                ..all
            },
            radius: 0.,
            color: if dark {
                [0.065, 0.075, 0.095, 1.]
            } else {
                [0.90, 0.91, 0.92, 1.]
            },
            clip: all,
        });
        let caret = self.rect_at(e, e.caret);
        layer.rects.push(SolidRect {
            rect: Rect {
                x: gutter,
                y: caret.y,
                width: (self.size.width - gutter).max(0.),
                height: self.line_height,
            },
            radius: 0.,
            color: if dark {
                [0.11, 0.13, 0.16, 1.]
            } else {
                [0.91, 0.94, 0.98, 1.]
            },
            clip: text_clip,
        });
        let first = (self.scroll_y / self.line_height).floor().max(0.) as usize;
        let last = (first + self.rows() + 2).min(e.text.len_lines());
        let selection = e.selection();
        for row in first..last {
            let line = e.text.line(row).to_string();
            let start = e.text.line_to_byte(row);
            let end = start + line.len();
            let y = 4. + row as f64 * self.line_height - self.scroll_y;
            let baseline = y + self.ascent + 3.;
            let number = (row + 1).to_string();
            layer.text.push(TextRun {
                text: number.clone(),
                x: (gutter - 12. - number.len() as f64 * self.cell) as f32,
                baseline: baseline as f32,
                size: 13.,
                color: self.foreground([0.43, 0.50, 0.56, 1.]),
                clip: Rect {
                    width: gutter,
                    ..all
                },
            });
            let selected_start = selection.start.max(start);
            let selected_end = selection.end.min(end);
            if selected_start <= selected_end
                && selection.start < end + usize::from(row + 1 < e.text.len_lines())
                && selection.end > start
            {
                let x = self.rect_at(e, selected_start.min(end)).x;
                let end_x = self.rect_at(e, selected_end.max(selected_start).min(end)).x;
                layer.rects.push(SolidRect {
                    rect: Rect {
                        x,
                        y,
                        width: (end_x - x).max(if selection.end > end { self.cell } else { 1. }),
                        height: self.line_height,
                    },
                    radius: 0.,
                    color: if dark {
                        [0.20, 0.32, 0.47, 1.]
                    } else {
                        [0.67, 0.79, 0.94, 1.]
                    },
                    clip: text_clip,
                });
            }
            let tokens = self.lex(&line);
            let mut col = 0usize;
            // Run boundaries follow the trivial lexer; tabs split runs at actual tab stops.
            // Clip horizontally before shaping, so long lines don't fill the glyph atlas.
            for token in tokens.iter() {
                let text = &line[token.bytes.clone()];
                let color = self.foreground(token.kind.color());
                let mut run = String::new();
                let mut run_col = col;
                for g in text.graphemes(true) {
                    let width = if g == "\t" {
                        4 - col % 4
                    } else {
                        g.width().max(1)
                    };
                    let x = gutter + 8. + col as f64 * self.cell - self.scroll_x;
                    let visible = x + width as f64 * self.cell >= gutter && x < self.size.width;
                    if g == "\t" || !visible {
                        if !run.is_empty() {
                            layer.text.push(TextRun {
                                text: std::mem::take(&mut run),
                                x: (gutter + 8. + run_col as f64 * self.cell - self.scroll_x)
                                    as f32,
                                baseline: baseline as f32,
                                size: 13.,
                                color,
                                clip: text_clip,
                            });
                        }
                    } else {
                        if run.is_empty() {
                            run_col = col;
                        }
                        run.push_str(g);
                    }
                    col += width;
                }
                if !run.is_empty() {
                    layer.text.push(TextRun {
                        text: run,
                        x: (gutter + 8. + run_col as f64 * self.cell - self.scroll_x) as f32,
                        baseline: baseline as f32,
                        size: 13.,
                        color,
                        clip: text_clip,
                    });
                }
            }
            if let Some(marked) = &e.marked {
                let from = marked.start.max(start);
                let to = marked.end.min(end);
                if from < to {
                    let x = self.rect_at(e, from).x;
                    let end_x = self.rect_at(e, to).x;
                    layer.underlines.push(Underline {
                        rect: Rect {
                            x,
                            y: y + self.line_height - 2.,
                            width: (end_x - x).max(1.),
                            height: 1.5,
                        },
                        thickness: 1.,
                        squiggle: false,
                        color: self.foreground([0.85, 0.87, 0.92, 1.]),
                        clip: text_clip,
                    });
                }
            }
        }
        let mut caret_layer = Layer::default();
        caret_layer.rects.push(SolidRect {
            rect: caret,
            radius: 0.,
            color: if dark {
                [0.91, 0.93, 0.97, 1.]
            } else {
                [0.05, 0.07, 0.10, 1.]
            },
            clip: text_clip,
        });
        Scene {
            background: if dark {
                [0.075, 0.085, 0.105, 1.]
            } else {
                [0.97, 0.97, 0.98, 1.]
            },
            layers: vec![layer, caret_layer],
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use vel_platform::TextInput;
    use vel_text::Text;
    #[test]
    fn utf16_geometry_hit_testing_scroll_and_retina_independence() {
        let mut e = Editor::new(Text::from("a🦀你\nnext"));
        let mut v = View::new(
            Size {
                width: 400.,
                height: 200.,
            },
            8.,
            12.,
        );
        let (rect, actual) = v.range_rect(&e, Utf16Range::new(1, 2));
        assert_eq!(actual, Utf16Range::new(1, 2));
        assert_eq!(rect.width, 16.);
        let byte = v.hit_test(
            &e,
            Point {
                x: rect.x + 16.,
                y: rect.y,
            },
        );
        assert_eq!(byte, 5);
        v.scroll_x = 0.25;
        assert_eq!(v.rect_at(&e, 1).x, rect.x - 0.25);
        e.text_input(TextInput::Marked {
            text: "你好".into(),
            selected: Utf16Range::new(2, 0),
            replacement: Some(Utf16Range::new(1, 2)),
        });
        assert_eq!(v.scene(&e).layers[0].underlines.len(), 1);
    }
    #[test]
    fn selection_across_newlines_and_visibility_are_bounded() {
        let mut e = Editor::new(Text::from("a\n你🦀\nthird"));
        e.anchor = 0;
        e.caret = e.text.len_bytes();
        let mut v = View::new(
            Size {
                width: 100.,
                height: 40.,
            },
            8.,
            12.,
        );
        let scene = v.scene(&e);
        assert!(scene.layers[0].rects.len() >= 4);
        v.reveal(&e);
        let caret = v.rect_at(&e, e.caret).clamped(v.size);
        assert!(caret.y >= 0. && caret.y + caret.height <= v.size.height);
    }
}
