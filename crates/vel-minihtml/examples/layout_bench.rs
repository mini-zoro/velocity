//! Seven runs per case, with both precompiled and parse+layout timings.
use std::{hint::black_box, time::Instant};
use vel_minihtml::*;
#[derive(Debug)]
struct Metrics;
impl FontMetrics for Metrics {
    fn measure(&self, text: &str, style: &TextStyle) -> TextMetrics {
        TextMetrics {
            advance: text.chars().count() as f32 * style.font_size * 0.6,
            ascent: style.font_size * 0.8,
            descent: style.font_size * 0.2,
        }
    }
}
fn main() {
    let options = LayoutOptions::default();
    let html = include_str!("../tests/fixtures/hover.html");
    assert!((1900..=2200).contains(&html.len()));
    let doc = Document::parse(html).unwrap();
    let annotations:Vec<_>=(0..200).map(|i|format!("<div class='diagnostic'><b>E{i:03}</b>: mismatched types <a href='subl:show_error {{\"index\":{i}}}'>details</a></div>")).collect();
    let docs: Vec<_> = annotations
        .iter()
        .map(|s| Document::parse(s).unwrap())
        .collect();
    black_box(layout(&doc, &options, &Metrics));
    for run in 0..7 {
        let start = Instant::now();
        for _ in 0..100 {
            black_box(layout(&doc, &options, &Metrics));
        }
        let hover = start.elapsed().as_secs_f64() * 1000. / 100.;
        let start = Instant::now();
        for _ in 0..100 {
            black_box(layout(&Document::parse(html).unwrap(), &options, &Metrics));
        }
        let full = start.elapsed().as_secs_f64() * 1000. / 100.;
        let start = Instant::now();
        for _ in 0..10 {
            for doc in &docs {
                black_box(layout(doc, &options, &Metrics));
            }
        }
        let batch = start.elapsed().as_secs_f64() * 1000. / 10.;
        let start = Instant::now();
        for _ in 0..10 {
            for html in &annotations {
                black_box(layout(&Document::parse(html).unwrap(), &options, &Metrics));
            }
        }
        let batch_full = start.elapsed().as_secs_f64() * 1000. / 10.;
        println!(
            "{{\"case\":\"minihtml\",\"run\":{run},\"html_bytes\":{},\"hover_layout_ms\":{hover:.6},\"hover_parse_layout_ms\":{full:.6},\"annotations_200_layout_ms\":{batch:.6},\"annotations_200_parse_layout_ms\":{batch_full:.6}}}",
            html.len()
        );
        assert!(hover <= 0.5, "hover layout over budget: {hover} ms");
        assert!(batch <= 5., "annotations over budget: {batch} ms");
    }
}
