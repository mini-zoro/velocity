//! Virtual grammars shared by the build-time index and the source loader.
pub(crate) const PLAIN_TEXT_PATH: &str = "Packages/Default/Plain Text.sublime-syntax";
pub(crate) const PLAIN_TEXT_SYNTAX: &str =
    "name: Plain Text\nfile_extensions: [txt]\nscope: text.plain\ncontexts: { main: [] }\n";
pub(crate) const FIND_RESULTS_PATH: &str = "Packages/Default/Find Results.sublime-syntax";
pub(crate) const INTERNAL_SYNTAXES: [(&str, &str); 2] = [
    (PLAIN_TEXT_PATH, PLAIN_TEXT_SYNTAX),
    (
        FIND_RESULTS_PATH,
        include_str!("../resources/Packages/Default/Find Results.sublime-syntax"),
    ),
];
