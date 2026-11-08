//! Núcleo do AtlasDisco: analisador de espaço em disco multiplataforma.

pub mod analysis;
pub mod export;
pub mod fmt;
pub mod model;
pub mod ops;
pub mod pattern;
pub mod platform;
pub mod remote;
pub mod scan;
pub mod search;
pub mod snapshot;

pub use model::{Node, Scan};
pub use scan::{scan, Progress, ScanOptions};
