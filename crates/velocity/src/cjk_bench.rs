//! Deterministic CPU typing and scrolling on a 10,000-line Chinese document.
use crate::{editor::Editor, view::View};
use std::{fmt::Write, hint::black_box};
use vel_config::settings::Layer;
use vel_platform::Size;
use vel_render::font::Fonts;
use vel_text::Text;

pub fn run() -> Result<(), String> {
    let mut text = String::new();
    for row in 0..10_000 {
        if row != 0 {
            text.push('\n');
        }
        write!(
            text,
            "第{:03}行：小雨已经停了，街角的书店亮起了灯。路过的人推开门，在窗前读完了这一页。",
            row % 96
        )
        .expect("writing to a String");
    }
    let mut editor = Editor::new(Text::from(text.as_str()));
    editor.settings.set(Layer::View, "word_wrap", false.into());
    editor.sync();
    let mut fonts = Fonts::load()?;
    let mut view = View::new(
        Size {
            width: 1200.,
            height: 800.,
        },
        fonts.cell.into(),
        fonts.ascent.into(),
    );
    view.set_font_metrics(fonts.metrics, 2.);
    view.set_wide_advances(fonts.wide_advances());
    let at = editor.text.line_to_byte(5_000);
    editor.set_caret(at, false);
    view.center_caret(&editor);
    black_box(view.scene(&editor));
    for _ in 0..200 {
        crate::type_through_keymap(&mut editor, "字")?;
        view.reveal(&editor);
        black_box(view.scene(&editor));
    }
    for frame in 0..240 {
        let vertical = if frame < 120 { 41. } else { -41. };
        view.scroll(&editor, 0., vertical);
        black_box(view.scene(&editor));
    }
    assert_eq!(editor.text.len_lines(), 10_000);
    assert_eq!(editor.caret, at + "字".len() * 200);
    println!(
        "{}",
        serde_json::json!({"scenario":"cjk", "lines":10000, "typing_frames":200, "scroll_frames":240, "wrap":false})
    );
    Ok(())
}
