#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
mod region;
pub use region::{Region, Selection};

mod buffer;
mod change;
pub use buffer::{
    Buffer, BufferId, EditKind, HistoryLimits, RegionSet, Settings, ViewId, ViewState,
};
pub use change::{Bias, Change, ChangeId, ChangeSet};
pub use vel_text::Edit;
mod layout;
pub mod ops;
pub use layout::{MonospaceLayout, VerticalLayout};
