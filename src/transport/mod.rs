//! TCP/TLS transport with one ordered writer and explicit cancellation.
//!
//! Owns the framed TCP connection and bounded writer, and layers the
//! provisional TLS 1.3 profile with ALPN `lanweave/1`, fresh per-connection
//! certificates, and disabled resumption and early data.
