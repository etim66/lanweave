# Threat Model

## Scope

Attackers on the LAN may publish false devices, copy names, observe traffic shape, open many connections, flood prompts, send arbitrary frames, and delay or drop traffic. This model assumes uncompromised endpoints, a working operating-system random generator, a correct TLS implementation, and users who privately share the displayed code.

## Threats And Controls

| Threat | Control | Remaining risk |
| --- | --- | --- |
| Spoofed visible device | Treat discovery as untrusted and require pairing confirmation before metadata | Attackers can create noise or denial of service |
| Misleading display name | Escape and visibly quote untrusted text; never treat a name as identity | Similar names may still confuse users |
| Pairing-request spam | Bound requests and prompts by source and globally; add prompt deadlines | Attackers can consume bounded attention and resources |
| Pairing code disclosure | Generate after acceptance, display locally, never send or log, expire after 120 seconds | Anyone who sees the live code may pair |
| Online code guessing | One cryptographic attempt per accepted request, short expiry, prompt limits, and rate limits | Attackers can cause prompts or denial of service |
| Split-connection intermediary | Bind pairing confirmation to the live TLS exporter and exact hello bodies | The composition still needs specialist review |
| Replay | Fresh TLS and pairing values; strict state; no resumption | Replays still consume bounded parsing work |
| Prompt or transfer flooding in a session | One active proposal, bounded prompt time, `busy` collision handling | A paired peer can remain annoying until disconnected |
| Simultaneous proposal confusion | Fixed initiator-priority rule and one active proposal | One side may wait and retry its queued proposal |
| Idle resource retention | Close after 600 seconds without a transfer request | Active malicious traffic needs separate progress limits |
| Malformed frames or JSON | Validate fixed lengths and strict schema before allocation or expensive work | Parser defects may remain |
| Terminal escape injection | Escape peer text and control characters | Unicode look-alikes can still confuse users |
| Path traversal and overwrite | Single safe names, platform checks, no-follow temp files, bounded copy names, no-replace finalization | Filesystem edge cases need platform tests |
| Corrupt transfer | TLS integrity plus exact size and SHA-256 before finalization | A paired peer can intentionally choose harmful content |
| Interrupted transfer | Delete current partial and keep verified files | A transfer is not all-or-nothing |
| Authorization reuse | No trust database, TLS resume, reusable code, or reconnect token | Users must repeat pairing after every disconnect |
| Secret logging | Redact pairing, TLS, path, and content secrets; test logs | Endpoint compromise and memory copies remain out of scope |

## Out Of Scope

Lanweave cannot protect a compromised computer, prove a person's real-world identity, make a received file safe to open, hide all traffic metadata, protect a disclosed live code, or guarantee network availability.

Directories, resume, parallel transfer, trusted devices, automatic acceptance, compression, QUIC, and mobile clients require a new threat review before inclusion.
