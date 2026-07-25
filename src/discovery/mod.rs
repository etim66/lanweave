//! Live device discovery over mDNS/DNS-SD.
//!
//! Discovery answers only "where might a Lanweave listener be?". It does not
//! prove identity or authorize a connection. The adapter trait and candidate
//! store live here, and the local listener is bound before advertising.
