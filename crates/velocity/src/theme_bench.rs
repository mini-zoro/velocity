//! Original review fixture and CPU measurements for theme/minihtml integration.
use crate::workspace::Workspace;
use vel_config::json;
pub const HOVER: &str = include_str!("../fixtures/theme-hover.html");
pub fn demo(workspace: &mut Workspace) -> Result<(), String> {
    workspace.new_file()?;
    workspace.set_file_type("Rust")?;
    let editor = workspace.editor_mut();
    editor.run("insert",&json!({"characters":"// Theme and minihtml review\nfn render_document() {\n    let greeting = \"Hello, 世界 👋\";\n    println!(\"{greeting}\");\n}\n"}))?;
    editor.set_caret(editor.text.line_to_byte(2), false);
    editor.run("add_phantoms",&json!({"key":"demo","items":[{"at":editor.text.line_to_byte(1)+3,"layout":"inline","content":"<span style='background-color:var(--bluish);color:var(--background);padding:2px 5px;border-radius:3px'>inline</span>"},{"at":editor.text.line_to_byte(2),"layout":"below","content":"<div style='padding:4px;border-left:2px solid var(--cyanish)'>Below: <code>greeting: String</code></div>"},{"at":editor.text.line_to_byte(4),"layout":"block","content":"<div style='padding:8px;border:1px solid var(--bluish);border-radius:6px'>Block phantom · <a href='subl:hide_popup'>hide popup</a></div>"}]}))?;
    editor.run("add_annotations",&json!({"key":"demo","annotation_color":"var(--redish)","items":[{"at":editor.text.line_to_byte(3)-1,"content":"<small><b>Review annotation</b> · <a href='subl:hide_popup'>dismiss popup</a></small>"}]}))?;
    editor.run(
        "show_popup",
        &json!({"content":HOVER,"flags":18,"max_width":460,"max_height":220}),
    )?;
    workspace.workbench_action("new_html_sheet",&json!({"name":"Theme & minihtml","content":format!("<h1>Velocity</h1><p>Theme and minihtml review</p><p><a href='subl:set_theme_appearance {{\"dark\":false}}'>Light</a> · <a href='subl:set_theme_appearance {{\"dark\":true}}'>Dark</a> · <a href='subl:theme_demo'>New demo</a></p>{HOVER}")}))?;
    let previous = workspace.active.saturating_sub(1);
    workspace.select(previous);
    Ok(())
}

pub fn native_step(workspace: &mut Workspace, step: usize) -> Result<(), String> {
    match step % 10 {
        0 => workspace.set_appearance((step / 10).is_multiple_of(2))?,
        1 => {
            workspace.editor_mut().run(
                "show_popup",
                &json!({"content":HOVER,"flags":18,"max_width":460,"max_height":220}),
            )?;
            workspace.editor_mut().run("auto_complete", &json!({}))?;
        }
        2 => {
            if let Some(p) = &mut workspace.editor().html.borrow_mut().popup {
                p.html.scroll_by(0., 40.);
            }
        }
        3 => {
            if let Some(i) = workspace
                .tabs
                .iter()
                .position(|t| t.editor.settings.get("velocity_html_sheet").is_some())
            {
                workspace.select(i);
            }
        }
        4 => {
            let i = workspace
                .tabs
                .iter()
                .position(|t| {
                    t.editor.settings.get("velocity_html_sheet").is_none()
                        && !t.editor.html.borrow().phantoms.is_empty()
                })
                .unwrap_or(0);
            workspace.select(i);
        }
        5 => workspace.editor_mut().run("hide_popup", &json!({}))?,
        _ => {}
    }
    Ok(())
}

pub fn run(
    reporter: &crate::report::Reporter,
    options: &crate::cli::Options,
) -> Result<(), String> {
    use crate::{report::ms, stats::summarize, view::View};
    use std::{hint::black_box, time::Instant};
    use vel_platform::Size;
    let mut workspace =
        Workspace::new(vel_text::Text::new(), options.data_dir.clone(), None, false)?;
    demo(&mut workspace)?;
    let mut panels = crate::panels::Panels::new(&workspace, false);
    let full = Size {
        width: options.width as f64,
        height: options.height as f64,
    };
    let content = panels.layout(full);
    let mut view = View::new(
        Size {
            width: content.width,
            height: content.height,
        },
        7.8,
        12.,
    );
    view.apply_style(workspace.editor());
    let mut popup = Vec::new();
    let mut annotations = Vec::new();
    let mut switches = Vec::new();
    for _ in 0..100 {
        workspace.editor_mut().run("hide_popup", &json!({}))?;
        let at = Instant::now();
        workspace.editor_mut().run(
            "show_popup",
            &json!({"content":HOVER,"flags":18,"max_width":460,"max_height":220}),
        )?;
        black_box(view.scene(workspace.editor()));
        popup.push(ms(at.elapsed()));
    }
    workspace.editor_mut().run("hide_popup", &json!({}))?;
    workspace
        .editor_mut()
        .run("erase_phantoms", &json!({"key":"demo"}))?;
    workspace
        .editor_mut()
        .run("erase_annotations", &json!({"key":"demo"}))?;
    let editor = workspace.editor_mut();
    editor.run("select_all", &json!({}))?;
    editor.run("insert",&json!({"characters":(0..200).map(|i|format!("let item_{i} = {i};\n")).collect::<String>()}))?;
    let items=(0..200).map(|i|json!({"at":editor.text.line_to_byte(i+1)-1,"content":format!("<small><b>E{i:03}</b> diagnostic <a href='subl:hide_popup'>details</a></small>")})).collect::<Vec<_>>();
    editor.run(
        "add_annotations",
        &json!({"key":"benchmark","annotation_color":"var(--redish)","items":items}),
    )?;
    view.size = Size {
        width: 1200.,
        height: 4100.,
    };
    view.apply_style(workspace.editor());
    black_box(view.scene(workspace.editor()));
    for _ in 0..100 {
        let at = Instant::now();
        black_box(view.scene(workspace.editor()));
        annotations.push(ms(at.elapsed()));
    }
    view.size = Size {
        width: content.width,
        height: content.height,
    };
    for i in 0..50 {
        let at = Instant::now();
        workspace.set_appearance(i % 2 == 0)?;
        let mut scene = view.scene(workspace.editor());
        crate::app_scene_translate(&mut scene, content.x, content.y);
        panels.paint(&workspace, &mut scene);
        black_box(scene);
        switches.push(ms(at.elapsed()));
    }
    reporter.emit(json!({"kind":"theme_ui_cpu","popup_bytes":HOVER.len(),"popup_first_frame_ms":summarize(&popup),"annotations":200,"annotation_frame_ms":summarize(&annotations),"theme_next_frame_ms":summarize(&switches),"popup_samples_ms":popup,"annotation_samples_ms":annotations,"theme_switch_samples_ms":switches,"metrics":"deterministic headless; native shaping/upload/scanout excluded","gui_started":false,"gpu_started":false,"viewport":[0,0,1200,4100]}))?;
    if summarize(&popup).unwrap().p95 > 2.
        || summarize(&annotations).unwrap().p95 > 2.
        || summarize(&switches).unwrap().p95 > 50.
    {
        return Err("theme UI CPU budget exceeded".into());
    }
    Ok(())
}
