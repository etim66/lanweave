//! Pairing adapter wrapping an RFC 9382 SPAKE2-P256-SHA256-HKDF-HMAC library.
//!
//! Lanweave does not implement elliptic-curve group arithmetic itself. The
//! prototype adapter sits behind a strict audit gate and is wired into the
//! mutual confirmation flow bound to the current TLS exporter and exact
//! hello bodies.
