//! Safe filesystem boundary for received files.
//!
//! Handles destination name validation, restrictive no-follow temporary file
//! creation, and no-replace finalization. Lanweave never overwrites or
//! silently renames a destination file.
