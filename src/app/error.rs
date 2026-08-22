//! Internal error placeholder. A real `thiserror` enum will replace this
//! alias eventually. Kept behind a module so call sites can migrate one by one.

/// Application error type.
pub type AppError = anyhow::Error;

/// Convenience alias for application results.
pub type AppResult<T> = Result<T, AppError>;
