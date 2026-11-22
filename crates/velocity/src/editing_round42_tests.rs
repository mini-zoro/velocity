use super::*;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use vel_highlight::ScopeProvider;
use vel_text::Text;

fn highlighted(source: &str, syntax: &str, selection: Selection) -> Editor {
    let mut editor = Editor::new(Text::from(source));
    editor.set_view_setting("tab_size", json!(4));
    editor.set_view_setting("translate_tabs_to_spaces", json!(true));
    editor.sync();
    editor
        .attach_language(
            crate::language::Assets::bundled().unwrap(),
            syntax,
            Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
        )
        .unwrap();
    editor.set_selection(selection);
    settle(&mut editor);
    editor
}

fn settle(editor: &mut Editor) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        editor.poll_language();
        let ready = {
            let language = editor.language.borrow();
            let language = language.as_ref().unwrap();
            assert!(language.errors.is_empty(), "{:?}", language.errors);
            language.ready()
        };
        if ready {
            return;
        }
        assert!(Instant::now() < deadline, "language worker did not finish");
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

fn fixture(marked: &str, repetitions: usize) -> (String, Selection) {
    let mut source = String::new();
    let mut carets = Vec::new();
    for _ in 0..repetitions {
        for ch in marked.chars() {
            if ch == '|' {
                carets.push(Region::caret(source.len()));
            } else {
                source.push(ch);
            }
        }
    }
    // Exercise a non-first primary as well as all sorted endpoints.
    carets.rotate_right(1);
    (source, Selection::from_regions(carets))
}

#[test]
fn thousand_caret_enter_matches_per_caret_rules_and_round_trips_history() {
    for (syntax, marked, expected_start) in [
        (
            "C",
            "if (x) {|}\nif (x)\n    f();|\n    #define ONE 1|\n    |\n    caf\u{e9}();|\n",
            "if (x) {\n    \n}\nif (x)\n    f();\n\n    #define ONE 1\n\n\n    \n",
        ),
        (
            "Python",
            "if flag:|\n    compute()|\n    |\n    f(|)\n    # note|\n",
            "if flag:\n    \n    compute()\n    \n\n    \n    f(",
        ),
        (
            "Markdown",
            "    prose|\n\n- item|\n\n```c\nif (x) {|}\nif (x)\n    f();|\n    |\n```\n\n",
            "    prose\n    \n\n- item\n\n\n```c\n",
        ),
    ] {
        let (source, before) = fixture(marked, 200);
        assert_eq!(before.len(), 1000);
        let mut editor = highlighted(&source, syntax, before.clone());
        let mut reference = highlighted(&source, syntax, before.clone());
        let version = editor.buffer.change_id();

        reference.insert_indented_newline::<false>(&[]);
        reference.sync();
        editor.run("insert", &json!({"characters":"\n"})).unwrap();

        let result = editor.text.to_string();
        let after = editor.buffer.view(editor.id).sel().clone();
        assert!(
            result.starts_with(expected_start),
            "{syntax}: {:?}",
            &result[..expected_start.len().min(result.len())]
        );
        assert_eq!(result, reference.text.to_string(), "{syntax}");
        assert_eq!(
            &after,
            reference.buffer.view(reference.id).sel(),
            "{syntax}"
        );
        assert_eq!(editor.buffer.change_id().0, version.0 + 1, "{syntax}");

        editor.run("undo", &json!({})).unwrap();
        assert_eq!(editor.text.to_string(), source, "{syntax}");
        assert_eq!(editor.buffer.view(editor.id).sel(), &before, "{syntax}");
        editor.run("redo", &json!({})).unwrap();
        assert_eq!(editor.text.to_string(), result, "{syntax}");
        assert_eq!(editor.buffer.view(editor.id).sel(), &after, "{syntax}");

        settle(&mut editor);
        settle(&mut reference);
        let actual = editor.language.borrow();
        let expected = reference.language.borrow();
        let scopes =
            |language: &crate::language::Language| Arc::clone(language.scopes.as_ref().unwrap());
        let actual = scopes(actual.as_ref().unwrap());
        let expected = scopes(expected.as_ref().unwrap());
        for caret in after.iter() {
            assert_eq!(
                actual.scopes_at(caret.b),
                expected.scopes_at(caret.b),
                "{syntax}"
            );
        }
    }
}

#[test]
fn thousand_caret_enter_regex_work_tracks_distinct_inputs() {
    let (source, selection) = fixture("if (flag)\n    f();|\n", 1000);
    let mut editor = highlighted(&source, "C", selection.clone());
    let mut per_caret = HashMap::new();
    NEWLINE_EVALUATIONS.set(0);
    for caret in selection.iter() {
        let _ = editor.newline_with_blank::<false>(caret, None, &mut per_caret);
    }
    let scalar = NEWLINE_EVALUATIONS.replace(0);
    editor.run("insert", &json!({"characters":"\n"})).unwrap();
    let batched = NEWLINE_EVALUATIONS.get();
    assert!(
        scalar >= 1000 && batched > 0 && batched <= 16,
        "{scalar} -> {batched} regex evaluations"
    );
}
