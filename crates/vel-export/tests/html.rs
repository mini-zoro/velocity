use std::sync::Arc;
use vel_buffer::{Region, Selection};
use vel_color_scheme::{Color, ColorScheme, FontStyle};
use vel_export::{Error, Options, copy_as_html, export_range, write_print_file};
use vel_highlight::{StyleSpan, StyledLine};
use vel_text::Text;

fn scheme() -> ColorScheme {
    ColorScheme::parse(r##"{"name":"Export test","globals":{"foreground":"#102030","background":"#fafbfc","gutter_foreground":"#556677"}}"##).unwrap()
}
#[test]
fn golden_full_document_and_print_artifact() {
    let scheme = scheme();
    let text = Text::from("<a>&\"'\t猫\n🦀");
    let mut style = scheme.default_style();
    style.foreground = Color::rgb(255, 0, 0);
    style.font_style = FontStyle::parse("bold italic underline").unwrap();
    let lines = vec![StyledLine {
        line: 0,
        exact: true,
        spans: Arc::from([StyleSpan { range: 0..3, style }]),
    }];
    let options = Options {
        title: "<&>".into(),
        line_numbers: true,
        ..Options::default()
    };
    let export = export_range(&text, 0..text.len_bytes(), &lines, &scheme, &options).unwrap();
    assert_eq!(export.plain_text, "<a>&\"'\t猫\n🦀");
    assert_eq!(export.html, include_str!("golden/highlighted.html"));
    let path = write_print_file(&export).unwrap();
    assert_eq!(path.extension().unwrap(), "html");
    // Check what the browser will actually receive, after the writer has dropped.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), export.html);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn golden_selections_clip_spans_and_preserve_plain_tabs() {
    let text = Text::from("zero\n\t猫e\u{301}\t<&\nlast");
    let scheme = scheme();
    let selection = Selection::from_regions([Region::new(12, 6), Region::new(20, 16)]);
    let export = copy_as_html(&text, &selection, &[], &scheme, &Options::default()).unwrap();
    assert_eq!(export.plain_text, "猫e\u{301}\nlast");
    assert_eq!(export.html, include_str!("golden/selections.html"));
    let tabs = export_range(&text, 5..16, &[], &scheme, &Options::default()).unwrap();
    assert!(tabs.html.contains("    猫e\u{301} &lt;&amp;\n"));
    assert_eq!(tabs.plain_text, "\t猫e\u{301}\t<&\n");
}
#[test]
fn all_carets_copy_distinct_lines_and_mixed_selections_ignore_carets() {
    let text = Text::from("a\nb\n");
    let scheme = scheme();
    let export = copy_as_html(
        &text,
        &Selection::from_regions([Region::caret(0), Region::caret(1), Region::caret(2)]),
        &[],
        &scheme,
        &Options::default(),
    )
    .unwrap();
    assert_eq!(export.plain_text, "a\nb\n");
    let mixed = copy_as_html(
        &text,
        &Selection::from_regions([Region::new(0, 1), Region::caret(4)]),
        &[],
        &scheme,
        &Options::default(),
    )
    .unwrap();
    assert_eq!(mixed.plain_text, "a");
}
#[test]
fn hostile_title_font_and_source_cannot_break_out_of_html_or_css() {
    let source = "</pre><script>alert('&')</script>\r\0";
    let text = Text::from(source);
    let scheme = scheme();
    let options = Options {
        title: "</title><script>".into(),
        font_family: "\";}</style><script>\n\\".into(),
        ..Options::default()
    };
    let export = export_range(&text, 0..text.len_bytes(), &[], &scheme, &options).unwrap();
    assert_eq!(export.html.matches("</style>").count(), 1);
    assert_eq!(export.html.matches("</title>").count(), 1);
    assert!(!export.html.contains("<script>"));
    assert!(export.html.contains("&#13;&#xfffd;"));
    assert_eq!(export.plain_text, source);
}
#[test]
fn invalid_ranges_options_and_spans_return_errors() {
    let text = Text::from("猫\n");
    let scheme = scheme();
    let options = Options::default();
    for range in [1..3, 0..9, std::ops::Range { start: 3, end: 2 }] {
        assert_eq!(
            export_range(&text, range, &[], &scheme, &options),
            Err(Error::Range)
        );
    }
    for range in [1..3, 0..9, std::ops::Range { start: 3, end: 2 }] {
        let lines = [StyledLine {
            line: 0,
            exact: true,
            spans: Arc::from([StyleSpan {
                range,
                style: scheme.default_style(),
            }]),
        }];
        assert_eq!(
            export_range(&text, 0..3, &lines, &scheme, &options),
            Err(Error::Highlights)
        );
    }
    let options = Options {
        font_size: f32::NAN,
        ..options
    };
    assert_eq!(
        export_range(&text, 0..3, &[], &scheme, &options),
        Err(Error::Options)
    );
    let export = export_range(&text, 4..4, &[], &scheme, &Options::default()).unwrap();
    assert!(export.plain_text.is_empty());
}
#[test]
fn absent_or_approximate_spans_fall_back_and_alpha_decorations_survive() {
    let text = Text::from("a\tb");
    let scheme = scheme();
    let mut style = scheme.default_style();
    style.foreground.a = 128;
    style.font_style = FontStyle::parse("squiggly_underline glow").unwrap();
    let line = StyledLine {
        line: 0,
        exact: true,
        spans: Arc::from([StyleSpan { range: 1..2, style }]),
    };
    let export = export_range(
        &text,
        0..4,
        std::slice::from_ref(&line),
        &scheme,
        &Options::default(),
    );
    assert_eq!(export, Err(Error::Range));
    let export = export_range(
        &text,
        0..3,
        std::slice::from_ref(&line),
        &scheme,
        &Options::default(),
    )
    .unwrap();
    assert!(export.html.contains("#10203080"));
    assert!(export.html.contains("text-decoration:underline wavy;"));
    let line = StyledLine {
        exact: false,
        ..line
    };
    let export = export_range(&text, 0..3, &[line], &scheme, &Options::default()).unwrap();
    assert!(!export.html.contains("#10203080"));
}

#[test]
fn clipped_styles_and_malformed_span_order_are_checked() {
    let text = Text::from("<猫>&");
    let scheme = scheme();
    let mut style = scheme.default_style();
    style.foreground = Color::rgb(255, 0, 0);
    let line = StyledLine {
        line: 0,
        exact: true,
        spans: Arc::from([StyleSpan { range: 0..5, style }]),
    };
    let export = export_range(
        &text,
        1..4,
        std::slice::from_ref(&line),
        &scheme,
        &Options::default(),
    )
    .unwrap();
    assert_eq!(export.plain_text, "猫");
    assert!(export.html.contains("<span class=\"s0\">猫</span>"));
    assert!(export.html.contains("color:#ff0000"));
    assert_eq!(
        export_range(
            &text,
            0..6,
            &[line.clone(), line],
            &scheme,
            &Options::default()
        ),
        Err(Error::Highlights)
    );
    let line = StyledLine {
        line: 0,
        exact: true,
        spans: Arc::from([
            StyleSpan { range: 0..5, style },
            StyleSpan { range: 4..6, style },
        ]),
    };
    assert_eq!(
        export_range(&text, 0..6, &[line], &scheme, &Options::default()),
        Err(Error::Highlights)
    );
}
