# Design Decisions

| ID | Status | Decision | Reason |
| --- | --- | --- | --- |
| DD-001 | Accepted | `lanweave` opens a persistent TUI. | Discovery, incoming requests, and repeat transfers require a running app. |
| DD-002 | Accepted | Entering `/` shows commands available in the current state. | Users can discover actions without memorizing flags. |
| DD-003 | Accepted | Discovery and advertising run only while the app runs. | A listed device must be available to receive a request. |
| DD-004 | Accepted | The initiator selects a device; the responder accepts or rejects before a code is created. | This matches the visible user flow and avoids unsolicited code ceremonies. |
| DD-005 | Accepted | The responder creates and displays a one-time eight-digit code after acceptance. The code is never sent as a normal wire field. | The code authorizes the intended live connection without exposing it to that untrusted connection. |
| DD-006 | Accepted | One pairing authorizes one live session. A session can carry several sequential transfers in either direction. | Users can continue sharing without repeating the code after every transfer. |
| DD-007 | Accepted | Only one proposal or transfer is active at a time. The initiator wins a simultaneous proposal race. | This bounds state and avoids request IDs in version 1. |
| DD-008 | Accepted | Every transfer has a complete immutable manifest and separate recipient approval. | Pairing is not file consent. |
| DD-009 | Accepted | Files transfer sequentially and stop on the first file failure. Completed files remain; the current partial is deleted. | This is bounded and works across common filesystems. |
| DD-010 | Accepted | Rejection before `ready` returns to idle; failure or cancellation after `ready` closes the session. | In-flight DATA makes reuse unsafe without a transfer barrier or identifier. |
| DD-011 | Accepted | Sessions close manually or after 600 seconds of idle time. | Temporary authorization must not remain open without a clear limit. |
| DD-012 | Accepted | Lanweave stores no trusted devices, reusable authorization, or TLS resume state. | Every new session must repeat user authorization. |
| DD-013 | Accepted | JSON carries control messages; binary `DATA` frames carry file bytes. | JSON is easy to inspect and binary data avoids encoding overhead. |
| DD-014 | Accepted | Start with one Cargo package, a private library implementation, a thin binary, and internal modules. | Package boundaries should follow proven reuse or isolation needs. |
| DD-015 | Research Gate | Approve the exact TLS, exporter, SPAKE2 composition, and pairing dependency before a security claim. | The design is experimental and cryptographic mistakes would be critical. |

## Deferred

Directories, resume, parallel files or transfers, compression, overwrite or rename, trusted devices, algorithm negotiation, QUIC, mobile support, and graphical interfaces are outside the MVP. The TUI is required and is not deferred.
