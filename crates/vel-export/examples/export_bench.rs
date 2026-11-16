//! Measures export of an already-highlighted 10k-line document, without parsing.
use std::{sync::Arc, time::Instant};
use vel_color_scheme::{ColorScheme, FontStyle};
use vel_export::{Options, export_range, write_print_file};
use vel_highlight::{StyleSpan, StyledLine};
use vel_text::Text;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scheme = ColorScheme::velocity_dark();
    let source = "\tlet answer = \"猫 & 🦀\"; // <export>\n".repeat(10_000);
    let text = Text::from(source.as_str());
    let mut keyword = scheme.default_style();
    keyword.font_style = FontStyle::parse("bold italic underline")?;
    let spans = Arc::from([
        StyleSpan {
            range: 0..1,
            style: scheme.default_style(),
        },
        StyleSpan {
            range: 1..4,
            style: keyword,
        },
        StyleSpan {
            range: 4..source.len() / 10_000,
            style: scheme.default_style(),
        },
    ]);
    let lines: Vec<_> = (0..10_000)
        .map(|line| StyledLine {
            line,
            spans: Arc::clone(&spans),
            exact: true,
        })
        .collect();
    let options = Options {
        line_numbers: true,
        ..Options::default()
    };
    let render = || export_range(&text, 0..text.len_bytes(), &lines, &scheme, &options);
    std::hint::black_box(render()?);
    let mut samples = Vec::new();
    let mut last = None;
    for _ in 0..20 {
        let start = Instant::now();
        let export = render()?;
        samples.push(start.elapsed().as_secs_f64() * 1000.);
        last = Some(export);
    }
    let export = last.unwrap();
    assert_eq!(export.plain_text, source);
    let path = write_print_file(&export)?;
    assert_eq!(std::fs::read_to_string(&path)?, export.html);
    println!(
        "{}",
        serde_json::json!({"lines":10_000,"bytes":source.len(),"highlight_spans":30_000,"milliseconds":samples,"print_file":path,"html_bytes":export.html.len()})
    );
    Ok(())
}
