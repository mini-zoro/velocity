//! Automatic CPU-only evidence for the view specification, without AppKit/Metal.
use crate::{
    cli::Options,
    editor::Editor,
    language::Assets,
    layout::{Layout, Options as WrapOptions},
    report::{Reporter, ms},
    stats::summarize,
    view::View,
    workspace::Workspace,
};
use std::{
    hint::black_box,
    path::Path,
    time::{Duration, Instant},
};
use vel_buffer::{Buffer, Change, ChangeSet, Edit, EditKind, Region, Selection};
use vel_config::{json, settings::Layer};
use vel_platform::Size;
use vel_text::Text;

fn wait(editor: &mut Editor, mut condition: impl FnMut(&Editor) -> bool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !condition(editor) {
        editor.poll_language();
        if let Some(error) = editor
            .language
            .borrow_mut()
            .as_mut()
            .and_then(|l| l.errors.pop())
        {
            return Err(error);
        }
        if Instant::now() > deadline {
            return Err("language benchmark timed out".into());
        }
        std::thread::park_timeout(Duration::from_micros(250));
    }
    Ok(())
}
fn view() -> View {
    View::new(
        Size {
            width: 3840.,
            height: 2160.,
        },
        7.8,
        12.,
    )
}
pub fn run(reporter: &Reporter, options: &Options) -> Result<(), String> {
    let assets_start = Instant::now();
    let assets = Assets::bundled()?;
    reporter.emit(json!({"kind":"view_features_environment", "gui_started":false,"gpu_started":false,"logical_size":[3840,2160],"syntax_count":assets.syntaxes.syntaxes().len(),"assets_ms":ms(assets_start.elapsed()),"boundary":"CPU layout/scenes only; excludes native event receipt, fonts, uploads, GPU and presentation"}))?;
    let fixture = options.file.clone().unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/data/rust-10mb.rs")
    });
    let source = std::fs::read_to_string(&fixture).map_err(|e| e.to_string())?;
    let mut workspace = Workspace::new(Text::from(source.as_str()), None, None, false)?;
    workspace.set_file_type("Rust")?;
    let editor = workspace.editor_mut();
    let mut v = view();
    black_box(v.scene(editor));
    let default_scene = Instant::now();
    wait(editor, |e| {
        black_box(v.scene(e));
        e.language.borrow().as_ref().is_some_and(|l| {
            (0..v.rows().min(e.text.len_lines()))
                .all(|row| l.styled.get(&row).is_some_and(|s| s.exact))
        })
    })?;
    reporter.emit(json!({"kind":"first_highlighted_screen_cpu","fixture":fixture,"bytes":source.len(),"after_default_scene_ms":ms(default_scene.elapsed()),"budget_ms":150,"native_present_pending":true}))?;
    let background = Instant::now();
    wait(editor, |e| {
        e.language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.ready() && l.highlight_complete())
    })?;
    reporter.emit(json!({"kind":"analysis_ready","after_first_highlight_ms":ms(background.elapsed()),"symbols":editor.language.borrow().as_ref().and_then(|l|l.analysis.as_ref()).map(|a|a.symbols.len())}))?;
    let mut scenes = Vec::new();
    let mut minimap_rects = 0;
    for i in 0..600 {
        v.scroll_y = ((i * 97) % editor.text.len_lines()) as f64 * v.line_height;
        v.clamp_scroll(editor);
        wait(editor, |e| {
            black_box(v.scene(e));
            v.exact(e)
        })?;
        let at = Instant::now();
        let scene = v.scene(editor);
        scenes.push(ms(at.elapsed()));
        minimap_rects = minimap_rects.max(
            scene
                .layers
                .iter()
                .find(|l| l.minimap)
                .map_or(0, |l| l.rects.len()),
        );
        black_box(scene);
    }
    reporter.emit(json!({"kind":"highlight_minimap_scroll_cpu","samples_ms":scenes,"stats_ms":summarize(&scenes),"p95_budget_ms":2,"exact_viewports":true,"boundary":"CPU scene with exact viewport styles; asynchronous delivery happens before the timer","minimap_rects_max":minimap_rects,"gpu_budget_ms":1,"gpu_pending":true}))?;
    // A fully parsed 10k-line Rust document, 500 real keymap/command transactions.
    let rust = "fn function() { let value = 10; } // 你好\n".repeat(10_000);
    let mut typing = Editor::new(Text::from(rust.as_str()));
    typing.attach_language(
        assets.clone(),
        "Rust",
        std::sync::Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
    )?;
    typing.settings.set(Layer::View, "word_wrap", json!(false));
    typing.sync();
    let byte = typing
        .text
        .line_to_byte(5000.min(typing.text.len_lines() - 1));
    typing.set_caret(byte, false);
    let mut typing_view = view();
    typing_view.reveal(&typing);
    wait(&mut typing, |e| {
        e.language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.ready() && l.highlight_complete())
    })?;
    black_box(typing_view.scene(&typing));
    let mut samples = Vec::new();
    let mut edits = Vec::new();
    let mut paints = Vec::new();
    for _ in 0..500 {
        wait(&mut typing, |e| {
            black_box(typing_view.scene(e));
            typing_view.exact(e)
                && e.language
                    .borrow()
                    .as_ref()
                    .is_some_and(|l| l.highlight_complete())
        })?;
        let at = Instant::now();
        crate::type_through_keymap(&mut typing, "x")?;
        edits.push(ms(at.elapsed()));
        let paint = Instant::now();
        black_box(typing_view.scene(&typing));
        paints.push(ms(paint.elapsed()));
        samples.push(ms(at.elapsed()));
    }
    reporter.emit(json!({"kind":"highlighted_typing_cpu","samples_ms":samples,"edit_stats_ms":summarize(&edits),"scene_stats_ms":summarize(&paints),"stats_ms":summarize(&samples),"p50_budget_ms":2,"p95_budget_ms":4,"carets":1,"before_key_exact":true,"after_key_may_fallback":true,"native_present_pending":true}))?;
    drop(workspace);
    drop(typing);
    let source =
        "    long words 你好 👩‍💻 wrap across the viewport and continue here\n".repeat(100_000);
    let mut buffer = Buffer::new(source.as_str());
    let id = buffer.new_view();
    let mut layout = Layout::default();
    let options = WrapOptions {
        width: 32,
        tab: 4,
        indent: true,
    };
    layout.configure(
        buffer.text(),
        (buffer.id(), buffer.change_id()),
        options,
        &[],
    );
    let mut wrap = Vec::new();
    for _ in 0..100 {
        let old = buffer.text().clone();
        let byte = old.line_to_byte(50_000) + 10;
        buffer.transact(
            id,
            EditKind::Other,
            &[Edit {
                range: byte..byte + 1,
                text: "x",
            }],
            Selection::from_regions([Region::caret(byte + 1)]),
        );
        let changes = ChangeSet::new(vec![Change {
            old_range: byte..byte + 1,
            new_len: 1,
        }]);
        let at = Instant::now();
        layout.change(
            &old,
            buffer.text(),
            (buffer.id(), buffer.change_id()),
            &changes,
        );
        wrap.push(ms(at.elapsed()));
        if layout.rewrapped != 1 {
            return Err(format!(
                "one-character edit rewrapped {} lines",
                layout.rewrapped
            ));
        }
    }
    reporter.emit(json!({"kind":"rewrap_100k_cpu","samples_ms":wrap,"stats_ms":summarize(&wrap),"budget_ms":1,"rewrapped_lines_per_edit":1,"boundary":"layout invalidation and rewrap; transaction applied before timer"}))?;
    // Exercise actual named-region commands, not synthetic height toggles.
    let fold_source = "fn f() {\n    work();\n}\n".repeat(33_333);
    let mut folds = Editor::new(Text::from(fold_source.as_str()));
    folds.attach_language(
        assets,
        "Rust",
        std::sync::Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
    )?;
    folds.settings.set(Layer::View, "word_wrap", json!(false));
    folds.sync();
    wait(&mut folds, |e| {
        e.language.borrow().as_ref().is_some_and(|l| l.ready())
    })?;
    let mut fv = view();
    fv.apply_style(&folds);
    let mut fold_samples = Vec::new();
    let mut unfold_samples = Vec::new();
    for _ in 0..20 {
        let at = Instant::now();
        folds.run("fold_by_level", &json!({"level":1}))?;
        fv.apply_style(&folds);
        fold_samples.push(ms(at.elapsed()));
        let at = Instant::now();
        folds.run("unfold_all", &json!({}))?;
        fv.apply_style(&folds);
        unfold_samples.push(ms(at.elapsed()));
    }
    reporter.emit(json!({"kind":"fold_unfold_100k_cpu","fold_samples_ms":fold_samples,"fold_stats_ms":summarize(&fold_samples),"unfold_samples_ms":unfold_samples,"unfold_stats_ms":summarize(&unfold_samples),"budget_ms":20,"boundary":"command, named-region tracking, wrap/fold projection rebuild; excludes painting"}))?;
    reporter
        .emit(json!({"kind":"complete","complete":true,"gui_started":false,"gpu_started":false}))
}
