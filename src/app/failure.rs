//! Safe application-level failure categories.

/// Safe error categories suitable for application state and user-facing views.
///
/// Detailed errors remain local to the failing adapter so paths, peer input, and
/// other sensitive diagnostics do not accidentally reach the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum FailureKind {
    Startup,
    Connection,
    Pairing,
    Session,
    Transfer,
    Internal,
}
