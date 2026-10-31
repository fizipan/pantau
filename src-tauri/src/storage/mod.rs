//! Storage items Pantau knows about, and how to measure them. Nothing here deletes.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod clean;
pub mod measure;
pub mod recent;
pub mod rules;
pub mod scheduler;

#[cfg(test)]
pub use measure::measure;
pub use measure::{Measured, denied_roots, find_named_dirs, measure_while};
pub use rules::storage_items;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Safe,
    Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Cleaner {
    RemoveDir,
    PnpmPrune,
    OldTemp,
    OldClaudeVersions,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemState {
    Pending,
    Ready,
    Denied,
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageItem {
    pub id: String,
    pub title: String,
    pub sub: String,
    pub path: PathBuf,
    pub category: Category,
    pub cleaner: Cleaner,
    /// The folder size is a ceiling: what a cleanup frees can be smaller.
    pub upper_bound: bool,
    pub bytes: Option<u64>,
    pub measured_at_ms: Option<u64>,
    pub state: ItemState,
}

#[derive(Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub tmpdir: PathBuf,
    pub private_tmp: PathBuf,
    pub scan_folders: Vec<PathBuf>,
}
