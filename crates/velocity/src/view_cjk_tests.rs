use super::*;
use crate::mouse_select::{Combine, DragSelect, Unit};
use vel_buffer::{ChangeId, Region, Selection};
use vel_config::{json, settings::Layer as SettingsLayer};

const CASES: [(&str, f64, usize); 3] = [("中文", 24., 4), ("A中", 20., 3), ("👩‍💻🦀", 36., 4)];

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 0.02, "{a} != {b}");
}

fn fixture(source: &str, wrap: bool, width: f64) -> (Editor, View) {
    let mut e = Editor::new(Text::from(source));
    e.settings
        .set(SettingsLayer::View, "word_wrap", json!(wrap));
    e.settings.set(SettingsLayer::View, "wrap_width", json!(12));
    e.settings.set(SettingsLayer::View, "minimap", json!(false));
    e.sync();
    let mut v = View::new(
        Size {
            width,
            height: 230.,
        },
        8.,
        12.,
    );
    v.set_wide_advances(WideAdvances::new([
        1.5, 1.5, 1.5, 1.5, 1.5, 2.25, 2.25, 1.5,
    ]));
    v.apply_style(&e);
    (e, v)
}

fn point(v: &View, e: &Editor, at: usize, dx: f64) -> Point {
    let r = v.rect_at(e, at);
    Point {
        x: r.x + dx,
        y: r.y + r.height / 2.,
    }
}

#[test]
fn cjk_carets_hit_drag_and_utf16_ranges() {
    for (prefix, width, columns) in CASES {
        let source = format!("{prefix} tag\n{prefix} tag");
        let (mut e, mut v) = fixture(&source, false, 700.);
        let left = v.rect_at(&e, 0).x;
        close(v.rect_at(&e, prefix.len()).x - left, width);
        assert_eq!(e.layout.borrow().column(&e.text, prefix.len()), columns);
        for (a, cluster) in
            unicode_segmentation::UnicodeSegmentation::grapheme_indices(prefix, true)
        {
            let b = a + cluster.len();
            let dx = v.rect_at(&e, b).x - v.rect_at(&e, a).x;
            assert_eq!(v.hit_test(&e, point(&v, &e, a, dx * 0.25)), a);
            assert_eq!(v.hit_test(&e, point(&v, &e, a, dx * 0.75)), b);
            let request = Utf16Range::from_bytes(&e.text, a..b);
            let (r, actual) = v.range_rect(&e, request);
            close(r.width, dx);
            assert_eq!(actual, request);
        }
        let tag = prefix.len() + 1;
        let start = point(&v, &e, tag, 8. * 0.75);
        let at = v.hit_test(&e, start);
        let mut drag = DragSelect::begin(
            &v,
            &mut e,
            start,
            at,
            Unit::Word,
            Combine::Replace,
            false,
            false,
        );
        assert_eq!(e.regions()[0].to_range(), tag..tag + 3);
        let last = e.text.line_to_byte(1) + tag;
        let last_point = point(&v, &e, last, 1.);
        drag.update(&v, &mut e, last_point);
        assert_eq!(e.regions()[0].to_range(), tag..last + 3);

        e.set_caret(prefix.len(), false);
        let r = v.rect_at(&e, prefix.len());
        let scene = v.scene(&e);
        assert!(
            scene.layers.iter().flat_map(|l| &l.rects).any(|q| {
                (q.rect.x - r.x).abs() < 0.02 && q.rect.y == r.y && q.rect.width == 1.
            })
        );

        let wide = prefix.find(|c: char| !c.is_ascii()).unwrap();
        let next = e.text.next_grapheme_boundary(wide);
        let inside = if prefix.starts_with('👩') {
            wide + '👩'.len_utf8()
        } else {
            wide
        };
        e.set_selection(Selection::from_regions([Region::new(
            inside,
            e.text.line_to_byte(1) + next,
        )]));
        let scene = v.scene(&e);
        let r = v.rect_at(&e, wide);
        let end = v.rect_at(&e, e.text.line_to_byte(1) + next);
        let selection = color(fallback_scheme(v.appearance), "selection");
        assert!(
            scene.layers[0]
                .rects
                .iter()
                .any(|q| q.rect.y == r.y && (q.rect.x - r.x).abs() < 0.02 && q.color == selection)
                || !scene.layers[0].contours.is_empty(),
            "{prefix:?}"
        );
        assert!(end.y > r.y && end.x > left);
    }
}

