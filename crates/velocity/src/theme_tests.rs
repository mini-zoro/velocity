//! Structural regression, lifecycle and profile tests; no AppKit/Metal initialization.
use crate::{
    editor::Editor,
    panels::{Overlay, Panels},
    view::View,
    workspace::Workspace,
};
use vel_config::{json, settings::Layer};
use vel_platform::{Point, Rect, Size};
use vel_render::scene::{Scene, Surface};
use vel_text::Text;
fn workspace() -> Workspace {
    Workspace::new(Text::new(), None, None, false).unwrap()
}
fn view() -> View {
    View::new(
        Size {
            width: 900.,
            height: 600.,
        },
        7.8,
        12.,
    )
}
#[test]
fn sidebar_cap_matches_themed_sidebar_in_light_and_dark() {
    let mut workspace = workspace();
    let mut panels = Panels::new(&workspace, false);
    panels.side_visible = true;
    panels.layout(Size {
        width: 900.,
        height: 600.,
    });
    for dark in [false, true] {
        workspace.set_appearance(dark).unwrap();
        let mut scene = Scene::default();
        panels.paint(&workspace, &mut scene);
        let sidebar = panels.tree.nodes[&3].rect;
        let fills: Vec<_> = scene
            .layers
            .iter()
            .flat_map(|layer| &layer.surfaces)
            .filter_map(|surface| match surface {
                Surface::Fill(fill) => Some(fill),
                _ => None,
            })
            .collect();
        let cap = fills
            .iter()
            .find(|fill| {
                fill.rect.x == 0.
                    && fill.rect.y == 0.
                    && fill.rect.width == sidebar.width
                    && fill.rect.height == sidebar.y
            })
            .expect("themed sidebar cap");
        let body = fills
            .iter()
            .find(|fill| fill.rect == sidebar)
            .expect("themed sidebar body");
        assert_eq!(cap.color, body.color);
        assert!(cap.color[3] > 0. && cap.color[..3].iter().any(|c| *c > 0.));
    }
}
#[test]
fn sidebar_root_removal_updates_project_and_survives_resource_reload() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let folder = root.join("configured");
    let added = root.join("runtime");
    std::fs::create_dir(&folder).unwrap();
    std::fs::create_dir(&added).unwrap();
    std::fs::create_dir(root.join("kept")).unwrap();
    let project = root.join("fixture.sublime-project");
    std::fs::write(
        &project,
        r#"{"folders":[{"path":"configured"},{"path":"kept","name":"Kept"}],"future":{"keep":true}}"#,
    )
    .unwrap();
    let mut workspace = Workspace::new(Text::new(), None, Some(project.clone()), false).unwrap();
    let mut panels = Panels::new(&workspace, false);
    panels.add_folder(&mut workspace, added.clone()).unwrap();
    workspace.request_resource_reload();
    workspace.poll();
    panels
        .remove_sidebar_folder(&mut workspace, &folder)
        .unwrap();
    workspace.wait_resources();
    assert!(!workspace.folders.iter().any(|entry| entry.path == folder));
    assert!(workspace.folders.iter().any(|entry| entry.path == added));
    let saved = vel_session::project::Project::load(&project).unwrap();
    assert_eq!(saved.folders.len(), 2);
    // The remaining entry keeps its relative spelling; no default keys are pinned.
    assert_eq!(saved.folders[0].path, std::path::Path::new("kept"));
    assert_eq!(saved.folders[0].name.as_deref(), Some("Kept"));
    assert_eq!(saved.folders[1].path, std::path::Path::new("runtime"));
    assert_eq!(saved.extra["future"]["keep"], true);
    let raw = std::fs::read_to_string(&project).unwrap();
    assert!(
        !raw.contains("follow_symlinks") && !raw.contains("exclude_patterns"),
        "{raw}"
    );
    assert!(folder.is_dir());
}
#[test]
fn phantoms_reserve_flow_and_track_edits_undo_while_annotations_do_not_add_rows() {
    let mut e = Editor::new(Text::from("alpha beta\nsecond line\n"));
    let mut view = view();
    view.apply_style(&e);
    let original = e.layout.borrow().total();
    e.run("add_phantoms",&json!({"key":"hints","items":[{"at":6,"layout":"inline","content":"<b>type</b>"},{"at":0,"layout":"below","content":"<div style='height:40px'>below</div>"},{"at":11,"layout":"block","content":"block"}]})).unwrap();
    e.run("add_regions",&json!({"key":"errors","regions":[[0,5]],"annotations":["<b>error</b>"],"annotation_color":"#ff7777"})).unwrap();
    let scene = view.scene(&e);
    assert!(
        scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .any(|t| t.text == "type")
    );
    assert!(e.layout.borrow().total() > original);
    let phantom = e.html.borrow().phantoms["hints"]
        .iter()
        .find(|p| p.layout == vel_ui::minihtml::PhantomLayout::Inline)
        .unwrap()
        .at;
    assert_eq!(phantom, 6);
    let before = view.rect_at(&e, 7);
    assert!(before.x > view.rect_at(&e, 5).x + view.cell * 2.);
    e.set_caret(0, false);
    e.run("insert", &json!({"characters":"!"})).unwrap();
    view.scene(&e);
    assert!(e.html.borrow().phantoms["hints"].iter().any(|p| p.at == 7));
    e.run("undo", &json!({})).unwrap();
    view.scene(&e);
    assert!(e.html.borrow().phantoms["hints"].iter().any(|p| p.at == 6));
    e.run("erase_phantoms", &json!({"key":"hints"})).unwrap();
    view.apply_style(&e);
    assert_eq!(e.layout.borrow().total(), original);
    let scene = view.scene(&e);
    assert!(
        scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .any(|t| t.text == "error")
    );
    assert_eq!(e.layout.borrow().total(), original);
}
#[test]
fn popup_flags_and_subl_links_run_through_actual_editor_commands() {
    let mut e = Editor::new(Text::from("hello\n"));
    let mut view = view();
    e.run("show_popup",&json!({"content":"<a href='subl:insert {\"characters\":\"!\"}' title='Insert punctuation'>insert</a>","flags":16,"max_width":240,"max_height":80})).unwrap();
    e.set_caret(2, false);
    view.scene(&e);
    let p = e.html.borrow();
    let popup = p.popup.as_ref().unwrap();
    let link = &popup.html.display.links[0];
    let point = Point {
        x: popup.html.bounds.x + f64::from(link.rect.x) + 1.,
        y: popup.html.bounds.y + f64::from(link.rect.y) + 1.,
    };
    let target = popup.html.link_at(point).unwrap().clone();
    drop(p);
    let invocation = crate::html_ui::invocation(target);
    e.run(&invocation.command, &invocation.args).unwrap();
    assert_eq!(e.text.to_string(), "he!llo\n");
    e.run("show_popup", &json!({"content":"default"})).unwrap();
    e.set_caret(0, false);
    assert!(!e.html.borrow().popup.as_ref().unwrap().visible);
    e.run("show_popup", &json!({"content":"characters","flags":48}))
        .unwrap();
    e.run("insert", &json!({"characters":"x"})).unwrap();
    assert!(!e.html.borrow().popup.as_ref().unwrap().visible);
}
#[test]
fn html_sheet_survives_hot_exit_and_remains_read_only() {
    let mut w = workspace();
    w.workbench_action(
        "new_html_sheet",
        &json!({"name":"Notes","content":"<h1>Release notes</h1><p>世界 &amp; Velocity</p>"}),
    )
    .unwrap();
    let mut view = view();
    assert!(
        view.scene(w.editor())
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .any(|r| r.text.contains("Release"))
    );
    assert_eq!(w.tabs[w.active].label(), "Notes");
    let (window, documents) = w.snapshot().unwrap();
    let mut restored = workspace();
    restored.restore_window(&window, &documents).unwrap();
    restored
        .editor_mut()
        .run("insert", &json!({"characters":"unexpected edit"}))
        .unwrap();
    assert_eq!(restored.editor().text.to_string(), "");
    assert!(restored.editor().read_only);
    assert!(
        view.scene(restored.editor())
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .any(|r| r.text.contains("Release"))
    );
}
#[test]
fn installed_user_theme_override_reload_appearance_and_corrupt_fallback() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    let path = user.join("Velocity.sublime-theme");
    std::fs::write(
        &path,
        r##"{"rules":[{"class":"status_bar","layer0.tint":"#123456"}]}"##,
    )
    .unwrap();
    let mut w = Workspace::new(Text::new(), Some(profile.path().into()), None, false).unwrap();
    let mut p = Panels::new(&w, false);
    p.layout(Size {
        width: 900.,
        height: 600.,
    });
    let mut scene = Scene::default();
    p.paint(&w, &mut scene);
    assert!(
        scene
            .layers
            .iter()
            .flat_map(|l| &l.surfaces)
            .any(|s| matches!(s,Surface::Fill(r) if r.color[0]==18./255. && r.color[1]==52./255.))
    );
    let rev = w.themes.ui.borrow().revision;
    w.set_appearance(false).unwrap();
    assert!(w.themes.ui.borrow().revision > rev);
    assert!(!w.themes.ui.borrow().palette.is_dark());
    std::fs::write(&path, "{ corrupt theme").unwrap();
    w.wait_resources();
    assert!(
        w.messages
            .iter()
            .any(|s| s.contains("theme") || s.contains("JSON") || s.contains("key"))
    );
    view().scene(w.editor());
}
#[test]
fn density_only_user_texture_is_loaded_and_painted_without_a_one_x_file() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("retina@2x.png"),
        include_bytes!("../resources/Packages/Theme - Velocity/close@2x.png"),
    )
    .unwrap();
    std::fs::write(user.join("Velocity.sublime-theme"), r##"{"rules":[{"class":"status_bar","layer0.texture":"User/retina.png","layer0.tint":"#ffffff"}]}"##).unwrap();
    let w = Workspace::new(Text::new(), Some(profile.path().into()), None, false).unwrap();
    let mut p = Panels::new(&w, false);
    p.layout(Size {
        width: 900.,
        height: 600.,
    });
    let mut scene = Scene::default();
    p.paint(&w, &mut scene);
    assert!(scene.layers.iter().flat_map(|l| &l.surfaces).any(
        |s| matches!(s, Surface::Image(q) if q.image.width as f32 == q.image.logical_size[0] * 2.)
    ));
    assert!(!w.messages.iter().any(|s| s.contains("retina")));
}

