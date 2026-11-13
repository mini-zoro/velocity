//! Cross-module regressions; these tests do not instantiate winit or wgpu.
use crate::{
    editor::{Cursor, Editor, Motion},
    font::Fonts,
};
use unicode_segmentation::UnicodeSegmentation;

#[test]
fn instanced_shader_parses_and_validates_without_a_device() {
    let module = naga::front::wgsl::parse_str(include_str!("quads.wgsl")).expect("WGSL parse");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .expect("WGSL validation");
}

#[test]
fn pinyin_conversion_cancellation_and_following_english_do_not_duplicate_text() {
    let mut e = Editor::from_text("prefix suffix");
    e.place_cursor(Cursor { line: 0, byte: 7 }, false);
    e.ime.enabled = true;
    for (text, range) in [
        ("n", Some((1, 1))),
        ("nihao", Some((5, 5))),
        ("你好", Some((0, 6))),
        ("", None),
    ] {
        e.ime.update(text.into(), range);
        assert_eq!(e.lines[0], "prefix suffix");
    }
    e.commit_ime("你好");
    e.insert("x");
    assert_eq!(e.lines[0], "prefix 你好xsuffix");
    e.ime.update("shi".into(), Some((3, 3)));
    e.ime.clear();
    e.insert("y");
    assert_eq!(e.lines[0], "prefix 你好xysuffix");
}

#[test]
fn unicode_edits_match_a_flat_string_reference_over_many_transactions() {
    let mut rng = 0xabcdef0123456789u64;
    for _ in 0..20 {
        let mut e = Editor::from_text("");
        let mut model = String::new();
        let mut caret = 0;
        for _ in 0..300 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            match rng % 7 {
                0..=3 => {
                    let choices = ["x", "你", "🦀", "e\u{301}", "\n", "a\r\nb"];
                    let text = choices[((rng >> 32) as usize) % choices.len()];
                    let normalized = text.replace("\r\n", "\n");
                    e.insert(text);
                    model.insert_str(caret, &normalized);
                    caret += normalized.len();
                }
                4 => {
                    e.backspace();
                    if caret > 0 {
                        let previous = model[..caret].grapheme_indices(true).next_back().unwrap().0;
                        model.replace_range(previous..caret, "");
                        caret = previous;
                    }
                }
                5 => {
                    e.move_cursor(Motion::Left, false);
                    if caret > 0 {
                        caret = model[..caret].grapheme_indices(true).next_back().unwrap().0;
                    }
                }
                _ => {
                    e.move_cursor(Motion::Right, false);
                    if caret < model.len() {
                        caret += model[caret..].graphemes(true).next().unwrap().len();
                    }
                }
            }
            assert_eq!(e.lines.join("\n"), model);
            let actual = e.lines[..e.cursor.line]
                .iter()
                .map(|l| l.len() + 1)
                .sum::<usize>()
                + e.cursor.byte;
            assert_eq!(actual, caret);
            assert!(model.is_char_boundary(caret));
        }
    }
}

#[test]
fn fixture_load_cap_crlf_and_last_line_semantics() {
    let path = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/ime-and-unicode.rs"
    ));
    let capped = Editor::load(Some(path), 5).unwrap();
    assert_eq!(capped.lines.len(), 5);
    assert!(capped.truncated);
    let full = Editor::load(Some(path), 0).unwrap();
    assert!(!full.truncated);
    assert!(full.lines.iter().any(|l| l.contains("你好")));
    assert_eq!(full.lines.last().unwrap(), "");
    let e = Editor::from_text("a\r\n你\r\n");
    assert_eq!(e.lines, vec!["a", "你", ""]);
}

#[test]
fn cjk_and_emoji_fallback_can_shape_and_rasterize_headlessly() {
    let mut fonts = Fonts::load().expect("system Menlo");
    for text in ["你", "好", "😀"] {
        let glyphs = fonts.shape(text, 26.);
        assert!(!glyphs.is_empty(), "{text}");
        for g in glyphs {
            assert_ne!(g.id, 0, "{text}: {}", fonts.metadata());
            let image = fonts.rasterize(g.face, g.id, 26., 0).unwrap_or_else(|| {
                panic!("no raster for {text} glyph {g:?}: {}", fonts.metadata())
            });
            assert!(image.placement.width > 0);
        }
    }
}
