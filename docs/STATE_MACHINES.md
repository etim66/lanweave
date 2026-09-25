# State Machines

## Application

| State | Event | Next state |
| --- | --- | --- |
| `starting` | Terminal, listener, discovery, and event loop start | `home` |
| `home` | User opens `/devices` | `browsing` |
| `home` or `browsing` | Incoming pairing request | `pairing_inbound` |
| `browsing` | User selects a visible device | `pairing_outbound` |
| Any pairing or session state | Another incoming pairing request; reject as `busy` | Same state |
| `pairing_outbound` | Request accepted and code succeeds | `session_idle` |
| `pairing_inbound` | User accepts and pairing succeeds | `session_idle` |
| `session_idle` | Local user proposes files | `outbound_proposal` |
| `outbound_proposal` | Folder archive prepared and `transfer_request` sent | `outbound_proposal` |
| `session_idle` | Peer proposes files | `inbound_proposal` |
| Transfer finished, or cancelled after `ready` | Both sides show a summary | `transfer_complete` |
| Transfer cancelled before `ready` | The cancelling side shows a summary; the peer returns to idle | `transfer_complete` |
| `transfer_complete` | User dismisses the summary | `session_idle` (or `home` if the session closed) |
| Any pairing or session state | Session closes or connection fails | `home` |
| Any state | User quits or process shutdown begins | `shutting_down` |

Entering `/` opens the command list without changing the network state. Commands that are unsafe in the current state are hidden or disabled.

Screens that wait for the local user (pairing request, transfer review, exclusive
code sharing, waiting for a response or approval, transfer summary) show at least
one highlighted button. Left/Right moves between Accept and Reject, Enter chooses
the focused button, and Escape remains a shortcut for reject or cancel.

## Pairing Initiator

| State | Event and action | Next state |
| --- | --- | --- |
| `connecting` | TLS 1.3 succeeds; exchange `hello`; send `pair_request` | `awaiting_pair_response` |
| `awaiting_pair_response` | Rejected or prompt times out | `closed` |
| `awaiting_pair_response` | Accepted | `awaiting_code_entry` |
| `awaiting_code_entry` | User enters a valid eight-digit shape | `pairing` |
| `awaiting_code_entry` | User cancels or deadline expires | `closed` |
| `pairing` | Send A share, verify B share, send A confirmation, verify B confirmation | `session_idle` |
| Any state | Authentication, protocol, transport, or app failure | `closed` |

## Pairing Responder

| State | Event and action | Next state |
| --- | --- | --- |
| `awaiting_request` | Valid `pair_request`; show prompt | `awaiting_user_decision` |
| `awaiting_user_decision` | User rejects or prompt expires; send rejection | `closed` |
| `awaiting_user_decision` | User accepts; send acceptance, create code, display it, start 120-second deadline | `awaiting_initiator_share` |
| `awaiting_initiator_share` | Valid A share before deadline; send B share | `awaiting_initiator_confirm` |
| `awaiting_initiator_confirm` | Valid confirmation succeeds; consume code and queue B confirmation | `flushing_responder_confirm` |
| `flushing_responder_confirm` | Complete B confirmation is written and flushed | `session_idle` |
| Any state | Authentication, protocol, transport, or app failure | `closed` |

The code exists only after request acceptance. It is memory-only, one-use, allows one cryptographic pairing attempt, and cannot authorize another connection.

## Authorized Session

| State | Event and action | Next state |
| --- | --- | --- |
| `session_idle` | Local files submitted; send `transfer_request` | `awaiting_transfer_response` |
| `outbound_proposal` | User cancels during folder preparation; no request is sent | `transfer_complete` |
| `outbound_proposal` | User cancels before `ready`; send `transfer_cancel` | `transfer_complete` |
| `session_idle` | Peer `transfer_request` | `reviewing_transfer` |
| `session_idle` | 600-second idle deadline | `closed` |
| `awaiting_transfer_response` | Rejected | `session_idle` |
| `awaiting_transfer_response` | Accepted | `awaiting_ready` |
| `awaiting_ready` | `ready` | `sending_file` |
| `reviewing_transfer` | User or policy rejects; send response | `session_idle` |
| `reviewing_transfer` | User accepts and preparation succeeds; send acceptance and `ready` | `receiving_file` |
| `sending_file` | All files receive verified results | `session_idle` |
| `receiving_file` | All files are verified and results sent | `session_idle` |
| Any proposal state before `ready` | Safe rejection or `transfer_cancel` | `session_idle` |
| Any active transfer state | User cancels; send `transfer_cancel`, delete the partial | `closed` |
| Any active transfer state | Peer `transfer_cancel`; clean the partial, show a cancelled summary | `closed` |
| Any authorized session state | Local or peer `session_close`; clean an active transfer | `closed` |
| Any state | Terminal protocol, transport, or internal failure | `closed` |

Returning to `session_idle` starts a new 600-second deadline. Starting a proposal stops that idle deadline. Transfer progress deadlines apply while data is active.

## Simultaneous Proposal Race

Only one proposal can win:

- If the responder has a local proposal pending and receives the initiator's proposal, it re-queues its own proposal and reviews the initiator's.
- If the initiator has a local proposal pending and receives the responder's proposal, it rejects the responder's proposal as `busy`.
- The responder records `collision_response_pending` when it yields. It consumes exactly one later `transfer_response(busy)` for the withdrawn proposal in any state of the winning transfer, then clears that marker.
- Both peers then agree that the initiator's proposal is active.

## Transfer Invariants

- Pairing does not approve files.
- The full manifest is shown before transfer acceptance.
- No `DATA` is sent before acceptance and `ready`.
- Files are sent one at a time and in manifest order.
- The requester waits for each `file_result` before starting the next file.
- A failed file deletes its partial file and keeps the verified prefix.
- Transfer completion or pre-`ready` rejection and cancellation does not end the session.
- Cancellation or failure after `ready` closes the session because DATA may already be in flight.
- Session close removes authorization. A later session must pair again.