#[test]
fn cloned_views_keep_independent_widget_motion_ids() {
    let mut w = workspace();
    let first = w.editor().ui_id(0xE000);
    w.clone_file().unwrap();
    assert_ne!(first, w.editor().ui_id(0xE000));
    assert_ne!(
        w.editor().ui_id(0xE100_0000 + 256),
        w.editor().ui_id(0xE200)
    );
}
#[test]
fn inherited_user_theme_setting_controls_a_variable_paint_value() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("Vars.sublime-theme"), r##"{"extends":"Velocity.sublime-theme","variables":{"custom_tint":"#123456"},"rules":[{"class":"status_bar","settings":["emphasize"],"layer0.tint":"var(custom_tint)"}]}"##).unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        r#"{"theme":"Vars.sublime-theme","emphasize":true}"#,
    )
    .unwrap();
    let w = Workspace::new(Text::new(), Some(profile.path().into()), None, false).unwrap();
    let mut p = Panels::new(&w, false);
    p.layout(Size {
        width: 900.,
        height: 600.,
    });
    let mut scene = Scene::default();
    p.paint(&w, &mut scene);
    assert!(scene.layers.iter().flat_map(|l| &l.surfaces).any(
        |s| matches!(s, Surface::Fill(r) if r.color[0] == 18./255. && r.color[1] == 52./255.)
    ));
}
#[test]
fn output_panel_follows_window_theme_scale_and_scheme_changes() {
    let mut w = workspace();
    let mut p = Panels::new(&w, false);
    p.configure_output(&w, None, true);
    w.set_appearance(false).unwrap();
    w.editor_mut()
        .settings
        .set(Layer::User, "ui_scale", json!(1.5));
    p.panel = Some(crate::panels::Panel::Output);
    p.layout(Size {
        width: 900.,
        height: 600.,
    });
    p.paint(&w, &mut Scene::default());
    let output = p.output_editor.as_ref().unwrap();
    assert!(output.read_only);
    assert!(std::rc::Rc::ptr_eq(&output.theme, &w.themes.ui));
    assert_eq!(output.settings.number("ui_scale", 1.), 1.5);
    assert!(std::sync::Arc::ptr_eq(
        &output.language.borrow().as_ref().unwrap().scheme,
        &w.editor().language.borrow().as_ref().unwrap().scheme
    ));
}
#[test]
fn ui_scale_keeps_input_candidate_and_pointer_geometry_together() {
    let mut w = workspace();
    w.editor_mut()
        .settings
        .set(Layer::View, "ui_scale", json!(1.5));
    let mut p = Panels::new(&w, false);
    p.action(&mut w, "show_panel", &json!({"panel":"find"}), 0..10)
        .unwrap();
    p.layout(Size {
        width: 900.,
        height: 600.,
    });
    assert_eq!(p.query.scale, 1.5);
    p.query.set("a界b");
    p.query.click(p.query.bounds.x + 12. + 3. * 10.8, false);
    let caret = p.query.state().caret;
    assert!((caret.x - (p.query.bounds.x + 12. + 3. * 10.8)).abs() < 0.01);
    assert!(p.query.bounds.height >= 42.);
}
fn rect(r: Rect) -> serde_json::Value {
    json!([r.x, r.y, r.width, r.height])
}
fn snapshot(scene: &Scene) -> serde_json::Value {
    let mut snapshot = json!({"background":scene.background,"layers":scene.layers.iter().filter(|l|!l.surfaces.is_empty()||!l.rects.is_empty()||!l.text.is_empty()||!l.underlines.is_empty()||!l.contours.is_empty()).map(|l|json!({"surfaces":l.surfaces.iter().map(|s|match s{Surface::CssBox(r)=>json!({"css_box":rect(r.rect),"clip":rect(r.clip),"background":r.background,"radii":r.radii,"borders":r.borders.map(|b|json!({"width":b.width,"color":b.color}))}),Surface::Fill(r)=>json!({"fill":rect(r.rect),"clip":rect(r.clip),"color":r.color,"radius":r.radius}),Surface::Image(q)=>json!({"image":[q.image.width,q.image.height],"source":q.source,"rect":rect(q.rect),"clip":rect(q.clip),"tint":q.color})}).collect::<Vec<_>>(),"rects":l.rects.iter().map(|r|json!({"rect":rect(r.rect),"clip":rect(r.clip),"color":r.color,"radius":r.radius})).collect::<Vec<_>>(),"glyph_runs":l.text.iter().map(|r|json!({"text":r.text,"x":r.x,"baseline":r.baseline,"size":r.size,"family":r.family,"bold":r.style.bold,"italic":r.style.italic,"color":r.color,"clip":rect(r.clip)})).collect::<Vec<_>>(),"underlines":l.underlines.iter().map(|r|json!({"rect":rect(r.rect),"clip":rect(r.clip),"color":r.color})).collect::<Vec<_>>()})).collect::<Vec<_>>()});
    for (i, layer) in scene
        .layers
        .iter()
        .filter(|l| {
            !l.surfaces.is_empty()
                || !l.rects.is_empty()
                || !l.text.is_empty()
                || !l.underlines.is_empty()
                || !l.contours.is_empty()
        })
        .enumerate()
    {
        if !layer.contours.is_empty() {
            snapshot["layers"][i]["selection_contours"] = json!(layer.contours.iter().map(|c|json!({"points":c.points.iter().map(|p|[p.x+c.offset.x,p.y+c.offset.y]).collect::<Vec<_>>(),"triangles":c.triangles.as_ref(),"fill":c.fill,"border":c.border,"clip":rect(c.clip)})).collect::<Vec<_>>());
        }
        for (j, run) in layer.text.iter().enumerate() {
            if let Some(highlight) = &run.highlight {
                let ranges: Vec<_> = highlight.ranges.iter().map(|r| [r.start, r.end]).collect();
                snapshot["layers"][i]["glyph_runs"][j]["highlight"] =
                    json!({"ranges":ranges,"color":highlight.color});
            }
        }
    }
    snapshot
}

