//! Error types for the interceptor.
//!
//! We use [`anyhow`] for the application-level flow (rich context, easy `?`) and
//! a small typed [`AppError`] for the few conditions the caller may want to
//! branch on. Keeping the typed set tiny is deliberate: most failures here are
//! environmental (no adapter, daemon down, permissions) and read best as
//! contextual anyhow chains.

use thiserror::Error;

/// Domain errors that callers might reasonably want to match on.
#[derive(Debug, Error)]
pub enum AppError {
    /// The requested Bluetooth adapter (e.g. `hci0`) was not present.
    #[error("bluetooth adapter `{0}` not found (is it plugged in and is bluetoothd running?)")]
    AdapterNotFound(String),

    /// No BLE peripheral was selected/found to target.
    #[error("no target device selected")]
    NoTarget,

    /// The user aborted an interactive selection.
    #[error("selection cancelled")]
    Cancelled,
}
