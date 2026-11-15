//! Release real-highlight scope filtering/checking for a 100-line viewport.
use std::{
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};
use vel_buffer::{Change, ChangeId, ChangeSet};
use vel_color_scheme::ColorScheme;
use vel_highlight::DocumentHighlighter;
use vel_scope::Selector;
use vel_spell::{DocumentSpelling, SpellChecker};
use vel_syntax::{SyntaxDefinition, SyntaxSetBuilder};
use vel_text::Text;
fn main() {
    let checker = SpellChecker::english().unwrap();
    let mut builder = SyntaxSetBuilder::new();
    builder.add(
        SyntaxDefinition::load_from_str(
            "name: Fixture\nscope: text.fixture\ncontexts:\n  main: []\n",
            true,
            None,
        )
        .unwrap(),
    );
    let set = Arc::new(builder.build());
    let source = (0..100)
        .map(|i| {
            format!(
                "The quick brown fox jumps over a lazy dog with a mispelled word on line {i}.\n"
            )
        })
        .collect::<String>();
    let mut text = Text::from(source.as_str());
    let selector = Selector::parse("text").unwrap();
    let mut highlighter = DocumentHighlighter::new(
        text.clone(),
        ChangeId(0),
        set.clone(),
        &set.syntaxes()[0],
        Arc::new(ColorScheme::velocity_dark()),
    );
    highlighter.finish().unwrap();
    let scopes = highlighter
        .request_lines(ChangeId(0), 0..100, Duration::ZERO)
        .unwrap()
        .scopes;
    let start = Instant::now();
    let errors = black_box(checker.check_scoped(&scopes, &selector).unwrap());
    let viewport_ns = start.elapsed().as_nanos();
    assert_eq!(errors.len(), 100);
    let mut document = DocumentSpelling::new(text.clone(), ChangeId(0), selector);
    document.check_lines(&checker, &scopes, 0..100).unwrap();
    let mut samples = Vec::new();
    for version in 1..=100 {
        let at = text.line_to_byte(50) + 2;
        let value = if version % 2 == 0 { "e" } else { "a" };
        text.replace(at..at + 1, value);
        let changes = ChangeSet::new(vec![Change {
            old_range: at..at + 1,
            new_len: 1,
        }]);
        highlighter
            .apply_change(text.clone(), ChangeId(version), &changes)
            .unwrap();
        highlighter.finish().unwrap();
        let scopes = highlighter
            .request_lines(ChangeId(version), 0..100, Duration::ZERO)
            .unwrap()
            .scopes;
        let start = Instant::now();
        document
            .apply_change(text.clone(), ChangeId(version), &changes)
            .unwrap();
        black_box(document.check_lines(&checker, &scopes, 0..100).unwrap());
        samples.push(start.elapsed().as_nanos());
        assert_eq!(document.checked_lines(), 1);
    }
    println!("{{\"viewport_ns\":{viewport_ns},\"edit_ns\":{samples:?},\"lines\":100}}");
}
