//! Sublime syntax loading, interpretation and official scope-test evaluation.
//! Lines include their terminating newline; operation offsets are UTF-8 bytes.
mod cache;
pub mod indent;
pub mod metadata;
mod parser;
mod resources;
mod state;
pub mod syntax_definition;
mod syntax_set;
pub mod syntest;
mod tm_language;
mod yaml_load;
pub use cache::CacheStatus;
pub use metadata::{
    Metadata, MetadataError, MetadataSet, ScopedSpan, Symbol, SymbolMetadata, SymbolTransformation,
};
pub use parser::{ParsingError, ScopeOperations};
pub use state::{LineOps, ParseState, Rewind, SharedLineOps, SharedScopeOperations};
pub use syntax_definition::SyntaxDefinition;
pub use syntax_set::{SyntaxReference, SyntaxSet, SyntaxSetBuilder};
pub use tm_language::TmLanguageError;
#[cfg(feature = "matcher-profile")]
pub use vel_regex::profile as matcher_profile;
pub use vel_scope::{Scope, ScopeOp, ScopeStack, ScopeStackOp};
pub use yaml_load::ParseSyntaxError;

/// Failure while reading a package tree.
#[derive(Debug, thiserror::Error)]
pub enum LoadingError {
    /// Filesystem error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A definition contains invalid YAML or structure.
    #[error("{1}: {0}")]
    ParseSyntax(ParseSyntaxError, String),
    /// Package directory traversal failed.
    #[error("{0}")]
    WalkDir(Box<walkdir::Error>),
    /// Invalid or unsupported TextMate grammar structure.
    #[error(transparent)]
    TmLanguage(#[from] TmLanguageError),
    /// Virtual resource paths must be package-relative and have a supported extension.
    #[error("invalid package resource path: {0}")]
    InvalidPath(String),
}

mod dumps {
    use serde::{Serialize, de::DeserializeOwned};
    use std::io::Read;
    pub fn dump_binary<T: Serialize>(value: &T) -> Vec<u8> {
        // Only context bodies are compressed. Index metadata remains immediately available.
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        bincode::serialize_into(&mut encoder, value)
            .expect("serializing parsed syntax definitions");
        encoder
            .finish()
            .expect("compressing syntax contexts into memory")
    }
    pub fn from_reader<T: DeserializeOwned, R: Read>(reader: R) -> Result<T, bincode::Error> {
        let mut bytes = Vec::new();
        flate2::read::ZlibDecoder::new(reader)
            .read_to_end(&mut bytes)
            .map_err(|e| Box::new(bincode::ErrorKind::Io(e)))?;
        bincode::deserialize(&bytes)
    }
}
