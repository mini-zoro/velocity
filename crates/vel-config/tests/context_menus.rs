use std::{collections::BTreeSet, fs, io::Write, path::PathBuf};
use vel_config::{
    json,
    keymap::Invocation,
    resources::Resources,
    ui::{self, CommandState, MenuItem},
};

fn insert(resources: &mut Resources, package: &str, filename: &str, source: &str) {
    resources.insert(
        &format!("Packages/{package}/{filename}"),
        source.as_bytes().to_vec(),
        PathBuf::from("<test>"),
    );
}

#[test]
fn menus_layer_default_package_user_and_ignore_unrelated_filenames() {
    let mut resources = Resources::default();
    insert(
        &mut resources,
        "Default",
        "Context.sublime-menu",
        r#"[
        {"id":"edit","caption":"Edit","children":[{"id":"copy","caption":"Copy","command":"copy","args":{"mode":"base"}}]},
        {"caption":"Original","command":"base"},
    ]"#,
    );
    insert(
        &mut resources,
        "Zulu",
        "Context.sublime-menu",
        r#"[{"caption":"Last package","command":"z"}]"#,
    );
    insert(
        &mut resources,
        "Alpha",
        "Context.sublime-menu",
        r#"[{"id":"edit","children":[{"id":"copy","caption":"Copy Special","command":"copy_special","args":{"mode":"package"}},{"caption":"Extra","command":"extra"}]}]"#,
    );
    insert(
        &mut resources,
        "User",
        "Context.sublime-menu",
        r#"[{"id":"edit","caption":"User Edit","children":[{"id":"copy","args":{"mode":"user"}}]},{"caption":"User last","command":"last"}]"#,
    );
    insert(
        &mut resources,
        "User",
        "Tab Context.sublime-menu",
        r#"[{"command":"wrong_menu"}]"#,
    );
    let items = ui::load_menu(&resources, "Context.sublime-menu").unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].caption, "User Edit");
    assert_eq!(items[0].children.len(), 2);
    let copy = &items[0].children[0];
    assert_eq!(copy.caption, "Copy Special");
    assert_eq!(copy.command.as_deref(), Some("copy_special"));
    assert_eq!(copy.args, json!({"mode":"user"}));
    assert_eq!(items[1].command.as_deref(), Some("base"));
    assert_eq!(items[2].command.as_deref(), Some("z"));
    assert_eq!(items[3].command.as_deref(), Some("last"));
    resources.set_ignored(BTreeSet::from(["Alpha".into(), "User".into()]));
    let items = ui::load_menu(&resources, "Context.sublime-menu").unwrap();
    assert_eq!(items[0].children[0].command.as_deref(), Some("copy"));
    assert_eq!(items[0].children[0].args, json!({"mode":"user"}));
}

#[test]
fn loose_menu_overrides_its_archive_then_user_extends_it() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("Installed Packages")).unwrap();
    fs::create_dir_all(dir.path().join("Packages/Alpha")).unwrap();
    fs::create_dir_all(dir.path().join("Packages/User")).unwrap();
    let archive =
        fs::File::create(dir.path().join("Installed Packages/Alpha.sublime-package")).unwrap();
    let mut zip = zip::ZipWriter::new(archive);
    zip.start_file(
        "Context.sublime-menu",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(br#"[{"caption":"Archive","command":"archive"}]"#)
        .unwrap();
    zip.finish().unwrap();
    fs::write(
        dir.path().join("Packages/Alpha/Context.sublime-menu"),
        "[\r\n  // Loose overrides the archive\r\n  {\"id\":\"entry\",\"caption\":\"Loose\",\"command\":\"loose\"},\r\n]\r\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("Packages/User/Context.sublime-menu"),
        "[\r\n  {\"id\":\"entry\",\"caption\":\"User caption\"},\r\n]\r\n",
    )
    .unwrap();
    let mut resources = Resources::default();
    resources.load(dir.path()).unwrap();
    let items = ui::load_menu(&resources, "Context.sublime-menu").unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].caption, "User caption");
    assert_eq!(items[0].command.as_deref(), Some("loose"));
}

