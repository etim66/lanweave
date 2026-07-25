//! Incremental bounded frame codec for JSON controls and binary DATA frames.
//!
//! Implements the fixed 12-byte header (see `docs/MESSAGE_FORMAT.md`) and
//! handles partial and coalesced socket reads without unbounded allocation.