#[test]
fn cjk_option_drag_keeps_logical_columns() {
    let source = "中文后续\nA中bc\n👩‍💻🦀abc";
    let (mut e, v) = fixture(source, false, 700.);
    let a = point(&v, &e, "中".len(), 1.5);
    let b_at = e.text.line_to_byte(2) + "👩‍💻🦀".len();
    let b = point(&v, &e, b_at, 0.);
    close(v.logical_column_at(&e, a), 2.25);
    close(v.logical_column_at(&e, b), 4.);
    let at = v.hit_test(&e, a);
    let mut drag = DragSelect::begin(&v, &mut e, a, at, Unit::Char, Combine::Replace, false, true);
    drag.update(&v, &mut e, b);
    let selected: Vec<_> = e
        .regions()
        .into_iter()
        .map(|r| e.text.slice(r.to_range()).to_string())
        .collect();
    assert_eq!(selected, ["文", "b", "🦀"]);
}

#[test]
fn cjk_widest_line_reveal_and_vertical_overscroll() {
    for (prefix, width, _) in CASES {
        let body = prefix.repeat(80);
        let text = format!("{body}\n{}", format!("{prefix}\n").repeat(45));
        let (mut e, mut v) = fixture(&text, false, 380.);
        e.settings
            .set(SettingsLayer::View, "scroll_past_end", json!(1.));
        e.sync();
        v.scene(&e);
        v.scroll(&e, 100_000., 0.);
        let visible = v.right(&e) - v.gutter(&e) - v.margin(&e) * 2.;
        close(v.scroll_x, width * 80. + v.cell - visible);
        e.set_caret(body.len(), false);
        v.scroll(&e, -100_000., 0.);
        v.reveal(&e);
        close(
            v.rect_at(&e, body.len()).x + v.cell,
            v.right(&e) - v.margin(&e),
        );
        v.scroll(&e, 0., 100_000.);
        let last = v.rect_at(&e, e.text.len_bytes());
        close(last.y, v.tab_height + 4.);
    }
}

#[test]
fn cjk_adjusted_break_shares_caret_motion_selection_and_hit_testing() {
    let source = "甲乙。丙丁";
    let (mut e, mut v) = fixture(source, true, 550.);
    e.settings.set(SettingsLayer::View, "wrap_width", json!(3));
    e.sync();
    v.apply_style(&e);
    let first = "甲".len();
    let punctuation = "甲乙".len();
    let third = "甲乙。".len();
    let paint = v.rect_at(&e, first);
    assert!(paint.y > v.rect_at(&e, 0).y);
    let start = Point {
        x: paint.x,
        y: paint.y + paint.height / 2.,
    };
    assert_eq!(v.hit_test(&e, start), first);
    assert_eq!(v.hit_test(&e, point(&v, &e, punctuation, 1.)), punctuation);
    assert_eq!(
        v.hit_test(&e, point(&v, &e, punctuation, 11.)),
        third,
        "right half of the closing punctuation hits the byte after it"
    );
    e.set_caret(0, false);
    for (by, forward, expected) in [
        ("characters", true, first),
        ("characters", true, punctuation),
        ("characters", true, third),
        ("characters", false, punctuation),
    ] {
        e.run("move", &json!({"by":by,"forward":forward})).unwrap();
        assert_eq!(e.caret, expected, "{by} {forward}");
        assert!(e.text.is_char_boundary(e.caret));
    }
    e.set_caret(first, false);
    for (forward, expected) in [(false, 0), (true, first), (true, third)] {
        e.run("move", &json!({"by":"lines","forward":forward}))
            .unwrap();
        assert_eq!(e.caret, expected);
    }
    let (r, range) = v.range_rect(&e, Utf16Range::from_bytes(&e.text, first..third));
    close(r.y, paint.y);
    close(r.width, 24.);
    assert_eq!(range, Utf16Range::from_bytes(&e.text, first..third));
}

