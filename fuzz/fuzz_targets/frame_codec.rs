//! Fuzzes the incremental frame decoder with arbitrary socket bytes.
//!
//! The decoder must never panic and never allocate for invalid or oversized
//! headers; it only ever returns frames, `None`, or a `FrameError`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut buffer = bytes::BytesMut::from(data);
    while let Ok(Some(_frame)) = lanweave::fuzzing::decode(&mut buffer) {}
});
