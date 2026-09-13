# Implementation Roadmap

This roadmap lists the implementation sequence and dependency gates. Completed work should be reflected here and in the implemented-architecture section of [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Gate 1: App And TUI Shell

Create the Rust binary, async event loop, terminal setup and cleanup, application model, command registry, `/` command list, and basic screens.

Exit criteria:

- `lanweave` opens and exits without leaving the terminal broken.
- Keyboard, paste, resize, tick, and shutdown events use bounded channels.
- `/` shows commands valid for the current state.
- App state and protocol policy do not depend on terminal rendering.

## Gate 2: Discovery And Device Selection

Start the listener, mDNS browse, and advertisement with the app. Show live devices and allow selection.

Exit criteria:

- A running app advertises and accepts requests; cached stale records after a crash expire and cannot connect.
- Stale and goodbye records disappear from the TUI.
- IPv4, IPv6, multiple interfaces, duplicate records, and blocked multicast are tested.
- Direct address remains available as a fallback.

## Gate 3: Framing And Transport

Implement strict JSON controls, binary frames, incremental parsing, bounded queues, TCP, and provisional TLS 1.3.

Exit criteria:

- Golden and malformed-input tests cover every message and frame boundary. ✅
- Partial and combined socket reads behave identically. ✅
- TLS uses a fresh connection identity and disables resumption and early data. ✅
- One writer prevents frame interleaving and unbounded queues. ✅

Status: implemented. The framed connection, the single bounded ordered writer, and the provisional TLS 1.3 profile (ALPN `lanweave/1`, fresh P-256 responder certificate, disabled resumption and early data, 32-byte exporter) live in the crate-internal `transport` module; the session owner wires them into the application in Gate 4.

## Gate 4: Pairing Request And Authorization

Implement request prompts, accept/reject, local code display and entry, the pairing adapter, and mutual confirmation.

Exit criteria:

- A code is generated only after acceptance and never sent or logged.
- Expiry, one-attempt use, concurrency, and one-use state are tested.
- Pairing is bound to the current TLS connection.
- Independent audit and specialist review clear the release gate.

Status: implemented as a prototype. The session owner completes TLS, `hello`,
`pair_request`/`pair_response`, the one-time code, and the four SPAKE2 records
bound to the TLS exporter and the exact hello bodies. A session reaches
authorized idle only after both confirmations verify, and a rejected, timed
out, or failed pairing closes the provisional connection with no code state.
The pairing dependency, password mapping, and composition review remain
release-blocking; see [`CRYPTOGRAPHY.md`](CRYPTOGRAPHY.md).

## Gate 5: Local And Network File Transfer

Implement pasted path parsing, manifest review, destination safety, sequential streaming, verification, progress, and cleanup.

Exit criteria:

- Multi-file transfers work without loading whole files into memory.
- The recipient approves the complete metadata list before data.
- Unsafe or existing names never overwrite a destination.
- A later-file failure keeps the verified prefix and removes the current partial.

## Gate 6: Reusable Sessions

Allow either participant to propose later transfers, serialize transfer state, resolve simultaneous requests, and close sessions.

Exit criteria:

- Second and reverse-direction transfers pass integration tests.
- Rejection before `ready` keeps the session usable; active-transfer failure cleans up and closes it.
- Manual close and 600-second idle close work from both peers.
- A new connection always requires fresh pairing.

## Gate 7: Hardening And Release

Add fault injection, fuzzing, cross-platform tests, packaging, accessibility review, and complete project policy files.

Exit criteria:

- Parser, filesystem, session, and security-profile suites pass.
- `cargo audit` and `cargo deny` pass under a written policy.
- Licence, contribution, and security-reporting documents exist.
- Security claims match the completed audit evidence.
