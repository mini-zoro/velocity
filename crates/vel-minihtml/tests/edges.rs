use vel_minihtml::*;
#[derive(Debug)]
struct Metrics;
impl FontMetrics for Metrics {
    fn measure(&self, text: &str, style: &TextStyle) -> TextMetrics {
        TextMetrics {
            advance: text.chars().count() as f32 * style.font_size * 0.5,
            ascent: style.font_size * 0.8,
            descent: style.font_size * 0.2,
        }
    }
    fn image_size(&self, _: &ImageSource) -> Option<Size> {
        Some(Size {
            width: 20.,
            height: 10.,
        })
    }
}
fn render(html: &str) -> DisplayList {
    layout(
        &Document::parse(html).unwrap(),
        &LayoutOptions {
            max_width: 100.,
            root_font_size: 10.,
            ..LayoutOptions::default()
        },
        &Metrics,
    )
}
fn text(out: &DisplayList, needle: &str) -> Rect {
    out.items
        .iter()
        .find_map(|i| {
            if let DisplayItem::Text { text, rect, .. } = i {
                (text == needle).then_some(*rect)
            } else {
                None
            }
        })
        .unwrap()
}
#[test]
fn descendant_selectors_backtrack_and_conjoined_classes_match() {
    let out = render(
        "<style>.a > .b .leaf {color:red} .x.y {font-weight:bold}</style><div class='a'><div class='b'><div><span class='b'><i class='leaf x y'>z</i></span></div></div></div>",
    );
    let s = out
        .items
        .iter()
        .find_map(|i| {
            if let DisplayItem::Text { style, .. } = i {
                Some(style)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(s.color, Color::rgb(255, 0, 0));
    assert_eq!(s.font_weight, 700);
}
#[test]
fn image_constraints_preserve_explicit_dimensions_and_percentage_heights() {
    for (html, w, h) in [
        (
            "<img src='res://x' style='width:40px;height:20px;max-width:10px'>",
            10.,
            20.,
        ),
        ("<img src='res://x' style='max-height:5px'>", 10., 5.),
        (
            "<div style='height:40px'><img src='res://x' style='height:50%'></div>",
            40.,
            20.,
        ),
        ("<img src='res://x' style='min-width:30px'>", 30., 15.),
    ] {
        let out = render(html);
        let r = out
            .items
            .iter()
            .find_map(|i| {
                if let DisplayItem::Image { rect, .. } = i {
                    Some(*rect)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!((r.width, r.height), (w, h));
    }
}
#[test]
fn css_invalid_values_keep_prior_declarations_and_scheme_css_overrides() {
    let doc=Document::parse("<body id='plugin'><style>span {padding-left:4px;padding-left:-2px;color:red} div{padding:3px;padding:-5px}</style><span>x</span><div>y</div></body>").unwrap();
    let out = layout(
        &doc,
        &LayoutOptions {
            max_width: 100.,
            root_font_size: 10.,
            extra_css: "#plugin span {color:blue}".into(),
            ..LayoutOptions::default()
        },
        &Metrics,
    );
    assert_eq!(text(&out, "x").x, 4.);
    assert_eq!(text(&out, "y").x, 3.);
    let color = out
        .items
        .iter()
        .find_map(|i| {
            if let DisplayItem::Text { text, style, .. } = i {
                (text == "x").then_some(style.color)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(color, Color::rgb(0, 0, 255));
}
#[test]
fn preserved_wrapping_nonbreaking_space_and_unicode_remain_intact() {
    let out = render("<div style='width:20px;white-space:pre-wrap'>ab cd\n😀é</div>");
    let a = text(&out, "ab");
    let c = text(&out, "cd");
    assert!(c.y > a.y);
    assert!(
        out.items
            .iter()
            .any(|i| matches!(i,DisplayItem::Text{text,..} if text=="é"))
    );
    let out = render("a&nbsp;b");
    let contents = out
        .items
        .iter()
        .filter_map(|i| {
            if let DisplayItem::Text { text, .. } = i {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<String>();
    assert_eq!(contents, "a\u{a0}b");
}
#[test]
fn inline_mode_keeps_row_background_transparent() {
    let doc = Document::parse_inline("<b>bold</b> <code>code</code>").unwrap();
    let out = layout(&doc, &LayoutOptions::default(), &Metrics);
    assert!(
        out.items
            .iter()
            .all(|i| matches!(i, DisplayItem::Text { .. }))
    );
}
#[test]
fn malformed_css_and_metrics_remain_finite() {
    for css in [
        "color:var(--a)",
        "--a:var(--a);color:var(--a)",
        "padding:NaNpx",
        "border:bad(-2)",
        "font-size:1e999px",
        "width:-2px",
        "position:relative;left:-3px",
    ] {
        let out = render(&format!("<span style='{css}'>x</span>"));
        assert!(out.scroll_height.is_finite());
        assert!(out.items.iter().all(|i| i.rect().width.is_finite()));
    }
    let selector = format!("{} {{color:red}}", "#x".repeat(5000));
    let out = render(&format!("<style>{selector}</style><span>x</span>"));
    assert!(out.scroll_height.is_finite());
}
