use vel_snippet::{Environment, ExpansionOptions, Snippet, SnippetEdit, SnippetFile};
use vel_text::{Edit, Text};
fn apply(text: &mut Text, edits: &[SnippetEdit]) {
    text.apply_edits(
        &edits
            .iter()
            .map(|e| Edit {
                range: e.range.clone(),
                text: &e.text,
            })
            .collect::<Vec<_>>(),
    );
}
#[test]
fn expansion_table_variables_escapes_forward_mirrors_and_transforms() {
    let env = Environment::default().with_path(std::path::Path::new("/tmp/main.rs"));
    let cases = [
        ("$1abc $0", "abc "),
        ("${1:世界}-$1-$0", "世界-世界-"),
        ("$1 ${1:forward} $1", "forward forward forward"),
        (
            r"\$1 ${TM_FILENAME} ${UNSET:fallback} ${TM_LINE_INDEX}/${TM_LINE_NUMBER}",
            "$1 main.rs fallback 0/1",
        ),
        (
            r"${1:foo bar} ${1/\s.*//} ${1/(.*)/\U$1\E/}",
            "foo bar foo FOO BAR",
        ),
        (r"${1:foo} ${1/(foo)|(bar)/(?1:yes:no)/}", "foo yes"),
        (r"${1:bar} ${1/(foo)|(bar)/(?1yes:no)/}", "bar no"),
        (r"${1:ab} ${1/(.)/\u$1/g}", "ab AB"),
        (r"${1:a/b} ${1/\//-/g}", "a/b a-b"),
        (r"${1:ab} ${1/.*?/X/g}", "ab XXXXX"),
        ("${1:${2:nested}} $2 $1", "nested nested nested"),
        ("${1:$1}", ""),
        (r"${1:a} ${1/(a)?(b)?/(?1:(?2:both:a):none)/}", "a a"),
    ];
    for (source, expected) in cases {
        let snippet = Snippet::parse(source).unwrap();
        let mut text = Text::new();
        let expanded = snippet
            .expand(
                &text,
                std::slice::from_ref(&(0..0)),
                &env,
                ExpansionOptions::default(),
            )
            .unwrap();
        apply(&mut text, &expanded.edits);
        assert_eq!(text.to_string(), expected, "{source}");
        assert!(
            expanded
                .session
                .fields()
                .iter()
                .all(|field| field.range.end <= text.len_bytes()
                    && text.is_char_boundary(field.range.start)
                    && text.is_char_boundary(field.range.end))
        );
        assert!(expanded.session.fields().iter().any(|f| f.number == 0));
    }
    for invalid in ["${1", "${1:no", "${1/x/y/q}", "${1/[/x/}", "${}"] {
        assert!(Snippet::parse(invalid).is_err(), "{invalid}");
    }
}
#[test]
fn nested_field_navigation_parent_mirrors_and_deletion() {
    let snippet =
        Snippet::parse("${1:before ${2:inner} after} | $1 | ${2/(.*)/\\U$1/} $0").unwrap();
    let mut text = Text::new();
    let mut expanded = snippet
        .expand(
            &text,
            std::slice::from_ref(&(0..0)),
            &Environment::default(),
            ExpansionOptions::default(),
        )
        .unwrap();
    apply(&mut text, &expanded.edits);
    assert_eq!(expanded.session.active_field(), Some(1));
    expanded.session.next_field();
    assert_eq!(expanded.session.active_field(), Some(2));
    apply(&mut text, &expanded.session.replace_field("café").unwrap());
    assert_eq!(
        text.to_string(),
        "before café after | before café after | CAFÉ "
    );
    expanded.session.prev_field();
    apply(
        &mut text,
        &expanded.session.replace_field("changed").unwrap(),
    );
    assert_eq!(text.to_string(), "changed | changed | CAFÉ ");
    // $2 remains a transformed occurrence but no editable primary survives its parent's replacement.
    assert!(
        !expanded
            .session
            .fields()
            .iter()
            .any(|f| f.number == 2 && !f.mirror)
    );
    expanded.session.next_field();
    assert_eq!(expanded.session.active_field(), Some(0));
    assert!(!expanded.session.has_next_field());
}
#[test]
fn multicaret_selection_indentation_and_region_mapping() {
    let snippet = Snippet::parse("${1:$SELECTION}\n\t$1\n$TM_CURRENT_WORD $0").unwrap();
    let mut text = Text::from("  one\n\ttwo\n");
    let mut expanded = snippet
        .expand(
            &text,
            &[2..5, 7..10],
            &Environment::default(),
            ExpansionOptions::default(),
        )
        .unwrap();
    apply(&mut text, &expanded.edits);
    assert_eq!(
        text.to_string(),
        "  one\n    one\n  one \n\ttwo\n        two\n    two \n"
    );
    assert_eq!(expanded.session.selections().len(), 2);
    for range in expanded.session.selections() {
        assert!(matches!(
            text.slice(range).to_string().as_str(),
            "one" | "two"
        ));
    }
    apply(&mut text, &expanded.session.replace_field("世界").unwrap());
    for range in expanded.session.selections() {
        assert_eq!(text.slice(range).to_string(), "世界");
    }
    assert!(
        snippet
            .expand(
                &text,
                std::slice::from_ref(&std::ops::Range { start: 5, end: 2 }),
                &Environment::default(),
                ExpansionOptions::default()
            )
            .is_err()
    );
}
#[test]
fn xml_cdata_entities_and_metadata_environment() {
    let resource = SnippetFile::parse("<snippet><content><![CDATA[<b>${1:$TM_COMMENT_START}</b>\n]]></content><tabTrigger>bold</tabTrigger><scope>text.html</scope><description>A &amp; B</description></snippet>").unwrap();
    assert_eq!(resource.tab_trigger, "bold");
    assert_eq!(resource.description, "A & B");
    let mut metadata = vel_syntax::Metadata::default();
    metadata
        .shell_variables
        .insert("TM_COMMENT_START".into(), "//".into());
    let mut text = Text::new();
    let expanded = resource
        .snippet
        .expand(
            &text,
            std::slice::from_ref(&(0..0)),
            &Environment::default().with_metadata(&metadata),
            ExpansionOptions::default(),
        )
        .unwrap();
    apply(&mut text, &expanded.edits);
    assert_eq!(text.to_string(), "<b>//</b>\n");
    assert_eq!(
        SnippetFile::parse("<snippet><content>&lt;tag&gt; &#65;</content></snippet>")
            .unwrap()
            .content,
        "<tag> A"
    );
    assert!(SnippetFile::parse("<wrong><content>x</content></wrong>").is_err());
}