#[test]
fn cjk_wrap_tabs_invisibles_guides_and_rulers() {
    for (prefix, width, columns) in CASES {
        let (mut e, mut v) = fixture(&format!("{prefix}\t x"), false, 550.);
        e.settings
            .set(SettingsLayer::View, "draw_white_space", json!("all"));
        e.settings.set(SettingsLayer::View, "rulers", json!([12]));
        e.settings.set(SettingsLayer::View, "tab_size", json!(4));
        e.sync();
        let scene = v.scene(&e);
        let left = v.rect_at(&e, 0).x;
        let at = v.rect_at(&e, prefix.len() + 1).x;
        close(at - left, width + (4 - columns % 4) as f64 * 8.);
        let dot = scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .find(|r| r.text == "·")
            .unwrap();
        close(f64::from(dot.x), at);
        assert!(
            scene.layers[0]
                .rects
                .iter()
                .any(|r| (r.rect.x - left - 96.).abs() < 0.02)
        );

        let text = format!("     {}", prefix.repeat(20));
        let (e, mut v) = fixture(&text, true, 600.);
        let scene = v.scene(&e);
        let layout = e.layout.borrow();
        let first = layout.segment(&e.text, 0, 0);
        let next = layout.segment(&e.text, 0, 1);
        assert!(
            first.bytes.end > 5,
            "indent is not a line of its own: {prefix}"
        );
        assert!(next.bytes.start == first.bytes.end && next.indent == 5);
        let x = layout.segment_x(&e.text, 0, &first, first.bytes.end, v.cell, &mut None);
        assert!((96. - 18. ..=96.).contains(&x));
        let (r, _) = v.range_rect(&e, Utf16Range::from_bytes(&e.text, 5..first.bytes.end));
        close(r.width, x - 40.);
        assert!(scene.layers[0].rects.iter().any(|r| r.rect.width <= 1. && (r.rect.x - (v.rect_at(&e, 0).x + 32.)).abs() < 0.02));
    }
}

