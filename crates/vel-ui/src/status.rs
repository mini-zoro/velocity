//! Status text formatting, independent of document and native window services.
/// Describes selected Unicode scalar characters, omitting a single empty caret.
pub fn selection(regions: usize, characters: usize) -> String {
    let chars = format!(
        "{characters} character{} selected",
        if characters == 1 { "" } else { "s" }
    );
    match (regions, characters) {
        (0 | 1, 0) => String::new(),
        (1, _) => chars,
        (_, 0) => format!("{regions} selection regions"),
        _ => format!("{regions} selection regions · {chars}"),
    }
}
/// Formats the clickable indentation item.
pub fn indentation(spaces: bool, width: usize) -> String {
    format!("{}: {width}", if spaces { "Spaces" } else { "Tab Size" })
}
/// Completed Find in Files feedback, counting occurrences and files with matches.
pub fn search(matches: usize, files: usize) -> String {
    format!(
        "{matches} {} in {files} {}",
        if matches == 1 { "match" } else { "matches" },
        if files == 1 { "file" } else { "files" }
    )
}
/// Position, optional selection and optional format fields in their displayed order.
pub fn position(
    line: usize,
    column: usize,
    regions: usize,
    characters: usize,
    encoding: Option<&str>,
    endings: Option<&str>,
) -> String {
    let mut fields = vec![format!("Ln {line}, Col {column}")];
    let selected = selection(regions, characters);
    if !selected.is_empty() {
        fields.push(selected);
    }
    fields.extend(encoding.into_iter().chain(endings).map(str::to_owned));
    fields.join(" · ")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_and_format_status_table() {
        for (regions, characters, expected) in [
            (1, 0, ""),
            (1, 1, "1 character selected"),
            (1, 2, "2 characters selected"),
            (2, 0, "2 selection regions"),
            (2, 1, "2 selection regions · 1 character selected"),
            (3, 8, "3 selection regions · 8 characters selected"),
        ] {
            assert_eq!(selection(regions, characters), expected);
        }
        assert_eq!(position(2, 3, 1, 0, None, None), "Ln 2, Col 3");
        assert_eq!(
            position(2, 3, 1, 1, Some("UTF-8"), Some("Unix")),
            "Ln 2, Col 3 · 1 character selected · UTF-8 · Unix"
        );
        assert_eq!(indentation(true, 4), "Spaces: 4");
        assert_eq!(indentation(false, 2), "Tab Size: 2");
        assert_eq!(search(1, 1), "1 match in 1 file");
        assert_eq!(search(2, 1), "2 matches in 1 file");
        assert_eq!(search(0, 0), "0 matches in 0 files");
    }
}