#[test]
fn field_typing_preserves_independent_multicaret_values() {
    let snippet = Snippet::parse("${1:$SELECTION}:$1").unwrap();
    let mut text = Text::from("one two");
    let mut expanded = snippet
        .expand(
            &text,
            &[0..3, 4..7],
            &Environment::default(),
            ExpansionOptions::default(),
        )
        .unwrap();
    apply(&mut text, &expanded.edits);
    let selections = expanded.session.selections();
    let input: Vec<_> = selections
        .iter()
        .map(|range| SnippetEdit {
            range: range.end..range.end,
            text: "!".into(),
        })
        .collect();
    let edits = expanded.session.edit_fields(&text, &input).unwrap();
    apply(&mut text, &edits);
    assert_eq!(text.to_string(), "one!:one! two!:two!");
}
#[test]
fn translated_tabs_follow_visual_unicode_columns_and_empty_xml_content() {
    for (before, expected) in [("é", "é   "), ("世界", "世界    ")] {
        let mut text = Text::from(before);
        let caret = text.len_bytes()..text.len_bytes();
        let snippet = Snippet::parse("\t$0").unwrap();
        let expanded = snippet
            .expand(
                &text,
                std::slice::from_ref(&caret),
                &Environment::default(),
                ExpansionOptions::default(),
            )
            .unwrap();
        apply(&mut text, &expanded.edits);
        assert_eq!(text.to_string(), expected);
    }
    assert_eq!(
        SnippetFile::parse("<snippet><content/></snippet>")
            .unwrap()
            .content,
        ""
    );
}

#[test]
fn multiline_field_edits_do_not_double_indent_and_external_edits_rebase() {
    let snippet = Snippet::parse("${1:one\ntwo}:$1$0").unwrap();
    let mut text = Text::from("  ");
    let mut expanded = snippet
        .expand(
            &text,
            std::slice::from_ref(&(2..2)),
            &Environment::default(),
            ExpansionOptions::default(),
        )
        .unwrap();
    apply(&mut text, &expanded.edits);
    let primary = expanded.session.selections()[0].clone();
    let edits = expanded
        .session
        .edit_fields(
            &text,
            &[SnippetEdit {
                range: primary.start + 1..primary.start + 1,
                text: "!".into(),
            }],
        )
        .unwrap();
    apply(&mut text, &edits);
    assert_eq!(text.to_string(), "  o!ne\n  two:o!ne\n  two");
    let end = text.len_bytes();
    let changes = vel_buffer::ChangeSet::new(vec![vel_buffer::Change {
        old_range: end..end,
        new_len: 1,
    }]);
    assert!(expanded.session.rebase(&changes));
    assert_eq!(
        expanded
            .session
            .fields()
            .iter()
            .find(|f| f.number == 0)
            .unwrap()
            .range,
        end..end
    );
}

#[test]
fn documented_snippet_variable_table_at_a_unicode_column() {
    // https://www.sublimetext.com/docs/completions.html#variables
    let mut env = Environment::default().with_path(std::path::Path::new("/tmp/table.rs"));
    env.0.insert("TM_FULLNAME".into(), "Ada Example".into());
    env.0.insert("TM_SCOPE".into(), "source.rust".into());
    let text = Text::from("first\n é word end");
    let start = "first\n é ".len();
    let end = start + 4;
    for (variable, expected) in [
        ("TM_CURRENT_LINE", " é word end"),
        ("TM_CURRENT_WORD", "word"),
        ("TM_DIRECTORY", "/tmp"),
        ("TM_FILENAME", "table.rs"),
        ("TM_FILEPATH", "/tmp/table.rs"),
        ("TM_FULLNAME", "Ada Example"),
        ("TM_SCOPE", "source.rust"),
        ("TM_LINE_INDEX", "1"),
        ("TM_LINE_NUMBER", "2"),
        ("TM_SELECTED_TEXT", "word"),
        ("SELECTION", "word"),
        ("TM_TAB_SIZE", "4"),
        ("TM_SOFT_TABS", "YES"),
    ] {
        let s = Snippet::parse(&format!("${variable}")).unwrap();
        let expansion = s
            .expand(
                &text,
                std::slice::from_ref(&(start..end)),
                &env,
                ExpansionOptions::default(),
            )
            .unwrap();
        assert_eq!(expansion.edits[0].text, expected, "{variable}");
    }
    let s = Snippet::parse("$TM_SOFT_TABS/$TM_TAB_SIZE").unwrap();
    let expansion = s
        .expand(
            &text,
            std::slice::from_ref(&(start..start)),
            &env,
            ExpansionOptions {
                tab_size: 8,
                translate_tabs_to_spaces: false,
            },
        )
        .unwrap();
    assert_eq!(expansion.edits[0].text, "NO/8");
}
