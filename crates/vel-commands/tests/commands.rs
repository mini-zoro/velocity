use vel_buffer::{Buffer, Region, Selection, ViewId};
use vel_commands::{Host, Kind, Registry, Snippet};
use vel_config::{Value, json, settings::EditorSettings};
struct Fixture {
    buffer: Buffer,
    id: ViewId,
    settings: EditorSettings,
    clipboard: String,
    actions: Vec<String>,
}
impl Fixture {
    fn new(text: &str, regions: impl IntoIterator<Item = Region>) -> Self {
        let mut buffer = Buffer::new(text);
        let id = buffer.new_view();
        buffer.set_selection(id, Selection::from_regions(regions));
        Self {
            buffer,
            id,
            settings: EditorSettings::default(),
            clipboard: "X\nY".into(),
            actions: Vec::new(),
        }
    }
}
impl Host for Fixture {
    fn buffer(&mut self) -> (&mut Buffer, ViewId) {
        (&mut self.buffer, self.id)
    }
    fn settings(&self) -> EditorSettings {
        self.settings.clone()
    }
    fn clipboard(&mut self) -> String {
        self.clipboard.clone()
    }
    fn set_clipboard(&mut self, text: &str) {
        self.clipboard = text.into();
    }
    fn action(&mut self, name: &str, _: &Value) -> Result<(), String> {
        self.actions.push(name.into());
        Ok(())
    }
}
fn run(f: &mut Fixture, name: &str, args: Value) {
    Registry::standard().run(name, &args, f).unwrap();
}
#[test]
fn editing_commands_apply_to_both_carets_and_undo_atomically() {
    for (name, args, expected) in [
        ("insert", json!({"characters":"🦀"}), "a🦀b\na🦀b"),
        ("paste", json!({}), "aXb\naYb"),
        ("left_delete", json!({}), "b\nb"),
        ("right_delete", json!({}), "a\na"),
        ("delete_word", json!({"forward":false}), "b\nb"),
        (
            "delete_word",
            json!({"forward":true,"sub_words":true}),
            "a\na",
        ),
        ("insert_best_completion", json!({}), "a\tb\na\tb"),
        ("insert_snippet", json!({"contents":"($0)"}), "a()b\na()b"),
        ("upper_case", json!({}), "AB\nAB"),
        ("lower_case", json!({}), "ab\nab"),
        ("title_case", json!({}), "Ab\nAb"),
        ("swap_case", json!({}), "AB\nAB"),
        ("transpose", json!({}), "ba\nba"),
        ("duplicate_line", json!({}), "ab\nab\nab\nab"),
        ("indent", json!({}), "\tab\n\tab"),
        ("unindent", json!({}), "ab\nab"),
    ] {
        let mut f = Fixture::new("ab\nab", [Region::caret(1), Region::caret(4)]);
        let original = f.buffer.text().clone();
        run(&mut f, name, args);
        assert_eq!(f.buffer.text().to_string(), expected, "{name}");
        if f.buffer.text() != &original {
            run(&mut f, "undo", json!({}));
            assert_eq!(f.buffer.text(), &original, "undo {name}");
            run(&mut f, "redo", json!({}));
            assert_eq!(f.buffer.text().to_string(), expected, "redo {name}");
        }
    }
}
#[test]
fn movement_selection_and_soft_history_cover_multicursor_commands() {
    for (name, args) in [
        (
            "move",
            json!({"by":"characters","forward":true,"extend":true}),
        ),
        ("move", json!({"by":"lines","forward":true})),
        ("move", json!({"by":"pages","forward":false})),
        (
            "move",
            json!({"by":"stops","forward":true,"word_begin":true}),
        ),
        ("move_to", json!({"to":"eol","extend":true})),
        ("select_all", json!({})),
        ("expand_selection", json!({"to":"word"})),
        ("expand_selection", json!({"to":"indentation"})),
        ("single_selection", json!({})),
        ("split_selection_into_lines", json!({})),
        ("select_lines", json!({"forward":true})),
        ("find_under_expand", json!({})),
        ("find_under_expand_skip", json!({})),
        ("find_all_under", json!({})),
    ] {
        let mut f = Fixture::new("ab ab\nab ab\nlast", [Region::caret(1), Region::caret(7)]);
        let before = f.buffer.text().clone();
        run(&mut f, name, args);
        assert_eq!(f.buffer.text(), &before, "{name}");
        assert!(!f.buffer.view(f.id).sel().is_empty());
        run(&mut f, "soft_undo", json!({}));
        run(&mut f, "soft_redo", json!({}));
    }
}
#[test]
fn line_commands_transform_separate_selected_blocks() {
    for (name, args, expected) in [
        (
            "sort_lines",
            json!({"case_sensitive":true}),
            "A\nb\n--\nC\nd",
        ),
        (
            "sort_lines",
            json!({"case_sensitive":false}),
            "A\nb\n--\nC\nd",
        ),
        ("reverse_lines", json!({}), "A\nb\n--\nC\nd"),
        (
            "permute_lines",
            json!({"operation":"reverse"}),
            "A\nb\n--\nC\nd",
        ),
        ("join_lines", json!({}), "b A\n--\nd C"),
    ] {
        let mut f = Fixture::new("b\nA\n--\nd\nC", [Region::new(0, 4), Region::new(7, 10)]);
        run(&mut f, name, args);
        assert_eq!(f.buffer.text().to_string(), expected, "{name}");
    }
    for name in ["swap_line_up", "swap_line_down"] {
        let mut f = Fixture::new("a\nb\nc\nd\ne", [Region::caret(2), Region::caret(6)]);
        run(&mut f, name, json!({}));
        assert_eq!(
            f.buffer.text().to_string(),
            if name == "swap_line_up" {
                "b\na\nd\nc\ne"
            } else {
                "a\nc\nb\ne\nd"
            }
        );
    }
}
#[test]
fn clipboard_snippets_newline_tabs_and_typing_groups() {
    let mut f = Fixture::new("  a\n\tb", [Region::caret(3), Region::caret(6)]);
    run(&mut f, "insert", json!({"characters":"\n"}));
    assert_eq!(f.buffer.text().to_string(), "  a\n  \n\tb\n\t");
    let mut f = Fixture::new("a\nb", [Region::caret(0), Region::caret(2)]);
    run(&mut f, "copy", json!({}));
    assert_eq!(f.clipboard, "a\nb");
    run(&mut f, "cut", json!({}));
    assert_eq!(f.buffer.text().to_string(), "");
    assert_eq!(
        Snippet::parse("${2:two} ${1:one} $2$0").unwrap(),
        Snippet {
            text: "two one two".into(),
            selection: 4..7
        }
    );
    let mut f = Fixture::new("", [Region::caret(0)]);
    let mut registry = Registry::standard();
    registry
        .run("insert", &json!({"characters":"a"}), &mut f)
        .unwrap();
    registry
        .run("insert", &json!({"characters":"b"}), &mut f)
        .unwrap();
    registry.run("undo", &json!({}), &mut f).unwrap();
    assert_eq!(f.buffer.text().to_string(), "");
    f.settings.translate_tabs_to_spaces = true;
    run(&mut f, "insert_best_completion", json!({}));
    assert_eq!(f.buffer.text().to_string(), "    ");
}
#[test]
fn registry_domains_chains_macros_repeat_and_window_actions() {
    let mut registry = Registry::standard();
    let mut f = Fixture::new("a\na", [Region::caret(1), Region::caret(3)]);
    assert_eq!(registry.kind("insert"), Some(Kind::TextCommand));
    assert_eq!(registry.kind("save"), Some(Kind::WindowCommand));
    assert_eq!(registry.kind("new_window"), Some(Kind::ApplicationCommand));
    registry
        .run("toggle_record_macro", &json!({}), &mut f)
        .unwrap();
    registry
        .run("insert", &json!({"characters":"!"}), &mut f)
        .unwrap();
    registry
        .run("toggle_record_macro", &json!({}), &mut f)
        .unwrap();
    registry.run("run_macro", &json!({}), &mut f).unwrap();
    assert_eq!(f.buffer.text().to_string(), "a!!\na!!");
    registry.run("redo_or_repeat", &json!({}), &mut f).unwrap();
    assert_eq!(f.buffer.text().to_string(), "a!!!\na!!!");
    registry
        .run(
            "chain",
            &json!({"commands":[["save",{}],["next_view",{}]]}),
            &mut f,
        )
        .unwrap();
    assert_eq!(f.actions, ["save", "next_view"]);
    for name in [
        "new_file",
        "prompt_open_file",
        "open_file",
        "prompt_save_as",
        "save_all",
        "close",
        "close_file",
        "prev_view",
        "select_by_index",
        "new_window",
        "close_window",
        "increase_font_size",
        "decrease_font_size",
        "reset_font_size",
        "scroll_lines",
        "show_at_center",
    ] {
        registry.run(name, &json!({}), &mut f).unwrap();
        assert_eq!(f.actions.last().unwrap(), name);
    }
    assert!(registry.run("unknown", &json!({}), &mut f).is_err());
    assert!(registry.run("insert", &json!({}), &mut f).is_err());
}
