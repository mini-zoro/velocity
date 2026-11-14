use std::sync::Arc;
use vel_syntax::{
    MetadataSet, ScopeStack, ScopedSpan, SymbolTransformation, SyntaxDefinition, SyntaxSetBuilder,
    syntest,
};
fn preferences(scope: &str, settings: &str) -> String {
    format!(
        "<plist version=\"1.0\"><dict><key>scope</key><string><![CDATA[{scope}]]></string><key>settings</key><dict>{settings}</dict></dict></plist>"
    )
}
#[test]
fn preserved_text_does_not_execute_shell_control_indentation() {
    let mut metadata = MetadataSet::new();
    metadata
        .add_plist(
            preferences(
                "source.shell meta.string.heredoc",
                "<key>preserveIndent</key><true/>",
            )
            .as_bytes(),
        )
        .unwrap();
    let text = "        esac\n   case x in\n     {\n       ?>\n";
    let spans = vec![ScopedSpan {
        range: 0..text.len(),
        scopes: "source.shell.fixture meta.string.heredoc"
            .parse::<ScopeStack>()
            .unwrap()
            .scopes,
    }];
    assert_eq!(
        vel_syntax::indent::reindent(
            text,
            &spans,
            &metadata,
            vel_syntax::indent::IndentOptions::default()
        ),
        text
    );
}
#[test]
fn cdata_and_independent_setting_precedence() {
    let mut set = MetadataSet::new();
    set.add_plist(preferences("source.fixture",r#"<key>shellVariables</key><array><dict><key>name</key><string>TM_COMMENT_START</string><key>value</key><string>// </string></dict><dict><key>name</key><string>TM_COMMENT_END_2</string><key>value</key><string>*/</string></dict></array><key>increaseIndentPattern</key><string><![CDATA[\{[ &]*$]]></string><key>cancelCompletion</key><true/><key>preserveIndent</key><false/><key>indentParens</key><true/>"#).as_bytes()).unwrap();
    set.add_plist(preferences("source.fixture meta.special - comment",r#"<key>increaseIndentPattern</key><string></string><key>cancelCompletion</key><integer>0</integer><key>shellVariables</key><array><dict><key>name</key><string>TM_COMMENT_START</string><key>value</key><string># </string></dict></array>"#).as_bytes()).unwrap();
    let plain: ScopeStack = "source.fixture".parse().unwrap();
    let special: ScopeStack = "source.fixture meta.special".parse().unwrap();
    let plain = set.metadata_for(&plain);
    let special = set.metadata_for(&special);
    assert!(set.matches_pattern(&plain, "increaseIndentPattern", "fn main() { &\n"));
    assert!(!set.matches_pattern(&special, "increaseIndentPattern", "{"));
    assert_eq!(special.boolean("cancelCompletion"), Some(false));
    assert_eq!(special.boolean("indentParens"), Some(true));
    assert_eq!(special.shell_variables["TM_COMMENT_START"], "# ");
    assert_eq!(special.shell_variables["TM_COMMENT_END_2"], "*/");
    let comment = set.metadata_for(&"source.fixture meta.special comment.line".parse().unwrap());
    assert_eq!(comment.boolean("cancelCompletion"), Some(true));
}
#[test]
fn transformations_handle_global_empty_matches_unicode_and_capture_case() {
    let transform = SymbolTransformation::parse(
        r#"
        # Strip whitespace outside quoted strings, including reluctant empty matches.
        s/(\"(?:\\\"|[^"])*\")?[ \t]*?/\1/g;
    "#,
    )
    .unwrap();
    assert_eq!(
        transform.apply(" key . \"two words\" "),
        "key.\"two words\""
    );
    let transform = SymbolTransformation::parse(r"s/(é)(foo)/\u$1-\U$2\E/g; s/$/!/;").unwrap();
    assert_eq!(transform.apply("éfoo"), "É-FOO!");
    assert_eq!(
        SymbolTransformation::parse("s/^/  /g;")
            .unwrap()
            .apply("word"),
        "  word"
    );
    assert!(SymbolTransformation::parse("s/unclosed").is_err());
}
#[test]
fn nested_scopes_form_one_symbol_and_transform_names() {
    let mut metadata = MetadataSet::new();
    metadata.add_plist(preferences("meta.function.identifier entity.name.function, meta.function.identifier meta.interpolation",r#"<key>showInSymbolList</key><true/><key>showInIndexedSymbolList</key><true/><key>symbolTransformation</key><string>s/\s+//g;</string>"#).as_bytes()).unwrap();
    let scopes = |s: &str| s.parse::<ScopeStack>().unwrap().scopes;
    let spans = vec![
        ScopedSpan {
            range: 0..1,
            scopes: scopes("source.fixture meta.function.identifier entity.name.function"),
        },
        ScopedSpan {
            range: 1..6,
            scopes: scopes(
                "source.fixture meta.function.identifier meta.interpolation variable.other",
            ),
        },
    ];
    let symbols = metadata.symbols("f$name", &spans).unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "f$name");
    assert_eq!(symbols[0].range, 0..6);
}
fn fixture() -> Arc<vel_syntax::SyntaxSet> {
    let mut b = SyntaxSetBuilder::new();
    b.add_with_path(SyntaxDefinition::load_from_str("name: Fixture\nscope: source.fixture\ncontexts:\n  main:\n    - match: 'name[0-9]+'\n      scope: entity.name.function.fixture\n",true,None).unwrap(),"Packages/Fixture/Fixture.sublime-syntax");
    Arc::new(b.build())
}
#[test]
fn symbol_runner_checks_names_exhaustiveness_and_partial_files() {
    let set = fixture();
    let metadata = MetadataSet::new();
    let header = "// SYNTAX TEST \"Packages/Fixture/Fixture.sublime-syntax\"\n";
    let input = format!("{header}   name123\n// @@@@@@@ definition\n");
    let result = syntest::evaluate_with_metadata(&set, &metadata, &input, "fixture");
    assert_eq!(
        (result.symbol_counts.passed, result.symbol_counts.failed),
        (1, 0)
    );
    let result = syntest::evaluate_with_metadata(
        &set,
        &metadata,
        &format!("{input}   name456\n"),
        "fixture",
    );
    assert_eq!(result.symbol_counts.failed, 1);
    let partial = format!(
        "{}{}",
        input.replace("SYNTAX TEST ", "SYNTAX TEST partial-symbols "),
        "   name456\n// ^^^^^^^ entity.name.function\n"
    );
    let result = syntest::evaluate_with_metadata(&set, &metadata, &partial, "fixture");
    assert_eq!(result.symbol_counts.failed, 0);
    assert_eq!(result.scope_counts.passed, 1);
    let wrong = input.replace("definition", "reference");
    assert_eq!(
        syntest::evaluate_with_metadata(&set, &metadata, &wrong, "fixture")
            .symbol_counts
            .failed,
        1
    );
}
#[test]
fn reindent_variants_execute_and_detect_a_broken_rule() {
    let set = fixture();
    let mut metadata = MetadataSet::new();
    metadata.add_plist(preferences("source.fixture",r#"<key>increaseIndentPattern</key><string>\{$</string><key>decreaseIndentPattern</key><string>^\s*\}</string>"#).as_bytes()).unwrap();
    let input =
        "// SYNTAX TEST reindent \"Packages/Fixture/Fixture.sublime-syntax\"\n{\n    item\n}\n";
    let result = syntest::evaluate_with_metadata(&set, &metadata, input, "fixture");
    assert_eq!(
        (result.reindent_counts.passed, result.reindent_counts.failed),
        (2, 0)
    );
    let wrong = input.replace("    item", "item");
    assert_eq!(
        syntest::evaluate_with_metadata(&set, &metadata, &wrong, "fixture")
            .reindent_counts
            .failed,
        2
    );
    let bad = "// SYNTAX TEST reindent-unchanged \"Packages/Fixture/Fixture.sublime-syntax\"\n{}\n    item\n";
    assert_eq!(
        syntest::evaluate_with_metadata(&set, &metadata, bad, "fixture")
            .reindent_counts
            .failed,
        1
    );
}
#[test]
fn annotation_function_suffix_does_not_weaken_negative_selectors() {
    let mut builder = SyntaxSetBuilder::new();
    builder.add_with_path(SyntaxDefinition::load_from_str("name: Fixture\nscope: source.fixture\ncontexts:\n  main:\n    - match: 'comment'\n      scope: comment.line.fixture\n",true,None).unwrap(),"Packages/Fixture/Fixture.sublime-syntax");
    let set = builder.build();
    let input = "// SYNTAX TEST \"Packages/Fixture/Fixture.sublime-syntax\"\n   comment\n// ^^^^^^^ -comment() {}\n";
    assert_eq!(syntest::evaluate(&set, input, "fixture").counts.failed, 1);
}
