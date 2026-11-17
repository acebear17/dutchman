//! Shared stream and result budgets.
pub const CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024 * 1024;
/// The host takes a loss report under 1 MiB.
pub const MAX_REPORT_BYTES: usize = 1024 * 1024 - 1;
/// Bytes read in one conversion, across the file and its parts: the reader's own cap.
pub const MAX_INPUT_BYTES: u64 = 1 << 30;
