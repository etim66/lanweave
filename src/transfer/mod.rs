//! Local sequential file transfer engine.
//!
//! Implements the in-memory multi-file read/write/hash/verify pipeline with
//! fail-fast semantics, and bridges the engine to bounded DATA frames over
//! the authorized TLS session.
