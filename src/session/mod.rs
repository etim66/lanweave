//! Authorized session ownership: pairing state, transfer policy, idle timer.
//!
//! One task owns each connection and its mutable session state. After the
//! `session_idle` state is reached, the reusable transfer loop runs here.
