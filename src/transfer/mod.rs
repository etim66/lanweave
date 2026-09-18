//! Local sequential file transfer engine.
//!
//! Implements the in-memory multi-file read/write/hash/verify pipeline with
//! fail-fast semantics, and bridges the engine to bounded DATA frames over
//! the authorized TLS session.
//!
//! Path review and the wire manifest live in [`selection`]; the byte pipeline
//! lives in [`engine`]. Both are local-only until the session owner connects
//! them to the network.

pub(crate) mod engine;
pub(crate) mod selection;
