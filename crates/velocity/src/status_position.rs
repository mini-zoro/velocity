//! Device-free selection values for the status bar.
pub(crate) fn selection_column(editor: &crate::editor::Editor, row: usize) -> usize {
    let text = &editor.text;
    let caret = editor.caret;
    match editor
        .settings
        .string("selection_description_column_type", "display")
    {
        "character" | "characters" => {
            text.byte_to_char(caret) - text.byte_to_char(text.line_to_byte(row)) + 1
        }
        "utf16" => text.byte_to_utf16(caret) - text.byte_to_utf16(text.line_to_byte(row)) + 1,
        "byte" => caret - text.line_to_byte(row) + 1,
        _ => editor.status_column.borrow_mut().column(
            text,
            (editor.buffer.id(), editor.buffer.change_id()),
            caret,
            editor.preferences.tab_size,
            crate::large::LONG_LINE,
        ),
    }
}

pub(crate) fn selection_sum(editor: &crate::editor::Editor) -> Option<String> {
    let regions = editor.regions();
    if regions.len() < 2 || editor.binary.is_some() {
        return None;
    }
    let mut integer = Some(0i128);
    let mut sum = 0.0;
    for region in regions.iter() {
        // Bound work per region in the status paint path, including nonnumeric selections.
        if region.empty() || region.end() - region.begin() > 128 {
            return None;
        }
        let source = editor.text.slice(region.to_range()).to_string();
        let number = source.trim();
        integer = integer.and_then(|total| total.checked_add(number.parse().ok()?));
        sum += number.parse::<f64>().ok()?;
        if !sum.is_finite() {
            return None;
        }
    }
    Some(integer.map_or_else(
        || {
            // Display useful precision without binary round-off tails (e.g. 0.1 + 0.2).
            format!("{sum:.14e}")
                .parse::<f64>()
                .unwrap_or(sum)
                .to_string()
        },
        |value| value.to_string(),
    ))
}
