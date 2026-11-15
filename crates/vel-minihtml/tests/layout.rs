use std::fmt::Write;
use vel_minihtml::*;
#[derive(Debug)]
struct Metrics;
impl FontMetrics for Metrics {
    fn measure(&self, text: &str, style: &TextStyle) -> TextMetrics {
        TextMetrics {
            advance: text
                .chars()
                .map(|c| if c as u32 > 0x2fff { 1.0 } else { 0.6 })
                .sum::<f32>()
                * style.font_size,
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
fn options(width: f32) -> LayoutOptions {
    LayoutOptions {
        max_width: width,
        root_font_size: 10.,
        ..LayoutOptions::default()
    }
}
fn render(html: &str, width: f32) -> DisplayList {
    layout(&Document::parse(html).unwrap(), &options(width), &Metrics)
}
fn runs(list: &DisplayList) -> Vec<(&str, Rect, &TextStyle)> {
    list.items
        .iter()
        .filter_map(|i| {
            if let DisplayItem::Text {
                text, rect, style, ..
            } = i
            {
                Some((text.as_str(), *rect, style))
            } else {
                None
            }
        })
        .collect()
}
fn close(a: f32, b: f32) {
    assert!((a - b).abs() < 0.01, "{a} != {b}");
}
#[test]
fn selectors_cascade_custom_properties_and_inline_formatting() {
    let html = r##"<html class="user"><head><style>
      html.dark { --ink: #f00; --pad: 2px 4px; }
      div span {color:green} .a > span.note {color:blue}
      #popup .a > span.note {color:var(--ink);padding:var(--pad)}
      span, b {font-style:italic} span {color:purple !important}
    </style></head><body id="popup"><div class="a"><span class="note x" style="color:yellow">hello</span> <b>bold</b><i>italics</i><u>under</u><code>code</code><var>var</var><tt>tt</tt></div></body></html>"##;
    let out = render(html, 400.);
    let runs = runs(&out);
    let hello = runs.iter().find(|(t, _, _)| *t == "hello").unwrap();
    assert_eq!(hello.2.color, Color::rgb(128, 0, 128));
    assert!(hello.2.italic);
    close(hello.1.x, 4.);
    assert_eq!(
        runs.iter()
            .find(|(t, _, _)| *t == "bold")
            .unwrap()
            .2
            .font_weight,
        700
    );
    assert!(
        runs.iter()
            .find(|(t, _, _)| *t == "italics")
            .unwrap()
            .2
            .italic
    );
    assert!(
        runs.iter()
            .find(|(t, _, _)| *t == "under")
            .unwrap()
            .2
            .underline
    );
    for tag in ["code", "tt"] {
        assert_eq!(
            runs.iter()
                .find(|(t, _, _)| *t == tag)
                .unwrap()
                .2
                .font_family
                .as_ref(),
            "monospace"
        );
    }
}
#[test]
fn dark_light_predefined_variables_fallback_and_cycles() {
    let html = r##"<style>html.dark span {color:var(--redish)} html.light span {color:#123} div {--a:var(--b);--b:var(--a)} b {color:var(--missing,#0f0)}</style><span>x</span><div><b>y</b><i style="color:var(--a)">z</i></div>"##;
    let doc = Document::parse(html).unwrap();
    let opts = options(100.);
    let out = layout(&doc, &opts, &Metrics);
    assert_eq!(
        runs(&out)[0].2.color,
        opts.palette.color("var(--redish)").unwrap()
    );
    assert_eq!(runs(&out)[1].2.color, Color::rgb(0, 255, 0));
    let mut light = opts;
    light.palette.set("--background", Color::rgb(255, 255, 255));
    let out = layout(&doc, &light, &Metrics);
    assert_eq!(runs(&out)[0].2.color, Color::rgb(17, 34, 51));
    for name in [
        "background",
        "foreground",
        "accent",
        "redish",
        "orangish",
        "yellowish",
        "greenish",
        "cyanish",
        "bluish",
        "purplish",
        "pinkish",
    ] {
        assert!(light.palette.color(&format!("var(--{name})")).is_ok());
    }
}
#[test]
fn wrapping_spaces_inline_boundaries_and_baselines() {
    let out = render("hello<span> </span><b>world</b>", 60.);
    let r = runs(&out);
    assert_eq!(r.len(), 2);
    close(r[0].1.x, 0.);
    close(r[1].1.x, 0.);
    close(r[1].1.y, 12.);
    close(out.scroll_height, 24.);
    let out = render(
        "<span style='font-size:20px'>A</span><small>b</small>",
        100.,
    );
    let baselines: Vec<_> = out
        .items
        .iter()
        .filter_map(|i| {
            if let DisplayItem::Text { rect, baseline, .. } = i {
                Some(rect.y + baseline)
            } else {
                None
            }
        })
        .collect();
    close(baselines[0], baselines[1]);
    let out = render("<div style='text-align:right'>abc</div>", 100.);
    close(runs(&out)[0].1.x, 82.);
    let out = render("<div style='text-align:center'>abc</div>", 100.);
    close(runs(&out)[0].1.x, 41.);
}
#[test]
fn box_model_css_order_units_constraints_and_relative_offsets() {
    let out = render(
        "<div style='width:40px;padding:1px 2px 3px 4px;margin:5px 6px 7px 8px;border:2px solid red;border-radius:4px'>x</div>",
        200.,
    );
    let r = runs(&out);
    close(r[0].1.x, 14.);
    close(r[0].1.y, 8.);
    let rect = out
        .items
        .iter()
        .find_map(|i| {
            if let DisplayItem::Box {
                rect, background, ..
            } = i
            {
                (background.a == 0).then_some(*rect)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(
        rect,
        Rect {
            x: 8.,
            y: 5.,
            width: 50.,
            height: 20.
        }
    );
    close(out.scroll_height, 32.);
    let out = render(
        "<div style='font-size:2em;padding-left:1rem;width:50%;min-width:60px;max-width:70px;position:relative;left:-3px;top:2pt'>x</div>",
        100.,
    );
    let r = runs(&out);
    close(r[0].2.font_size, 20.);
    close(r[0].1.x, 7.);
    close(r[0].1.y, 8. / 3.);
    let out = render(
        "<span style='position:relative;left:5px;top:3px'>x</span>y",
        100.,
    );
    let r = runs(&out);
    close(r[0].1.x, 5.);
    close(r[0].1.y, 3.);
    close(r[1].1.x, 6.);
    close(r[1].1.y, 0.);
    assert_eq!(Length::Px(2.).resolve(10., 12., 100.), Some(2.));
    assert_eq!(Length::Em(2.).resolve(10., 12., 100.), Some(20.));
    assert_eq!(Length::Rem(2.).resolve(10., 12., 100.), Some(24.));
    assert_eq!(Length::Percent(20.).resolve(10., 12., 100.), Some(20.));
}
#[test]
fn inline_block_dimensions_and_vertical_alignment() {
    let out = render(
        "a<span style='display:inline-block;width:20px;height:20px;background-color:red;vertical-align:top'>b</span> c",
        100.,
    );
    let r = runs(&out);
    close(r[0].1.x, 0.);
    close(r[1].1.x, 6.);
    close(r[1].1.y, 0.);
    close(out.scroll_height, 20.);
    let out = render(
        "hello <span style='display:inline-block;width:40px;height:20px'>b</span>",
        60.,
    );
    let r = runs(&out);
    let b = r.iter().find(|(t, _, _)| *t == "b").unwrap();
    close(b.1.x, 0.);
    close(b.1.y, 12.);
    assert!(!r.iter().any(|(t, _, _)| *t == " "));
}
#[test]
fn lists_whitespace_headings_and_hidden_content() {
    let out = render(
        "<ol><li>one<li>two<ul style='list-style-type:square'><li>nested</li></ul></ol>",
        200.,
    );
    let r = runs(&out);
    assert!(r.iter().any(|(t, _, _)| *t == "1."));
    assert!(r.iter().any(|(t, _, _)| *t == "2."));
    assert!(r.iter().any(|(t, _, _)| *t == "▪"));
    let out = render("<div style='white-space:pre'>a  b\nc</div>", 20.);
    let r = runs(&out);
    assert!(r.iter().any(|(t, _, _)| *t == "  "));
    let c = r.iter().find(|(t, _, _)| *t == "c").unwrap();
    close(c.1.x, 0.);
    close(c.1.y, 12.);
    assert!(out.size.width > 20.);
    let out = render("<div style='white-space:nowrap'>hello world</div>", 30.);
    assert!(out.size.width > 30.);
    close(out.scroll_height, 12.);
    let out = render(
        "<h1>A</h1><h2>B</h2><h3>C</h3><h4>D</h4><h5>E</h5><h6>F</h6><div style='display:none'>invisible</div><script>alert(1)</script><head>hidden</head><x>visible</x>",
        200.,
    );
    let r = runs(&out);
    assert_eq!(r.len(), 7);
    assert_eq!(r[0].2.font_size, 20.);
    assert_eq!(r[6].0, "visible");
}
#[test]
fn links_images_tooltips_and_command_data() {
    let out = render(
        r#"<div title="outer"><a title="inner" href='subl:open_file {"file":"a.rs"}'><img src="res://Packages/x.png" style="width:40px;border:1px solid red;border-radius:3px"></a><a href="https://example.com">web</a></div>"#,
        200.,
    );
    let image = out
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
    close(image.width, 40.);
    close(image.height, 20.);
    assert_eq!(out.tooltip_at(image.x + 1., image.y + 1.), Some("inner"));
    assert!(
        matches!(out.link_at(image.x+1.,image.y+1.),Some(LinkTarget::Command{name,args}) if name=="open_file" && args["file"]=="a.rs")
    );
    assert!(
        out.links
            .iter()
            .any(|r| matches!(&r.target, LinkTarget::Browser(_)))
    );
    let out = render(
        "<img style='display:block;width:10px' src='data:image/png;base64,AA=='><img src='https://bad' alt='fallback'>",
        100.,
    );
    assert_eq!(
        out.items
            .iter()
            .filter(|i| matches!(i, DisplayItem::Image { .. }))
            .count(),
        1
    );
    assert_eq!(runs(&out)[0].0, "fallback");
    assert!(LinkTarget::parse("subl:evil []").is_err());
    assert!(LinkTarget::parse("subl:bad-name").is_err());
    assert!(matches!(
        LinkTarget::parse("custom:value").unwrap(),
        LinkTarget::Callback(_)
    ));
    assert!(ImageSource::parse("file:///a.gif").is_some());
    assert!(ImageSource::parse("data:image/jpeg;base64,AA==").is_some());
    assert!(ImageSource::parse("javascript:alert(1)").is_none());
}
#[test]
fn inline_subset_entities_and_bounded_parser() {
    let doc=Document::parse_inline("<head>discard</head><style>b{color:red}</style><p>A &amp; B</p><b style='font-size:100px'>bold</b><code>x</code>").unwrap();
    let out = layout(&doc, &options(300.), &Metrics);
    let r = runs(&out);
    assert!(r.iter().any(|(t, _, _)| *t == "&"));
    assert!(!r.iter().any(|(t, _, _)| *t == "discard"));
    assert_eq!(
        r.iter().find(|(t, _, _)| *t == "bold").unwrap().2.font_size,
        10.
    );
    assert!(Document::parse(&"<div>".repeat(300)).is_err());
    assert!(Document::parse(&"x".repeat(4 * 1024 * 1024 + 1)).is_err());
    for source in [
        "<",
        "<span title='unterminated",
        "😀< / >x",
        "<style>/*",
        "<span style='--a:var(--a);color:var(--a)'>ok",
    ] {
        let out = render(source, 100.);
        assert!(out.scroll_height.is_finite());
    }
}
fn snapshot(out: &DisplayList) -> String {
    let mut s = format!(
        "size {:.2} {:.2}; scroll {:.2}\n",
        out.size.width, out.size.height, out.scroll_height
    );
    for item in &out.items {
        match item{
            DisplayItem::Box{rect,background,borders,radii}=>writeln!(s,"box {} #{:02x}{:02x}{:02x}{:02x} border {:?} radii {:?}",geometry(*rect),background.r,background.g,background.b,background.a,borders.map(|b|b.width),radii).unwrap(),
            DisplayItem::Text{rect,baseline,text,style}=>writeln!(s,"text {} baseline {:.2} {:?} {}px {} italic={} underline={} #{:02x}{:02x}{:02x}{:02x} family={:?}",geometry(*rect),baseline,text,style.font_size,style.font_weight,style.italic,style.underline,style.color.r,style.color.g,style.color.b,style.color.a, style.font_family).unwrap(),
            DisplayItem::Image{rect,source}=>writeln!(s,"image {} {:?}",geometry(*rect),source).unwrap(),
        }
    }
    for link in &out.links {
        writeln!(s, "link {} {:?}", geometry(link.rect), link.target).unwrap();
    }
    for tip in &out.tooltips {
        writeln!(s, "tip {} {:?}", geometry(tip.rect), tip.title).unwrap();
    }
    s
}
fn geometry(r: Rect) -> String {
    format!("[{:.2},{:.2},{:.2},{:.2}]", r.x, r.y, r.width, r.height)
}
#[test]
fn documentation_style_golden_display_lists() {
    for (name, html, width) in [
        (
            "hover",
            r#"<style>.sig{background-color:color(var(--background) blend(white 8%));padding:4px;border-radius:3px} code{color:var(--greenish)}</style><div class="sig"><b>fn</b> <code>read(path)</code></div><p>Reads a <a href="https://example.com" title="API">file</a>.</p>"#,
            150.,
        ),
        (
            "lists",
            "<ul><li>alpha</li><li><b>beta</b></li></ul><ol><li>first</li><li>second</li></ol>",
            100.,
        ),
        (
            "inline_block",
            "<span style='display:inline-block;background-color:red;padding:2px;border:1px solid blue;border-radius:2px'>ab</span> cd<br><small>ef</small>",
            60.,
        ),
        (
            "image",
            "<a href='subl:open_file {\"file\":\"a.rs\"}' title='open'><img src='res://Packages/icon.png' style='width:20px;vertical-align:middle'></a> icon",
            100.,
        ),
    ] {
        let actual = snapshot(&render(html, width));
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/goldens/{name}.txt"));
        if std::env::var_os("VEL_UPDATE_GOLDENS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let backup = std::env::temp_dir().join("velocity-round8-backups");
            std::fs::create_dir_all(&backup).unwrap();
            if path.exists() {
                std::fs::copy(&path, backup.join(format!("minihtml-{name}.txt"))).unwrap();
            }
            let temporary = path.with_extension("txt.tmp");
            std::fs::write(&temporary, &actual).unwrap();
            std::fs::rename(temporary, &path).unwrap();
            continue;
        }
        assert_eq!(actual, std::fs::read_to_string(path).unwrap(), "{name}");
    }
}
