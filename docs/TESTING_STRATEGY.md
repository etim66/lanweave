# Testing Strategy

Tests show that the implementation follows the draft. They do not prove that unaudited cryptography is secure.

## TUI Tests

- `lanweave` opens, redraws, resizes, and restores the terminal on exit, error, and panic.
- Entering `/` shows the commands valid in each app state.
- Device selection, pairing prompts, code display and entry, file and folder paste, metadata review, per-file progress, completion summaries, rejection, cancellation, and close screens are keyboard accessible.
- Long device, command, file-review, transfer, and summary lists scroll with the arrow keys, PageUp/PageDown, and Home/End; the active transfer follows the file in flight until a manual scroll pauses it.
- Pasted paths with spaces, quotes, Unicode, newlines, `file://` URIs, escaped spaces, and terminal escape bytes are handled safely.
- Peer display names and filenames cannot inject terminal controls.

## Discovery Tests

- Devices appear after app startup and disappear after app exit, goodbye, or expiry.
- Duplicate, stale, spoofed, conflicting, IPv4, IPv6, and multi-interface records are handled within bounds.
- Discovery loss does not end an active session.
- Direct address works when multicast is blocked.

## Message And Frame Tests

- Golden encode/decode vectors cover every control message and both frame kinds.
- Duplicate fields, unknown fields, invalid UTF-8, invalid numbers, excess nesting, and oversized values fail consistently.
- Every header and body split point produces the same decoded frames as contiguous input.
- Several frames in one read stay ordered.
- Claimed lengths are checked before allocation.
- Binary `DATA` never reaches JSON parsing.

## Pairing Tests

- The responder creates no code before accepting `pair_request`.
- Rejection and prompt timeout close without code state.
- Code generation is uniform across all eight-digit values and keeps leading zeroes.
- The 120-second monotonic boundary and one-attempt rule are exact under concurrency.
- The code, derived secrets, exporter, and confirmation values never appear in logs or artifacts.
- Correct code succeeds only on the TLS connection used for pairing.
- Wrong code, replay, changed hello bytes, changed exporter, invalid points, reflection, and reordered records fail.
- Independent RFC and application-profile vectors pass against an approved implementation.

## Session Tests

- Pairing success enters idle without approving files.
- A first, second, and later transfer can run on one connection.
- Either participant can request the next transfer.
- A transfer rejection returns both peers to idle.
- Rejection or cancellation before `ready` returns both peers to idle.
- File failure or cancellation after `ready` cleans the partial file and closes both peers.
- Simultaneous proposals always select the pairing initiator's request and preserve the responder's local queued request.
- Manual close works from both sides.
- Exactly 600 idle seconds closes the session; starting a valid proposal before the boundary stops that idle deadline.
- Active transfer time is not session idle but is covered by progress timeouts.
- A closed session cannot resume and the next connection requires a new pairing request and code.

## Transfer And Filesystem Tests

- One-file, multi-file, zero-byte, and larger-than-memory transfers succeed in order.
- No file data is accepted before pairing, transfer acceptance, and `ready`.
- Duplicate, equivalent, unsafe, reserved, existing, and path-like names are rejected.
- Symlink races, destination creation races, source changes, disk full, write failure, wrong size, wrong hash, disconnect, and cancellation clean partial files.
- A later-file failure keeps exactly the verified prefix and starts no later file.
- Received files are never overwritten, opened, previewed, or executed automatically.

## Property And Fuzz Tests

- Fuzz frame decoding, strict JSON, protocol state, command parsing, pasted paths, and destination-name mapping.
- Arbitrary network segmentation either decodes the same sequence or gives one bounded error.
- Arbitrary state events never permit metadata before pairing or data before approval.
- A final file always has the declared size and digest.
- Counts, lengths, totals, indices, progress, queues, and timers never overflow their bounds.

## Integration And Fault Tests

- Run two app processes over loopback and real LANs.
- Disconnect at every pairing, proposal, file, and session boundary.
- Inject partial writes, delayed frames, full channels, storage failures, and cancellation races.
- Test supported operating-system and filesystem pairs.
- Measure memory, CPU, throughput, many-small-file behavior, prompt flooding, and connection flooding.

## Release Gates

- TUI, discovery, message, session, transfer, and cross-platform tests pass.
- Fuzzing has no known crash, path escape, or unbounded-allocation case.
- Cryptographic vectors, dependency audit, certificate verifier review, and composition review pass.
- `cargo audit` and `cargo deny` pass under the written policy.
- Security wording matches the audit evidence and does not overstate safety.
