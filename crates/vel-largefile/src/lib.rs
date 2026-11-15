//! Engine support for progressive huge documents, bounded layout and read-only hex views.
//!
//! The caller owns UI dispatch, encoding fallback and feature scheduling. See the
//! crate README for the lane-A integration contract and measured limits.
mod hex;
mod line;
mod load;
mod policy;

pub use hex::{ContentKind, HexRow, HexView, detect_content};
pub use line::{LongLineIndex, WordClass};
pub use load::{
    Cancellation, LoadError, LoadOptions, LoadSnapshot, LoadTask, Loaded, Progress, load_utf8,
    stream_utf8,
};
pub use policy::{DocumentMetrics, FeaturePolicy, HighlightMode, LargeDocumentPolicy};