#[test]
fn malformed_layer_reports_provenance_and_does_not_modify_other_resources() {
    let mut resources = Resources::default();
    insert(
        &mut resources,
        "Default",
        "Context.sublime-menu",
        r#"[{"command":"copy"}]"#,
    );
    insert(&mut resources, "Broken", "Context.sublime-menu", "[{broken");
    let error = ui::load_menu(&resources, "Context.sublime-menu").unwrap_err();
    assert!(error.contains("Packages/Broken/Context.sublime-menu"));
    resources.set_ignored(BTreeSet::from(["Broken".into()]));
    assert_eq!(
        ui::load_menu(&resources, "Context.sublime-menu")
            .unwrap()
            .len(),
        1
    );
    assert!(
        ui::load_menu(&resources, "Missing.sublime-menu")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn state_and_dispatch_share_context_args_and_hidden_rows_do_not_leave_separators() {
    let items: Vec<MenuItem> = ui::parse(r#"[
        {"caption":"-"},
        {"caption":"Invisible","command":"hidden"},
        {"caption":"-"},
        {"id":"disabled","caption":"Unavailable","command":"disabled","args":{"custom":42,"paths":["stale"],"group":99}},
        {"caption":"-"},{"caption":"-"},
        {"caption":"Checked","command":"toggle","args":{"setting":"spell_check"}},
        {"caption":"Gone submenu","children":[{"caption":"Hidden","command":"hidden"}]},
        {"caption":"-"}
    ]"#).unwrap();
    let context = json!({"paths":["/tmp/right"],"group":2,"index":4,"event":{"x":12.5,"y":7.0}});
    let mut evaluated = Vec::new();
    let rows = ui::resolve_menu(&items, "OSX", &context, |invocation| {
        evaluated.push(invocation.clone());
        CommandState {
            enabled: invocation.command != "disabled",
            visible: invocation.command != "hidden",
            checked: invocation.command == "toggle",
        }
    });
    assert_eq!(
        rows.iter().map(|r| r.caption.as_str()).collect::<Vec<_>>(),
        ["Unavailable", "-", "Checked"]
    );
    assert!(!rows[0].enabled);
    assert!(rows[2].enabled && rows[2].checked);
    let routed = rows[0].invocation.as_ref().unwrap();
    assert_eq!(routed.args["custom"], 42);
    // Concrete resource values are kept; absent keys come from the click.
    assert_eq!(routed.args["paths"], json!(["stale"]));
    assert_eq!(routed.args["group"], 99);
    for key in ["index", "event"] {
        assert_eq!(routed.args[key], context[key]);
    }
    assert!(evaluated.contains(routed));
    assert_eq!(
        rows[2].invocation.as_ref().unwrap().args["setting"],
        "spell_check"
    );
    assert!(rows[1].invocation.is_none());
}

#[test]
fn submenus_reflect_enabled_children_and_platform_filters_at_every_level() {
    let items: Vec<MenuItem> = ui::parse(r#"[
        {"caption":"Disabled submenu","children":[{"caption":"Disabled","command":"disabled"}]},
        {"caption":"Platform submenu","children":[
            {"caption":"Mac","command":"mac","platform":"OSX"},
            {"caption":"Other","command":"other","platform":"!OSX"},
            {"caption":"Windows","command":"win","platform":"Windows"}
        ]},
        {"caption":"Hidden parent","command":"hidden","children":[{"caption":"Never evaluate","command":"child"}]}
    ]"#).unwrap();
    for (platform, expected) in [
        ("OSX", vec!["Mac"]),
        ("Linux", vec!["Other"]),
        ("Windows", vec!["Other", "Windows"]),
    ] {
        let rows = ui::resolve_menu(&items, platform, &json!({}), |invocation| {
            assert_ne!(invocation.command, "child");
            CommandState {
                enabled: invocation.command != "disabled",
                visible: invocation.command != "hidden",
                checked: false,
            }
        });
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].enabled);
        assert!(rows[1].enabled);
        assert_eq!(
            rows[1]
                .children
                .iter()
                .map(|r| r.caption.as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn nonobject_args_are_normalized_without_losing_the_command() {
    let invocation = Invocation {
        command: "copy".into(),
        args: json!(null),
    };
    assert_eq!(
        ui::context_invocation(&invocation, &json!({"index": 3})),
        Invocation {
            command: "copy".into(),
            args: json!({"index":3})
        }
    );
    assert_eq!(
        ui::context_invocation(&invocation, &json!(false)).args,
        json!({})
    );
}

#[test]
fn click_context_fills_placeholders_but_keeps_concrete_resource_args() {
    let invocation = |args| Invocation {
        command: "move_to_group".into(),
        args,
    };
    let context = json!({"group": 2, "index": 5, "paths": ["/p/a"], "event": {"x": 1, "y": 2},
        "_context_menu": "tab"});
    let merged = ui::context_invocation(
        &invocation(json!({"group": 1, "index": -1, "paths": [], "_context_menu": "x"})),
        &context,
    );
    assert_eq!(merged.args["group"], 1, "a concrete resource value wins");
    assert_eq!(merged.args["index"], 5, "-1 asks for the clicked index");
    assert_eq!(
        merged.args["paths"],
        json!(["/p/a"]),
        "[] asks for the clicked paths"
    );
    assert_eq!(merged.args["event"], json!({"x": 1, "y": 2}));
    assert_eq!(
        merged.args["_context_menu"], "tab",
        "internal keys come from the click"
    );
    let open = ui::context_invocation(&invocation(json!({"group": null})), &context);
    assert_eq!(open.args["group"], 2);
}
