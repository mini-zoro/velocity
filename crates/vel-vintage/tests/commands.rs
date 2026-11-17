use vel_buffer::{Buffer, MonospaceLayout, Region, Selection};
use vel_vintage::{Key, Mode, Outcome, Settings, Vintage};
fn marked(input: &str) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut carets = Vec::new();
    for c in input.chars() {
        if c == '|' {
            carets.push(text.len());
        } else {
            text.push(c);
        }
    }
    (text, carets)
}
fn keys(input: &str) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut rest = input;
    while !rest.is_empty() {
        if rest.starts_with('<')
            && let Some(end) = rest.find('>')
        {
            let key = match &rest[1..end] {
                "Esc" => Some(Key::Escape),
                "CR" => Some(Key::Enter),
                "BS" => Some(Key::Backspace),
                "C-r" => Some(Key::Ctrl('r')),
                "C-v" => Some(Key::Ctrl('v')),
                _ => None,
            };
            if let Some(key) = key {
                keys.push(key);
                rest = &rest[end + 1..];
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        keys.push(Key::Char(c));
        rest = &rest[c.len_utf8()..];
    }
    keys
}
fn fixture(input: &str) -> (Vintage, Buffer, vel_buffer::ViewId) {
    let (text, points) = marked(input);
    let mut buffer = Buffer::new(text.as_str());
    let view = buffer.new_view();
    buffer.set_selection(
        view,
        Selection::from_regions(points.into_iter().map(Region::caret)),
    );
    let vi = Vintage::new(Settings {
        vintage_start_in_command_mode: true,
        vintage_ctrl_keys: true,
        ..Settings::default()
    });
    (vi, buffer, view)
}
fn run(initial: &str, commands: &str, expected: &str) {
    let (mut vi, mut buffer, view) = fixture(initial);
    for key in keys(commands) {
        vi.handle_key(key, &mut buffer, view, &MonospaceLayout::default());
    }
    let (text, carets) = marked(expected);
    assert_eq!(
        buffer.text().to_string(),
        text,
        "text: {initial:?} {commands:?}"
    );
    let actual = buffer
        .view(view)
        .sel()
        .iter()
        .map(|r| r.b)
        .collect::<Vec<_>>();
    assert_eq!(actual, carets, "carets: {initial:?} {commands:?}");
}
#[test]
fn command_table() {
    let mut failed = Vec::new();
    for (initial, command, expected) in [
        ("|abc", "l", "a|bc"),
        ("a|bc", "h", "|abc"),
        ("|a\nb\nc", "2j", "a\nb\n|c"),
        ("a\nb\n|c", "2k", "|a\nb\nc"),
        ("|one two three", "2w", "one two |three"),
        ("one two |three", "2b", "|one two three"),
        ("|one two", "e", "on|e two"),
        ("one |two", "ge", "on|e two"),
        ("|one.two three", "W", "one.two |three"),
        ("one.two |three", "B", "|one.two three"),
        ("|one.two three", "E", "one.tw|o three"),
        ("  a|bc", "0", "|  abc"),
        ("|  abc", "^", "  |abc"),
        ("|abc", "$", "ab|c"),
        ("a\nb\n|c", "gg", "|a\nb\nc"),
        ("|a\nb\nc", "G", "a\nb\n|c"),
        ("|a\nb\nc", "2G", "a\n|b\nc"),
        ("|abcabc", "fc", "ab|cabc"),
        ("|abcabc", "tc", "a|bcabc"),
        ("abcab|c", "Fa", "abc|abc"),
        ("abcab|c", "Ta", "abca|bc"),
        ("|abcabc", "fc;", "abcab|c"),
        ("|abcabc", "fc;,", "ab|cabc"),
        ("|(a(b)c)", "%", "(a(b)c|)"),
        ("a\n\n|b\n\nc", "{", "a\n|\nb\n\nc"),
        ("|a\n\nb\n\nc", "}", "a\n|\nb\n\nc"),
        ("|One. Two! Three?", " )".trim(), "One. |Two! Three?"),
        ("One. |Two! Three?", "(", "|One. Two! Three?"),
        ("|abc", "x", "|bc"),
        ("a|bc", "X", "|bc"),
        ("|abc", "2x", "|c"),
        ("|one two", "dw", "|two"),
        ("|one two three", "2d2w", "|"),
        ("|a\nb\nc", "dd", "|b\nc"),
        ("a\n|b", "dd", "|a"),
        ("|abc\ndef", "D", "|\ndef"),
        ("|one two", "cwX<Esc>", "|X two"),
        ("|abc", "sX<Esc>", "|Xbc"),
        ("|abc\ndef", "Shello<Esc>", "hell|o\ndef"),
        ("a|bc", "CY<Esc>", "a|Y"),
        ("|abc", "rX", "|Xbc"),
        ("|abc", "2rX", "X|Xc"),
        ("|a|bcd", "2rX", "XX|Xd"),
        ("|a|bcd", "2rXu", "|a|bcd"),
        ("|abc", "vlrX", "|XXc"),
        ("|a\nb", "VjrX", "|X\nX"),
        ("|e\u{301}界", "vlrX", "|XX"),
        (
            "|e\u{301}界👩‍💻 x\nabc () <>\n\nline",
            "lfhvbrh*",
            "|hh👩‍💻 x\nabc () <>\n\nline",
        ),
        ("|abc", "iXY<Esc>", "X|Yabc"),
        ("|abc", "aXY<Esc>", "aX|Ybc"),
        (" a|bc", "IX<Esc>", " |Xabc"),
        ("|abc", "AX<Esc>", "abc|X"),
        ("|abc", "oX<Esc>", "abc\n|X"),
        ("|abc", "OX<Esc>", "|X\nabc"),
        ("|abc", "RXY<Esc>", "X|Yc"),
        ("|abc", "3iX<Esc>", "XX|Xabc"),
        ("|abc", "3ie\u{301}<BS><Esc>", "|abc"),
        ("|abc", "~", "A|bc"),
        ("|ABC def", "guw", "|abc def"),
        ("|abc def", "gUw", "|ABC def"),
        ("|Abc def", "g~w", "|aBC def"),
        ("|a\nb", "J", "a| b"),
        ("|a\nb\nc", "3J", "a| b c"),
        ("|one two", "yiwp", "oon|ene two"),
        ("|a\nb", "yyP", "|a\na\nb"),
        ("|a\nb", "Yp", "a\n|a\nb"),
        ("|one two", "\"ayiw$\"ap", "one twoon|e"),
        ("|one two", "yiw$\"0p", "one twoon|e"),
        ("|one two", "yiw$\"\"p", "one twoon|e"),
        ("|a\nb", "ddp", "b\n|a\n"),
        ("|abc", "x.", "|c"),
        ("|abc", "iX<Esc>l.", "X|Xabc"),
        ("|abc", "iHello, world!<Esc>u", "|abc"),
        ("|abc", "cwhello, world!<Esc>u", "|abc"),
        ("|abc", "xu<C-r>", "|bc"),
        ("|abc", "qaxq@a", "|c"),
        ("|abc", "mal`a", "|abc"),
        ("|abc\ndef", "majl'a", "|abc\ndef"),
        ("|one two one", "/one<CR>", "one two |one"),
        ("|one two one", "/one<CR>n", "|one two one"),
        ("|one two one", "?one<CR>N", "|one two one"),
        ("|one two one", "*", "one two |one"),
        ("one two |one", "#", "|one two one"),
        ("|a\nb", "2<CR>", "a\n|b"),
        ("|a\nb\nc", ":2<CR>", "a\n|b\nc"),
        ("|abc", "H", "|abc"),
        ("|a\nb\nc", "M", "a\n|b\nc"),
        ("|a\nb\nc", "L", "a\nb\n|c"),
        ("|abc", "vlx", "|c"),
        ("|abc\ndef", "Vd", "|def"),
        ("|ab\ncd", "<C-v>jld", "|\n|"),
        ("|one |two", "dw", "|"),
        ("|e\u{301}界👩‍💻!", "x", "|界👩‍💻!"),
        ("e\u{301}|界👩‍💻!", "l", "e\u{301}界|👩‍💻!"),
    ] {
        if std::panic::catch_unwind(|| run(initial, command, expected)).is_err() {
            failed.push((initial, command));
        }
    }
    assert!(failed.is_empty(), "{failed:?}");
}
#[test]
fn text_object_table() {
    for (initial, keys, expected) in [
        ("|one two", "diw", "| two"),
        ("one |two", "daw", "on|e"),
        ("|one.two x", "diW", "| x"),
        ("|one.two x", "daW", "|x"),
        ("(|one)", "di(", "(|)"),
        ("(|one)", "da(", "|"),
        ("[|one]", "di[", "[|]"),
        ("[|one]", "da[", "|"),
        ("{|one}", "di{", "{|}"),
        ("{|one}", "da{", "|"),
        ("<|one>", "di<", "<|>"),
        ("<|one>", "da<", "|"),
        ("\"|one\"", "di\"", "\"|\""),
        ("\"|one\"", "da\"", "|"),
        ("'|one'", "di'", "'|'"),
        ("'|one'", "da'", "|"),
        ("`|one`", "di`", "`|`"),
        ("`|one`", "da`", "|"),
        ("<a>|one</a>", "dit", "<a>|</a>"),
        ("<a>|one</a>", "dat", "|"),
        ("|One. Two.", "dis", "| Two."),
        ("|One. Two.", "das", "|Two."),
        ("|one\ntwo\n\nthree", "dip", "|\nthree"),
        ("|one\ntwo\n\nthree", "dap", "|three"),
    ] {
        run(initial, keys, expected);
    }
}
#[test]
fn indent_contexts_ex_requests_and_clipboard() {
    let (mut vi, mut b, v) = fixture("|one\ntwo");
    let layout = MonospaceLayout::default();
    for k in keys(">>") {
        vi.handle_key(k, &mut b, v, &layout);
    }
    assert_eq!(b.text().to_string(), "    one\ntwo");
    for k in keys("<<") {
        vi.handle_key(k, &mut b, v, &layout);
    }
    assert_eq!(b.text().to_string(), "one\ntwo");
    vi.handle_key('=', &mut b, v, &layout);
    assert!(vi.context("vi_has_action"));
    assert!(
        matches!(vi.handle_key('=',&mut b,v,&layout),Outcome::NeedsCommand(c) if c.name=="reindent")
    );
    for (command, name) in [
        (":w<CR>", "save"),
        (":wq<CR>", "save_and_close"),
        (":q<CR>", "close"),
        (":e file.rs<CR>", "open_file"),
    ] {
        let mut outcome = Outcome::Handled;
        for k in keys(command) {
            outcome = vi.handle_key(k, &mut b, v, &layout);
        }
        assert!(matches!(outcome,Outcome::NeedsCommand(c) if c.name==name));
    }
    let mut clip = Vintage::new(Settings {
        vintage_start_in_command_mode: true,
        vintage_use_clipboard: true,
        ..Settings::default()
    });
    clip.set_clipboard("hi");
    clip.handle_key('p', &mut b, v, &layout);
    for k in keys("yiw") {
        clip.handle_key(k, &mut b, v, &layout);
    }
    assert!(clip.register('+').is_some());
    assert_eq!(Vintage::default().mode(), Mode::Insert);
}
use proptest::prelude::*;
proptest! {
    #[test]
    fn random_commands_preserve_utf8_selections(
        actions in prop::collection::vec(0u8..160, 0..240)
    ) {
        let (mut vi, mut buffer, view) = fixture("|e\u{301}界👩‍💻 x\nabc () <>\n\nline");
        let alphabet = "hjklwWbBeE0^$GfFtT;,{}()%diayc<>g~uUxXsSCDYpPoOaAiIrRvVq@m'`\"123/?:nN*#.="
            .chars().collect::<Vec<_>>();
        for action in actions {
            let key = match action {
                150 => Key::Escape,
                151 => Key::Enter,
                152 => Key::Backspace,
                153 => Key::Ctrl('r'),
                154 => Key::Ctrl('v'),
                _ => Key::Char(alphabet[action as usize % alphabet.len()]),
            };
            vi.handle_key(key, &mut buffer, view, &MonospaceLayout::default());
            for region in buffer.view(view).sel().iter() {
                prop_assert!(region.end() <= buffer.text().len_bytes());
                prop_assert!(buffer.text().is_char_boundary(region.a));
                prop_assert!(buffer.text().is_char_boundary(region.b));
                for point in [region.a, region.b] {
                    if point < buffer.text().len_bytes() {
                        prop_assert_eq!(
                            buffer.text().prev_grapheme_boundary(
                                buffer.text().next_grapheme_boundary(point)
                            ), point
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn unicode_multicursor_undo_macro_marks_and_failed_motions() {
    // Regression from the property test: ~ on uncased Han creates no buffer edit.
    // Undo must undo the put, whose old caret is valid in the shorter Unicode text.
    run(
        "|e\u{301}界👩‍💻 x\nabc () <>\n\nline",
        "kk2xP~u",
        "|👩‍💻 x\nabc () <>\n\nline",
    );
    run("|one |two", "cwi界!<Esc>u", "|one |two");
    run("|abc", "Rxy<BS><Esc>", "|xbc");
    run("|abc", "dfz", "|abc");
    run("|\n|abc", "x", "|\n|bc");
    run("|abc", "vio<Esc>", "|abc"); // invalid object cancels without editing
    run("|one two", "viwd", "| two");
    run("|one two three", "gUw", "|ONE two three");
    run("|one\ntwo", "gUU", "|ONE\ntwo");
    let (mut vi, mut buffer, view) = fixture("|abc def");
    let layout = MonospaceLayout::default();
    for k in keys("maA!<Esc>`a") {
        vi.handle_key(k, &mut buffer, view, &layout);
    }
    assert_eq!(buffer.view(view).sel().primary().unwrap().b, 0);
    vi.set_viewport(1, 1);
    vi.handle_key('H', &mut buffer, view, &layout);
    for k in keys("qa@aq@a") {
        vi.handle_key(k, &mut buffer, view, &layout);
    } // recursive replay has a fixed budget
    for region in buffer.view(view).sel().iter() {
        assert!(region.end() <= buffer.text().len_bytes());
    }
}
#[test]
fn host_reindent_completion_keeps_history_and_restores_carets() {
    let (mut vi, mut buffer, view) = fixture("|one");
    let layout = MonospaceLayout::default();
    vi.handle_key('=', &mut buffer, view, &layout);
    assert!(
        matches!(vi.handle_key('=',&mut buffer,view,&layout),Outcome::NeedsCommand(c) if c.name=="reindent")
    );
    vel_buffer::ops::indent(&mut buffer, view);
    vi.complete_host_command(&mut buffer, view, 1);
    assert_eq!(buffer.text().to_string(), "    one");
    vi.handle_key('u', &mut buffer, view, &layout);
    assert_eq!(buffer.text().to_string(), "one");
    assert_eq!(buffer.view(view).sel().primary().unwrap().b, 0);
}

#[test]
fn till_repetition_and_primary_selection_are_preserved() {
    run("|abcabc", "tc;", "abca|bc");
    run("abcab|c", "Ta;", "a|bcabc");
    let (mut vi, mut buffer, view) = fixture("|abc\n|def");
    let layout = MonospaceLayout::default();
    buffer.set_selection(
        view,
        Selection::from_regions([Region::caret(4), Region::caret(0)]),
    );
    vi.handle_key('l', &mut buffer, view, &layout);
    assert_eq!(buffer.view(view).sel().primary().unwrap().b, 1);
    vi.handle_key('x', &mut buffer, view, &layout);
    assert_eq!(buffer.view(view).sel().primary().unwrap().b, 1);
}

#[test]
fn search_counts_cancel_and_replace_backspace_history() {
    run("|one two one two one", "2/one<CR>", "one two one two |one");
    run("|one two one", "?one<CR>/<CR>", "|one two one");
    run("|one two one", "?one<CR>/<CR>n", "one two |one");
    run("|abc", "Rxy<BS><Esc>u<C-r><C-r>", "|xbc");
    run("|abc", "R<BS><Esc>", "|abc");
    let (mut vi, mut buffer, view) = fixture("|abc");
    let layout = MonospaceLayout::default();
    for key in keys("v/nope<Esc>") {
        vi.handle_key(key, &mut buffer, view, &layout);
    }
    assert_eq!(vi.mode(), Mode::VisualChar);
}

#[test]
fn insert_backspace_deletes_external_selection() {
    let (mut vi, mut buffer, view) = fixture("|abc");
    let layout = MonospaceLayout::default();
    vi.handle_key('i', &mut buffer, view, &layout);
    buffer.set_selection(view, Selection::from_regions([Region::new(1, 3)]));
    vi.handle_key(Key::Backspace, &mut buffer, view, &layout);
    assert_eq!(buffer.text().to_string(), "a");
    vi.handle_key(Key::Escape, &mut buffer, view, &layout);
    vi.handle_key('u', &mut buffer, view, &layout);
    assert_eq!(buffer.text().to_string(), "abc");
}

#[test]
fn dot_remembers_noop_change_without_adding_an_undo_group() {
    for (initial, command, expected) in [
        ("|first\nb\nlast\n", "ddrb.", "|b\nlast\n"),
        ("|first\n123\nlast\n", "dd~.", "12|3\nlast\n"),
        ("|first\nlower\nlast\n", "ddguu.", "|lower\nlast\n"),
    ] {
        run(initial, command, expected);
        // The no-op dot target creates no undo step; undo still restores dd.
        run(initial, &(command.to_owned() + "u"), initial);
    }
}
