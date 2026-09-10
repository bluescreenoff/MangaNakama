//! `doc`'s own test suites, one file per suite (moved out of `doc.rs`).

/// IO-043 — import with a selection active builds the layer mask.
mod import_mask_tests;
mod mask_tests;
mod tests;
mod combine_tests;
mod group_tests;
/// PR-041 — the operation counter the "save recovery data for every
/// operation" preference fires on.
mod op_count_tests;