#[test]
fn cjk_highlights_phantom_and_preedit_use_measured_endpoints() {
    for (prefix, width, _) in CASES {
        let source = format!("{prefix} typo typo");
        let (mut e, mut v) = fixture(&source, false, 620.);
        e.shared_shadow = true;
        let a = prefix.len() + 1;
        let b = a + 4;
        v.find_matches.push(a..b);
        e.settings
            .set(SettingsLayer::View, "spell_check", json!(true));
        e.sync();
        e.annotations.borrow_mut().current = Some(crate::annotations::Publication {
            identity: (e.buffer.id(), ChangeId(e.revision)),
            epoch: 0,
            reference: e.text.clone(),
            hunks: Arc::from([vel_diff::Hunk {
                old: 0..1,
                new: 0..1,
            }]),
            spelling: Arc::from([vel_spell::Misspelling {
                word: "typo".into(),
                range: a..b,
            }]),
            inline_spans: [(
                0,
                Arc::<[vel_diff::InlineChange]>::from([vel_diff::InlineChange {
                    old: a..b,
                    new: a..b,
                }]),
            )]
            .into(),
            navigation: None,
            checked: 0..1,
            suggestions: None,
            error: None,
        });
        e.annotations.borrow_mut().inline.insert(0);
        let scene = v.scene(&e);
        let x = v.rect_at(&e, a).x;
        close(x - v.rect_at(&e, 0).x, width + v.cell);
        let find = color(fallback_scheme(v.appearance), "find_highlight");
        assert!(
            scene.layers[0]
                .rects
                .iter()
                .any(|r| (r.rect.x - x).abs() < 0.02 && r.color == find)
        );
        let mut diff = color(fallback_scheme(v.appearance), "line_diff_added");
        diff[3] = 0.25;
        assert!(scene.layers[0].rects.iter().any(|r| {
            r.color == diff && (r.rect.x - x).abs() < 0.02 && (r.rect.width - 32.).abs() < 0.02
        }));
        e.set_selection(Selection::from_regions([Region::new(a, b)]));
        v.find_matches.clear();
        let scene = v.scene(&e);
        let second = v.rect_at(&e, b + 1);
        assert!(scene.layers[0].rects.iter().any(|r| {
            r.is_outline()
                && (r.rect.x - second.x).abs() < 0.02
                && (r.rect.width - 32.).abs() < 0.02
        }));
        e.set_caret(a, false);
        assert!(
            scene.layers[0]
                .underlines
                .iter()
                .any(|r| r.squiggle && (r.rect.x - x).abs() < 0.02 && r.rect.width == 32.)
        );

        let _ = e.run(
            "add_phantoms",
            &json!({"key":"cjk","items":[{"at":a,"layout":"inline","content":"<b>hint</b>"}]}),
        );
        v.scene(&e);
        assert!(v.rect_at(&e, b).x > x + 32.);

        let request = Utf16Range::from_bytes(&e.text, a..b);
        e.text_input(vel_platform::TextInput::Marked {
            text: prefix.into(),
            selected: Utf16Range::new(prefix.encode_utf16().count(), 0),
            replacement: Some(request),
        });
        let scene = v.scene(&e);
        let range = e.marked_utf16().unwrap();
        let (rect, actual) = v.range_rect(&e, range);
        assert_eq!(actual, range);
        close(v.rect_at(&e, e.caret).x, rect.x + rect.width);
        assert!(scene.layers[0].underlines.iter().any(|r| {
            !r.squiggle
                && (r.rect.x - rect.x).abs() < 0.02
                && (r.rect.width - rect.width).abs() < 0.02
        }));
    }
}

