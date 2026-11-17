use std::{sync::Arc, time::Duration};
use vel_buffer::ChangeId;
use vel_color_scheme::ColorScheme;
use vel_highlight::DocumentHighlighter;
use vel_syntax::SyntaxSet;
use vel_text::Text;

#[test]
fn missing_contexts_and_deep_input_do_not_stop_document_highlighting() {
    let yaml = r#"
name: Resilient
scope: source.resilient
contexts:
  main:
    - include: missing_include
    - match: '\['
      embed: scope:source.missing
      escape: '\]'
    - match: '\('
      push: nested
    - match: Z
      scope: keyword.done
  nested:
    - match: '\('
      push: nested
    - match: '\)'
      pop: true
"#;
    let set = Arc::new(
        SyntaxSet::load_roots(
            &[],
            &[
                ("User/Resilient.sublime-syntax", yaml),
                (
                    "Broken/Neighbor.sublime-syntax",
                    "scope: source.bad\ncontexts: {main: [{match: x, push: [[]]}]}",
                ),
            ],
        )
        .unwrap(),
    );
    assert!(set.warnings().iter().any(|w| w.contains("missing_include")));
    assert!(set.warnings().iter().any(|w| w.contains("Neighbor")));
    let source = format!(
        "[unavailable]\n{}x{}\nZ\n",
        "(".repeat(1200),
        ")".repeat(1200)
    );
    let scheme = Arc::new(
        ColorScheme::parse(r#"{"rules":[{"scope":"keyword.done","foreground":"red"}]}"#).unwrap(),
    );
    let mut engine = DocumentHighlighter::new(
        Text::from(source.as_str()),
        ChangeId(0),
        set.clone(),
        set.find_syntax_by_name("Resilient").unwrap(),
        scheme,
    );
    engine.finish().unwrap();
    let batch = engine
        .request_lines(ChangeId(0), 0..4, Duration::ZERO)
        .unwrap();
    assert!(batch.complete);
    assert_eq!(engine.stats().parsed_lines, 4);
    let done = batch.highlighted.iter().find(|l| l.line == 2).unwrap();
    assert!(
        done.spans
            .iter()
            .any(|s| s.style.foreground == vel_color_scheme::Color::rgb(255, 0, 0))
    );
    let scopes = engine.scope_snapshot(0..4);
    assert!(scopes.complete());
}
