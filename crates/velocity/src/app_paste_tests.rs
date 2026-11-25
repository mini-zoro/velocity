//! Large pastes stay readable: `word_wrap: auto` wraps a line wider than the viewport.
use super::*;
use crate::session::Sessions;
use std::{cell::RefCell, rc::Rc};
use vel_text::Text;

fn app(workspace: Workspace) -> App {
    let options = Options::default();
    let now = Instant::now();
    App::new(
        options.clone(),
        Clock::new(now, std::time::SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        Rc::new(RefCell::new(Sessions::new(None))),
    )
}

fn insert(app: &mut App, text: &str) {
    app.handle_text_input(
        None,
        TextInput::Insert {
            text: text.into(),
            replacement: None,
        },
    );
}

#[test]
fn a_pasted_single_huge_line_wraps_instead_of_blanking_the_viewport() {
    let mut app = app(Workspace::new(Text::new(), None, None, false).unwrap());
    // About 97k columns on one line: longer than any viewport, and longer than the
    // large-document long-line threshold.
    let line = "网关".repeat(48_500);
    insert(&mut app, &line);
    app.workspace.editor_mut().sync();
    assert!(
        app.workspace.editor().features.word_wrap,
        "auto wrap stays on for a lone long line"
    );
    let scene = app.scene();
    assert_eq!(
        app.view.scroll_x, 0.,
        "a wrapped document does not scroll sideways"
    );
    let painted: String = scene
        .layers
        .iter()
        .flat_map(|layer| &layer.text)
        .map(|run| run.text.as_str())
        .collect();
    assert!(painted.contains("网关"), "the pasted text is painted");
    assert_eq!(
        app.workspace.editor().caret,
        app.workspace.editor().text.len_bytes(),
        "the caret follows the paste"
    );
}
