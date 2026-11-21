//! Additive text adapters. Registry orchestration (including chain) stays in lib.rs.
use super::{Host, Kind, Registry, boolean, string};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use vel_buffer::ops::{self, IdentCase, Permutation};

pub(super) fn register(registry: &mut Registry) {
    for (name, handler) in [
        ("wrap_lines", wrap as super::Handler),
        ("permute_lines", lines),
        ("permute_selection", selections),
        ("sort_selection", sort),
        ("convert_ident_case", convert),
        ("snake_case", snake),
        ("camel_case", camel),
        ("pascal_case", pascal),
        ("kebab_case", kebab),
        ("upper_snake_case", upper_snake),
        ("set_mark", set_mark),
        ("select_to_mark", select_mark),
        ("swap_with_mark", swap_mark),
        ("clear_bookmarks", clear),
        ("toggle_bookmark", toggle),
        ("next_bookmark", next),
        ("prev_bookmark", previous),
        ("select_bookmarks", bookmarks),
        ("select_all_bookmarks", bookmarks),
        ("expand_selection", expand),
        ("expand_selection_to_paragraph", paragraph),
        ("trim_trailing_white_space", trim),
        ("trim_trailing_whitespace", trim),
        ("noop", noop),
        ("append", append),
        ("split_selection", split),
        ("invert_selection", invert),
        ("rot13", rot13),
    ] {
        registry.register(name, Kind::TextCommand, Some(handler));
    }
}
fn permutation(args: &Value) -> Result<Permutation, String> {
    static SEED: AtomicU64 = AtomicU64::new(1);
    Ok(match string(args, "operation", "reverse") {
        "reverse" => Permutation::Reverse,
        "unique" => Permutation::Unique,
        "shuffle" => Permutation::Shuffle(args["seed"].as_u64().unwrap_or_else(|| {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64)
                ^ SEED.fetch_add(1, Ordering::Relaxed)
        })),
        other => return Err(format!("unsupported permutation {other}")),
    })
}
fn wrap(host: &mut dyn Host, args: &Value) -> Result<(), String> {
    if host.intercept_text_command("wrap_lines", args)? {
        return Ok(());
    }
    let settings = host.settings();
    // The public command reference documents an explicit width, but does not establish
    // a fallback column: retain our existing 70 when neither setting nor ruler is set.
    // https://docs.sublimetext.io/reference/commands.html#wrap-lines
    let default_width = host
        .wrap_width()
        .filter(|n| *n > 0)
        .or_else(|| settings.rulers.first().copied().filter(|n| *n > 0))
        .unwrap_or(70);
    let width =
        args.get("width")
            .or_else(|| args.get("column"))
            .map_or(Ok(default_width), |v| {
                v.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .ok_or("wrap width must be a positive integer")
            })?;
    let prefixes = host.wrap_comment_prefixes();
    let (b, id) = host.buffer();
    sync_settings(b, id, &settings);
    ops::wrap_lines_with_prefixes(b, id, width, &prefixes);
    Ok(())
}
fn lines(host: &mut dyn Host, args: &Value) -> Result<(), String> {
    if args["operation"] == "sort" {
        return super::text_command("permute_lines", args, host);
    }
    if host.intercept_text_command("permute_lines", args)? {
        return Ok(());
    }
    let operation = permutation(args)?;
    let (b, id) = host.buffer();
    ops::permute_lines(b, id, operation);
    Ok(())
}
fn selections(host: &mut dyn Host, args: &Value) -> Result<(), String> {
    if host.intercept_text_command("permute_selection", args)? {
        return Ok(());
    }
    let operation = permutation(args)?;
    let (b, id) = host.buffer();
    ops::permute_selection(b, id, operation);
    Ok(())
}
fn sort(host: &mut dyn Host, args: &Value) -> Result<(), String> {
    if host.intercept_text_command("sort_selection", args)? {
        return Ok(());
    }
    let (b, id) = host.buffer();
    ops::sort_selection(
        b,
        id,
        args["case_sensitive"].as_bool().unwrap_or(true),
        boolean(args, "reverse"),
    );
    Ok(())
}
fn case(host: &mut dyn Host, args: &Value, name: &str, style: IdentCase) -> Result<(), String> {
    if host.intercept_text_command(name, args)? {
        return Ok(());
    }
    let settings = host.settings();
    let (b, id) = host.buffer();
    sync_settings(b, id, &settings);
    ops::convert_ident_case(b, id, style);
    Ok(())
}
fn sync_settings(
    b: &mut vel_buffer::Buffer,
    id: vel_buffer::ViewId,
    settings: &vel_config::settings::EditorSettings,
) {
    crate::sync_buffer_settings(b, id, settings);
}
fn convert(host: &mut dyn Host, args: &Value) -> Result<(), String> {
    // ST-BEHAVIOR: unverified [T5]: public changelog names the feature, not its
    // argument schema. Accept explicit styles; unknown spellings fail unchanged.
    let style = match string(args, "case", string(args, "to", "snake_case")) {
        "snake" | "snake_case" => IdentCase::Snake,
        "camel" | "camelCase" | "camel_case" => IdentCase::Camel,
        "pascal" | "PascalCase" | "pascal_case" => IdentCase::Pascal,
        "kebab" | "kebab-case" | "kebab_case" => IdentCase::Kebab,
        "upper_snake" | "UPPER_SNAKE" | "upper_snake_case" | "screaming_snake_case" => {
            IdentCase::UpperSnake
        }
        other => return Err(format!("unsupported identifier case {other}")),
    };
    case(host, args, "convert_ident_case", style)
}
fn snake(h: &mut dyn Host, a: &Value) -> Result<(), String> {
    case(h, a, "snake_case", IdentCase::Snake)
}
fn camel(h: &mut dyn Host, a: &Value) -> Result<(), String> {
    case(h, a, "camel_case", IdentCase::Camel)
}
fn pascal(h: &mut dyn Host, a: &Value) -> Result<(), String> {
    case(h, a, "pascal_case", IdentCase::Pascal)
}
fn kebab(h: &mut dyn Host, a: &Value) -> Result<(), String> {
    case(h, a, "kebab_case", IdentCase::Kebab)
}
fn upper_snake(h: &mut dyn Host, a: &Value) -> Result<(), String> {
    case(h, a, "upper_snake_case", IdentCase::UpperSnake)
}
fn expand(h: &mut dyn Host, a: &Value) -> Result<(), String> {
    if a["to"] == "paragraph" {
        paragraph(h, a)
    } else {
        super::text_command("expand_selection", a, h)
    }
}
macro_rules! adapter {
    ($function:ident,$name:literal,$operation:ident $(,$extra:expr)?)=>{
        fn $function(host:&mut dyn Host,args:&Value)->Result<(),String>{
            if host.intercept_text_command($name,args)?{return Ok(());}
            let(b,id)=host.buffer();ops::$operation(b,id $(,$extra)?);Ok(())
        }
    };
}
adapter!(set_mark, "set_mark", set_mark);
adapter!(select_mark, "select_to_mark", select_to_mark);
adapter!(swap_mark, "swap_with_mark", swap_with_mark);
fn clear(host: &mut dyn Host, args: &Value) -> Result<(), String> {
    if host.intercept_text_command("clear_bookmarks", args)? {
        return Ok(());
    }
    let (b, id) = host.buffer();
    ops::clear_named_regions(b, id, string(args, "name", ops::BOOKMARKS));
    Ok(())
}
adapter!(toggle, "toggle_bookmark", toggle_bookmark);
// Navigation has no settings dependency and preserves the host's selection goals.
fn next(h: &mut dyn Host, a: &Value) -> Result<(), String> {
    if h.intercept_text_command("next_bookmark", a)? {
        return Ok(());
    }
    let (b, id) = h.buffer();
    ops::next_bookmark(b, id, boolean(a, "extend") || boolean(a, "select"));
    Ok(())
}
fn previous(h: &mut dyn Host, a: &Value) -> Result<(), String> {
    if h.intercept_text_command("prev_bookmark", a)? {
        return Ok(());
    }
    let (b, id) = h.buffer();
    ops::prev_bookmark(b, id, boolean(a, "extend") || boolean(a, "select"));
    Ok(())
}
adapter!(bookmarks, "select_bookmarks", select_bookmarks);
adapter!(
    paragraph,
    "expand_selection_to_paragraph",
    expand_selection_to_paragraph
);
adapter!(trim, "trim_trailing_white_space", trim_trailing_white_space);
fn append(h: &mut dyn Host, a: &Value) -> Result<(), String> {
    if h.intercept_text_command("append", a)? {
        return Ok(());
    }
    let text = a["characters"].as_str().ok_or("append needs characters")?;
    let settings = h.settings();
    let (b, id) = h.buffer();
    if settings.translate_tabs_to_spaces
        && !boolean(a, "disable_tab_translation")
        && text.contains('\t')
    {
        let tab = settings.tab_size.max(1);
        let advance = |column, c| match c {
            '\n' | '\r' => 0,
            '\t' => column + (tab - column % tab),
            _ => column + 1,
        };
        let last = b.text().len_lines() - 1;
        let mut column = b.text().line(last).chars().fold(0, advance);
        let mut translated = String::with_capacity(text.len());
        for c in text.chars() {
            let next = advance(column, c);
            if c == '\t' {
                translated.extend(std::iter::repeat_n(' ', next - column));
            } else {
                translated.push(c);
            }
            column = next;
        }
        ops::append(b, id, &translated);
    } else {
        ops::append(b, id, text);
    }
    Ok(())
}
adapter!(split, "split_selection", split_selection);
adapter!(invert, "invert_selection", invert_selection);
adapter!(rot13, "rot13", rot13);
fn noop(_: &mut dyn Host, _: &Value) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vel_buffer::{Buffer, Region, Selection, ViewId};
    use vel_config::settings::EditorSettings;
    struct Fixture {
        buffer: Buffer,
        id: ViewId,
        finished: usize,
        intercepts: Vec<String>,
        settings: EditorSettings,
    }
    impl Fixture {
        fn new(text: &str) -> Self {
            let mut buffer = Buffer::new(text);
            let id = buffer.new_view();
            buffer.set_selection(id, Selection::from(Region::new(0, text.len())));
            Self {
                buffer,
                id,
                finished: 0,
                intercepts: Vec::new(),
                settings: EditorSettings::default(),
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
            String::new()
        }
        fn set_clipboard(&mut self, _: &str) {}
        fn action(&mut self, name: &str, _: &Value) -> Result<(), String> {
            Err(format!("unexpected host action {name}"))
        }
        fn command_finished(&mut self) {
            self.finished += 1;
        }
        fn intercept_text_command(&mut self, name: &str, _: &Value) -> Result<bool, String> {
            self.intercepts.push(name.into());
            Ok(false)
        }
    }
    #[test]
    fn r35_append_translates_tabs_unless_disabled() {
        let mut host = Fixture::new("αx");
        host.settings.tab_size = 4;
        host.settings.translate_tabs_to_spaces = true;
        let mut registry = Registry::standard();
        registry
            .run("append", &json!({"characters":"\tX\n\t"}), &mut host)
            .unwrap();
        assert_eq!(host.buffer.text().to_string(), "αx  X\n    ");
        registry
            .run(
                "append",
                &json!({
                    "characters":"\tY", "disable_tab_translation":true
                }),
                &mut host,
            )
            .unwrap();
        assert_eq!(host.buffer.text().to_string(), "αx  X\n    \tY");
        registry.run("undo", &json!({}), &mut host).unwrap();
        assert_eq!(host.buffer.text().to_string(), "αx  X\n    ");
        host.settings.translate_tabs_to_spaces = false;
        registry
            .run("append", &json!({"characters":"\tZ"}), &mut host)
            .unwrap();
        assert_eq!(host.buffer.text().to_string(), "αx  X\n    \tZ");
    }
    #[test]
    fn adapters_are_callable_and_chains_keep_registry_undo_macro_and_repeat() {
        let mut host = Fixture::new("HTTPServer");
        let mut registry = Registry::standard();
        registry
            .run(
                "chain",
                &json!({"commands":[["convert_ident_case",{"case":"snake_case"}],["upper_case"]]}),
                &mut host,
            )
            .unwrap();
        assert_eq!(host.buffer.text().to_string(), "HTTP_SERVER");
        assert_eq!(host.finished, 2);
        registry.run("undo", &json!({}), &mut host).unwrap();
        assert_eq!(host.buffer.text().to_string(), "http_server");
        registry.run("undo", &json!({}), &mut host).unwrap();
        assert_eq!(host.buffer.text().to_string(), "HTTPServer");
        registry
            .run("toggle_record_macro", &json!({}), &mut host)
            .unwrap();
        registry.run("snake_case", &json!({}), &mut host).unwrap();
        registry
            .run("toggle_record_macro", &json!({}), &mut host)
            .unwrap();
        registry.run("undo", &json!({}), &mut host).unwrap();
        registry.run("run_macro", &json!({}), &mut host).unwrap();
        assert_eq!(host.buffer.text().to_string(), "http_server");
        let nested = (0..34).fold(
            json!({"commands":[["noop"]]}),
            |args, _| json!({"commands":[["chain",args]]}),
        );
        assert!(
            registry
                .run("chain", &nested, &mut host)
                .unwrap_err()
                .contains("recursion")
        );
    }
    #[test]
    fn permutations_wrapping_and_paragraph_alias_use_one_intercept() {
        let mut registry = Registry::standard();
        for (name, args, text, expected) in [
            (
                "permute_lines",
                json!({"operation":"unique"}),
                "x\nx\ny\n",
                "x\ny\n",
            ),
            ("wrap_lines", json!({"width":4}), "a bb c", "a bb\nc"),
            ("trim_trailing_white_space", json!({}), "x \n", "x\n"),
            ("kebab_case", json!({}), "HTTPServer", "http-server"),
            ("camel_case", json!({}), "HTTP_SERVER", "httpServer"),
            ("pascal_case", json!({}), "http_server", "HttpServer"),
            ("upper_snake_case", json!({}), "httpServer", "HTTP_SERVER"),
        ] {
            let mut host = Fixture::new(text);
            registry.run(name, &args, &mut host).unwrap();
            assert_eq!(host.buffer.text().to_string(), expected, "{name}");
            assert_eq!(host.intercepts.len(), 1, "{name}");
        }
        for args in [json!({"to":"word"}), json!({"to":"paragraph"})] {
            let mut host = Fixture::new("a b\nc");
            registry.run("expand_selection", &args, &mut host).unwrap();
            assert_eq!(host.intercepts.len(), 1);
        }
        let mut host = Fixture::new("a\nb");
        host.buffer.set_selection(
            host.id,
            Selection::from_regions([Region::new(0, 1), Region::new(2, 3)]),
        );
        registry
            .run(
                "permute_selection",
                &json!({"operation":"reverse"}),
                &mut host,
            )
            .unwrap();
        assert_eq!(host.buffer.text().to_string(), "b\na");
        registry
            .run("sort_selection", &json!({}), &mut host)
            .unwrap();
        assert_eq!(host.buffer.text().to_string(), "a\nb");
        for name in [
            "set_mark",
            "select_to_mark",
            "swap_with_mark",
            "toggle_bookmark",
            "next_bookmark",
            "prev_bookmark",
            "select_bookmarks",
            "clear_bookmarks",
        ] {
            registry.run(name, &json!({}), &mut host).unwrap();
        }
    }
    #[test]
    fn invalid_arguments_leave_text_and_history_unchanged() {
        for (name, args) in [
            ("permute_lines", json!({"operation":"bad"})),
            ("wrap_lines", json!({"width":0})),
            ("convert_ident_case", json!({"case":"bad"})),
        ] {
            let mut host = Fixture::new("x");
            let version = host.buffer.change_id();
            assert!(Registry::standard().run(name, &args, &mut host).is_err());
            assert_eq!(host.buffer.change_id(), version);
            assert_eq!(host.buffer.text().to_string(), "x");
        }
    }

    #[test]
    fn wrap_uses_host_tab_stops_and_default_ruler() {
        let mut host = Fixture::new("\tone two");
        host.settings.rulers = vec![7];
        host.settings.tab_size = 4;
        Registry::standard()
            .run("wrap_lines", &json!({}), &mut host)
            .unwrap();
        assert_eq!(host.buffer.text().to_string(), "\tone\n\ttwo");
    }

    #[test]
    fn coverage_counts_and_registration_claims_match_the_standard_registry() {
        let registry = Registry::standard();
        let mut counts = std::collections::BTreeMap::new();
        for line in include_str!("../COVERAGE.md")
            .lines()
            .filter(|line| line.starts_with("| `"))
        {
            let fields: Vec<_> = line.split('|').map(str::trim).collect();
            let name = fields[1].trim_matches('`');
            let status = fields[2];
            *counts.entry(status).or_insert(0) += 1;
            assert_eq!(
                registry.kind(name).is_some(),
                matches!(status, "implemented" | "partially"),
                "{name}: {status}"
            );
        }
        assert_eq!(
            counts,
            std::collections::BTreeMap::from([
                ("implemented", 61),
                ("partially", 136),
                ("missing", 10),
                ("out of scope", 2)
            ])
        );
    }
}
