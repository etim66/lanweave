//! Internal error placeholder. A real `thiserror` enum will replace this
//! alias eventually. Kept behind a module so call sites can migrate one by one.

pub type AppError = anyhow::Error;

pub type AppResult<T> = Result<T, AppError>;