#[test]
fn key_screens_match_structural_goldens() {
    fn settled_scene(view: &mut View, editor: &Editor) -> Scene {
        // scene() stages annotation jobs. Wait for their version-bound publication,
        // rather than letting worker scheduling decide whether the diff gutter is drawn.
        view.scene(editor);
        editor.annotations.borrow_mut().flush();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let mut annotations = editor.annotations.borrow_mut();
            annotations.poll();
            if annotations.pending == 0
                && annotations
                    .exact(editor.buffer.id(), editor.buffer.change_id())
                    .is_some()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "annotation publication did not arrive"
            );
            drop(annotations);
            std::thread::yield_now();
        }
        view.scene(editor)
    }
    let mut w = workspace();
    crate::theme_bench::demo(&mut w).unwrap();
    w.editor_mut()
        .settings
        .set(Layer::View, "animation_enabled", json!(false));
    w.editor_mut()
        .settings
        .set(Layer::View, "show_minimap", json!(false));
    // Use plain-text semantics so worker timing cannot change the structural scene.
    *w.editor().language.borrow_mut() = None;
    let mut scenes = serde_json::Map::new();
    for dark in [true, false] {
        w.set_appearance(dark).unwrap();
        *w.editor().language.borrow_mut() = None;
        let appearance = if dark { "dark" } else { "light" };
        let mut view = view();
        let mut panels = Panels::new(&w, false);
        panels.side_visible = false;
        let content = panels.layout(view.size);
        view.size = Size {
            width: content.width,
            height: content.height,
        };
        // The frozen dark scene is the first paint, before scene() starts the worker.
        // The light scene includes published diffs. Model both lifecycle points explicitly.
        let scene = if dark {
            assert!(w.editor().annotations.borrow().current.is_none());
            view.scene(w.editor())
        } else {
            settled_scene(&mut view, w.editor())
        };
        scenes.insert(
            format!("{appearance}_phantoms_popup_annotations"),
            snapshot(&scene),
        );
        panels.overlay = Some(Overlay::Goto);
        panels.quick.set_items(vec![vel_ui::Item {
            caption: "render_document.rs".into(),
            icon: vel_ui::ItemIcon::File,
            annotation: "⌘P".into(),
            details: "<b>src/</b><i>render_document.rs</i>".into(),
            highlights: vec![0, 1, 2],
            ..Default::default()
        }]);
        panels.layout(Size {
            width: 900.,
            height: 600.,
        });
        let mut scene = Scene::default();
        panels.paint(&w, &mut scene);
        scenes.insert(format!("{appearance}_goto"), snapshot(&scene));
        let i = w
            .tabs
            .iter()
            .position(|t| t.editor.settings.get("velocity_html_sheet").is_some())
            .unwrap();
        let old = w.active;
        w.select(i);
        scenes.insert(
            format!("{appearance}_html_sheet"),
            snapshot(&view.scene(w.editor())),
        );
        w.select(old);
    }
    let actual = serde_json::to_string_pretty(&serde_json::Value::Object(scenes)).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/theme-scenes.json");
    if std::env::var_os("VEL_UPDATE_GOLDENS").is_some() {
        std::fs::write(&path, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    // The headless scene uses fixed grid advances and deterministic minihtml
    // metrics. The status convention and shortcut caption are native by design.
    #[cfg(windows)]
    let expected = expected
        .replace(
            "Ln 3, Col 1 · UTF-8 · Unix",
            "Ln 3, Col 1 · UTF-8 · Windows",
        )
        .replace(
            "↩ Open · ⌘↩ / ⌃↩ Open in Split · ⎋ Cancel",
            "Enter Open · Ctrl+Enter Open in Split · Esc Cancel",
        );
    let difference = actual
        .lines()
        .zip(expected.lines())
        .position(|(a, b)| a != b);
    assert!(
        actual == expected,
        "structural scene mismatch at line {:?}; inspect fixtures/theme-scenes.json",
        difference.map(|i| i + 1)
    );
}

#[test]
fn build_errors_use_html_annotations_without_phantom_rows_and_escape_markup() {
    let e = Editor::new(Text::from("fn main() {}\n"));
    let mut view = view();
    let mut scene = view.scene(&e);
    let rows = e.layout.borrow().total();
    let errors = [vel_build::Diagnostic {
        path: "example.rs".into(),
        line: 1,
        column: 1,
        message: "bad <tag> & value".into(),
        output_line: 0,
    }];
    crate::html_ui::paint_build_errors(&e, &view, errors.iter(), &mut scene);
    let text = scene
        .layers
        .iter()
        .flat_map(|l| &l.text)
        .map(|t| t.text.as_str())
        .collect::<String>();
    assert!(text.contains("bad <tag> & value"), "painted={text:?}");
    assert_eq!(e.layout.borrow().total(), rows);
    assert!(e.html.borrow().phantoms.is_empty());
    assert!(
        scene
            .layers
            .iter()
            .flat_map(|l| &l.rects)
            .any(|r| r.rect.width == 2.)
    );
}