#[test]
fn cjk_brackets_and_minimap_use_measured_columns() {
    let assets = crate::language::Assets::bundled().unwrap();
    for (prefix, width, columns) in CASES {
        let source = format!("{prefix} (token)\n");
        let (mut e, mut v) = fixture(&source, false, 700.);
        e.settings.set(SettingsLayer::View, "minimap", json!(true));
        e.settings
            .set(SettingsLayer::View, "match_brackets", json!(true));
        let scheme = Arc::new(ColorScheme::velocity_dark());
        e.attach_language(assets.clone(), "Plain Text", scheme)
            .unwrap();
        e.sync();
        let open = prefix.len() + 1;
        e.set_caret(open, false);
        let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
        loop {
            e.poll_language();
            if e.language
                .borrow()
                .as_ref()
                .is_some_and(|l| l.analysis.is_some())
            {
                break;
            }
            assert!(Instant::now() < deadline, "syntax analysis did not settle");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let marker = [0.11, 0.22, 0.33, 1.];
        e.language.borrow_mut().as_mut().unwrap().minimap =
            Arc::from([crate::language::MiniLine {
                row: 0,
                tokens: vec![crate::language::MiniToken {
                    column: columns + 1,
                    width: 1,
                    color: marker,
                }],
                non_ascii: true,
            }]);
        let scene = v.scene(&e);
        let x = v.rect_at(&e, open).x;
        assert!(
            scene.layers[0]
                .underlines
                .iter()
                .any(|r| { (r.rect.x - x).abs() < 0.02 && (r.rect.width - v.cell).abs() < 0.02 }),
            "bracket underline for {prefix}"
        );
        let rect = scene
            .layers
            .iter()
            .filter(|l| l.minimap)
            .flat_map(|l| &l.rects)
            .find(|r| r.color == marker)
            .unwrap()
            .rect;
        close(
            rect.x - v.minimap_geometry(&e).bounds.x,
            (width / 8. + 1.) * 0.65,
        );
    }
}

#[test]
fn cjk_replace_all_keeps_popup_at_the_measured_caret() {
    for (prefix, width, _) in CASES {
        let source = format!("{prefix} tag {prefix} tag");
        let mut w =
            crate::workspace::Workspace::new(Text::from(source.as_str()), None, None, false)
                .unwrap();
        let mut panels = crate::panels::Panels::new(&w, false);
        panels.query.set("tag");
        panels.replace.set("test");
        panels
            .action(&mut w, "replace_all", &json!({}), 0..source.len())
            .unwrap();
        assert_eq!(
            w.editor().text.to_string(),
            format!("{prefix} test {prefix} test")
        );
        w.editor_mut().set_caret(prefix.len() + 1, false);
        let (_, mut v) = fixture("", false, 700.);
        v.scene(w.editor());
        let caret = v.rect_at(w.editor(), w.editor().caret);
        let (x, y) = (caret.x, caret.y + caret.height);
        let area = Rect {
            x: 0.,
            y: 0.,
            width: 700.,
            height: 230.,
        };
        let popup = crate::completion::popup_bounds(caret, area, 100., 40.);
        close(x - v.rect_at(w.editor(), 0).x, width + 8.);
        assert_eq!((popup.x, popup.y), (x, y));
    }
}

#[test]
fn cjk_large_policy_retains_logical_grid() {
    for (prefix, _, columns) in CASES {
        let (mut e, mut v) = fixture(prefix, false, 700.);
        e.features.indexing = false;
        v.style_inputs = None;
        let scene = v.scene(&e);
        close(
            v.rect_at(&e, prefix.len()).x - v.rect_at(&e, 0).x,
            columns as f64 * 8.,
        );
        assert!(scene.layers.iter().all(|l| l.wide_text.is_none()));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn cjk_native_ink_gaps_and_caret_match_natural_advance() {
    use vel_render::{atlas::subpixel_x, font::Fonts};
    let mut fonts = Fonts::load().unwrap();
    let advances = fonts.wide_advances();
    for scale in [1., 2.] {
        let source = "一個小型任務";
        let px = 13. * scale;
        let mut measured = fonts.shape_editor(source, px, FontStyle::default(), advances);
        let mut natural = fonts.shape_ui(source, "Menlo", px, FontStyle::default());
        let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
        while fonts.fallback_pending() {
            assert!(Instant::now() < deadline);
            if fonts.poll_fallbacks() {
                measured = fonts.shape_editor(source, px, FontStyle::default(), advances);
                natural = fonts.shape_ui(source, "Menlo", px, FontStyle::default());
            } else {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert_eq!(measured.len(), source.chars().count());
        assert_eq!(natural.glyphs.len(), measured.len());
        let mut ink = |glyphs: &[vel_render::font::ShapedGlyph]| {
            glyphs
                .iter()
                .map(|g| {
                    assert_ne!(g.face, 0, "no CJK fallback face");
                    let (x, bucket) = subpixel_x(g.x);
                    let raster = fonts.rasterize(g.face, g.id, px, bucket).unwrap();
                    let left = x + raster.placement.left as f32;
                    (left, left + raster.placement.width as f32)
                })
                .collect::<Vec<_>>()
        };
        for (actual, expected) in ink(&measured)
            .windows(2)
            .zip(ink(&natural.glyphs).windows(2))
        {
            let gap = |pair: &[(f32, f32)]| pair[1].0 - pair[0].1;
            assert!(gap(actual) <= gap(expected) + 0.25);
        }
        let (e, mut v) = fixture(source, false, 700.);
        v.set_font_grid(fonts.cell.into(), fonts.ascent.into());
        v.set_wide_advances(advances);
        v.apply_style(&e);
        let x = v.rect_at(&e, 0).x;
        for (glyph, (byte, _)) in natural.glyphs.iter().zip(source.char_indices()) {
            close(v.rect_at(&e, byte).x - x, f64::from(glyph.x / scale));
        }
        close(
            v.rect_at(&e, source.len()).x - x,
            f64::from(natural.advance / scale),
        );
    }
}
