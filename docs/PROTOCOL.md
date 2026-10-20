# Lanweave Protocol Specification

**Wire protocol:** experimental version 1

**Status:** draft and not audited

This document is normative. **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** describe interoperability requirements.

## Scope

Version 1 creates one authorized session over one TCP/TLS connection. That session can carry several separately approved file transfers in either direction. Only one proposal or transfer is active at a time.

Directories, paths, symlinks, special files, parallel transfers, internet routing, session resume, and trusted devices are outside version 1.

## Roles

Pairing has fixed roles:

- The **initiator** selects a discovered device, sends `pair_request`, and enters the code. It is SPAKE2 Party A.
- The **responder** accepts or rejects the request and displays the locally generated code. It is SPAKE2 Party B and the TLS server.

Every transfer has separate roles:

- The **requester** proposes and sends files.
- The **recipient** reviews the proposal and receives accepted files.

Either pairing participant can be the requester for a later transfer.

## Fixed Profile

Version 1 uses TLS 1.3, ALPN `lanweave/1`, the framing and messages in [Message Format](MESSAGE_FORMAT.md), SHA-256 file digests, and the pairing profile in [Cryptography](CRYPTOGRAPHY.md). These choices are fixed and are not negotiated by Lanweave.

The responder creates the pairing code only after accepting `pair_request`. The code is eight decimal digits, kept in memory, valid for 120 seconds, and usable once. The responder shows it locally. The initiator enters it locally. The code MUST NOT appear in discovery, logs, or a normal wire message.

TLS encrypts all Lanweave frames. Pairing confirms that both ends know the code and binds that check to the current TLS connection. This design remains experimental and release-blocked on specialist review and an approved pairing implementation.

## Connection And Pairing Sequence

1. The peers establish TCP and complete TLS 1.3 with ALPN `lanweave/1`. TLS resumption, tickets, 0-RTT, and early application data are disabled.
2. The initiator sends `hello`; the responder replies with `hello`. Both contain exact protocol version `1`.
3. The initiator sends `pair_request`.
4. The responder shows the request in the TUI and sends `pair_response` with accept or reject.
5. On rejection, the connection closes. On acceptance, the responder creates and displays the code and starts its 120-second deadline.
6. The initiator enters the code. Cancelling code entry closes the connection.
7. The peers exchange four `pairing` records: initiator share, responder share, initiator confirmation, responder confirmation.
8. Each peer marks the session authorized only after the required confirmation is verified and its own confirmation is fully written and flushed.
9. Pairing secrets are cleared on a best-effort basis. The session enters `idle`.

The responder MUST NOT generate or display a code before accepting a live request. Accepting the request does not itself authorize the session.

## Session Sequence

An authorized session repeats this loop until it closes:

1. While idle, either participant may send one `transfer_request` containing the complete ordered manifest.
2. The recipient validates the request, shows all metadata, and asks the user to accept or reject it.
3. Before accepting, the recipient selects and prepares the destination. Failure sends a rejecting `transfer_response`.
4. On rejection, both peers return to session idle and no file bytes are sent.
5. On acceptance, the recipient sends an accepting `transfer_response` followed by `ready`.
6. The requester sends each file in order as `DATA` frames followed by `file_end`.
7. The recipient verifies and finalizes the file, then sends `file_result`.
8. A verified final result completes only that transfer. Both peers return to session idle.
9. A failure or cancellation after `ready` cleans the active transfer and closes the session. This prevents in-flight file frames from being mistaken for a later transfer. A rejection or cancellation before `ready` returns to session idle.

## Simultaneous Requests

The TUI disables **Send** while a proposal or transfer is active, but both peers can still race while idle. Version 1 resolves that race without request IDs:

- the pairing initiator's request has priority;
- if the responder receives an initiator request while its own request is pending, it withdraws and locally re-queues its request, then records that one stale `busy` response for the withdrawn request is pending;
- if the initiator receives a responder request while its own request is pending, it replies with `transfer_response` reason `busy`; and
- the responder consumes that one expected stale `busy` response in any state of the winning initiator transfer and clears the pending marker; and
- the responder may send its queued request after the initiator's transfer returns to idle.

This rule applies only to simultaneous proposals. It does not give the initiator permanent transfer priority.

## Session Lifetime

The session idle timer starts when pairing completes and restarts when a transfer finishes or a proposal is rejected or cancelled before `ready`. It expires after 600 consecutive seconds without a new transfer request.

An active transfer is not session idle. It has separate progress deadlines. Pairing and approval prompts also have bounded local deadlines.

Either participant MAY send `session_close` at any time after authorization. If a transfer is active, both peers stop it and clean the current partial file. App shutdown, idle expiry, transport loss, or a terminal error also closes the session. Closing removes all temporary authorization. A later connection MUST repeat the full pairing flow; TLS resumption and trusted-device shortcuts are forbidden.

## Transfer Admission

A request MUST contain 1 through 1,024 regular files. Every entry contains only a filename and exact size. The recipient rejects the complete request if:

- a field exceeds a fixed limit;
- a name is invalid, path-like, duplicate, or equivalent on the destination platform;
- checked size, storage, or resource policy fails; or
- the user rejects the request.

An occupied destination name does not reject the request. The recipient MUST NOT accept a subset, alter order, alter the wire manifest, or overwrite an existing destination. It MAY publish an entry under a local copy name such as `name (Copy).ext` when the requested name is occupied; this local name is not sent on the wire.

## Streaming

Files are sent sequentially with bounded buffers. `DATA` belongs to the current file and is at most 1 MiB per frame. The requester sends exactly the declared byte count and then `file_end(index, sha256)`. It waits for `file_result` before starting the next file.

On success, the recipient safely finalizes the file before sending `file_result` with `verified`. On the first file failure, it deletes the current partial file, keeps the verified prefix, and does not attempt later files.

## Failure Scope

These outcomes close the connection:

- pairing rejection, failure, expiry, or cancellation;
- malformed framing or unsafe input;
- wrong-state controls that make state unreliable;
- session idle expiry;
- `session_close`;
- transport loss; and
- an internal error that makes safe continuation impossible.

These outcomes end only the current proposal:

- `transfer_response` rejection, including `busy`; and
- `transfer_cancel` before `ready`.

These outcomes clean the current transfer and close the session:

- `transfer_cancel` after `ready`; and
- a failed `file_result`.

## Fixed Limits

| Item | Limit |
| --- | ---: |
| Filename | 255 UTF-8 bytes |
| Files per manifest | 1 to 1,024 |
| `transfer_request` body | 256 KiB |
| Exact `hello` body | 4,000 bytes |
| `pairing` body | 4,096 bytes |
| Generic JSON body | 1 MiB |
| `DATA` frame | 1 MiB |
| Integer and checked total | `2^53-1` |
| Pairing code lifetime | 120 monotonic seconds |
| Pairing attempts per accepted request | 1 |
| Session idle time | 600 monotonic seconds |

## Deferred

Directories, resume, parallel files or transfers, overwrite or rename negotiation, compression, chunk acknowledgements, application flow windows, capability negotiation, algorithm negotiation, persistent identity, trusted devices, and automatic reconnect are deferred.
