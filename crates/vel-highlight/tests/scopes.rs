use std::{sync::Arc, time::Duration};
use vel_buffer::{Change, ChangeId, ChangeSet};
use vel_color_scheme::ColorScheme;
use vel_highlight::{DocumentHighlighter, ScopeProvider, ScopeRun};
use vel_scope::Selector;
use vel_syntax::{MetadataSet, SyntaxDefinition, SyntaxSet, SyntaxSetBuilder};
use vel_text::Text;
fn set() -> Arc<SyntaxSet> {
    let mut builder = SyntaxSetBuilder::new();
    builder.add_with_path(SyntaxDefinition::load_from_str("name: Fixture\nfile_extensions: [fix]\nscope: source.fixture\ncontexts:\n  main:\n    - match: '(?<=fn )[\\p{L}_]+'\n      scope: entity.name.function.fixture\n    - match: '/\\*'\n      push:\n        - meta_scope: comment.block.fixture\n        - match: '\\*/'\n          pop: true\n",true,None).unwrap(),"Fixture.sublime-syntax");
    Arc::new(builder.build())
}
fn engine(set: &Arc<SyntaxSet>, text: Text, version: u64) -> DocumentHighlighter {
    DocumentHighlighter::new(
        text,
        ChangeId(version),
        set.clone(),
        &set.syntaxes()[0],
        Arc::new(ColorScheme::velocity_dark()),
    )
}
#[test]
fn compact_scopes_match_reference_through_edits_and_scheme_swaps() {
    assert!(std::mem::size_of::<ScopeRun>() <= 16);
    let set = set();
    let mut text = Text::from("fn café\n/* first\nsecond */\nfn 世界\n");
    let mut actual = engine(&set, text.clone(), 0);
    let unavailable = actual
        .request_lines(ChangeId(0), 0..3, Duration::ZERO)
        .unwrap();
    assert!(!unavailable.scopes.complete());
    assert!(unavailable.scopes.scopes_at(1).is_none());
    for version in 0..35 {
        actual.finish().unwrap();
        let snapshot = actual.scope_snapshot(0..text.len_lines());
        assert!(snapshot.complete());
        let reference =
            vel_syntax::indent::scoped_spans(&set, &set.syntaxes()[0], &text.to_string()).unwrap();
        for span in reference {
            for point in span.range {
                if text.is_char_boundary(point) {
                    assert_eq!(
                        snapshot.scopes_at(point),
                        Some(span.scopes.as_slice()),
                        "v{version} at {point}"
                    );
                }
            }
        }
        let expected = MetadataSet::new()
            .symbols(
                &text.to_string(),
                &vel_syntax::indent::scoped_spans(&set, &set.syntaxes()[0], &text.to_string())
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(actual.symbols().unwrap().symbols, expected);
        if version == 0 {
            let selector = Selector::parse("comment.block").unwrap();
            let point = text.line_to_byte(2) + 2;
            assert!(snapshot.match_selector(point, &selector).is_some());
            assert_eq!(snapshot.extract_scope(point), Some(9..27));
        }
        let at = text.line_to_byte((version as usize) % text.len_lines());
        let insert = if version % 3 == 0 { "\n" } else { "x" };
        let changes = ChangeSet::new(vec![Change {
            old_range: at..at,
            new_len: insert.len(),
        }]);
        text.insert(at, insert);
        actual
            .apply_change(text.clone(), ChangeId(version + 1), &changes)
            .unwrap();
        assert!(snapshot.version() < actual.version());
        if version % 7 == 0 {
            actual
                .set_color_scheme(Arc::new(ColorScheme::velocity_dark()))
                .unwrap();
        }
    }
}
#[test]
fn symbol_transformations_are_selected_and_incremental() {
    let set = set();
    let mut metadata = MetadataSet::new();
    metadata.add_plist(br#"<plist><dict><key>scope</key><string>entity.name.function</string><key>settings</key><dict><key>symbolTransformation</key><string>s/^/local:/</string><key>symbolIndexTransformation</key><string>s/^/index:/</string></dict></dict></plist>"#).unwrap();
    let mut actual =
        engine(&set, Text::from("fn first\nfn second\n"), 0).with_metadata(Arc::new(metadata));
    actual.finish().unwrap();
    let symbols = actual.symbols().unwrap();
    assert!(symbols.complete);
    assert_eq!(symbols.symbols[0].name, "local:first");
    assert_eq!(symbols.symbols[0].index_name, "index:first");
    let mut text = actual.text().clone();
    text.replace(3..8, "new");
    actual
        .apply_change(
            text,
            ChangeId(1),
            &ChangeSet::new(vec![Change {
                old_range: 3..8,
                new_len: 3,
            }]),
        )
        .unwrap();
    actual.finish().unwrap();
    let symbols = actual.symbols().unwrap();
    assert_eq!(symbols.symbols[0].name, "local:new");
    assert_eq!(symbols.symbols[1].range, 10..16);
}
