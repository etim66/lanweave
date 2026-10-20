# Error Handling

Wire responses use stable codes. Paths, operating-system errors, stack traces, pairing codes, cryptographic values, and free-form diagnostics remain local.

## Pairing Rejection

`pair_response` rejects before code generation:

| Reason | Meaning |
| --- | --- |
| `user_rejected` | The responder declined. |
| `busy` | The responder cannot handle another pairing request. |
| `timeout` | The responder did not decide before the prompt deadline. |
| `unavailable` | The app cannot start pairing. |

Pairing rejection closes the provisional connection. No code or authorization state is created.

## Authentication Failure

Pairing authentication failures use `authentication_failed` when a response is safe, or a silent close. The wire does not reveal whether the code was wrong, expired, exhausted, already used, or bound to another connection.

Authentication failure closes the connection and clears code and pairing state. A later attempt starts a new pairing request. There is no trusted-device or reconnect shortcut.

## Transfer Rejection

`transfer_response` rejection ends only the proposal:

| Reason | Meaning |
| --- | --- |
| `user_rejected` | The recipient declined the complete manifest. |
| `busy` | Another proposal won a simultaneous request race. |
| `timeout` | The recipient did not decide before the proposal deadline. |
| `invalid_manifest` | Count, size, or total is invalid. |
| `invalid_filename` | A filename is unsafe for the destination. |
| `name_conflict` | Requested names conflict with one another. |
| `destination_exists` | Reserved for compatibility; an occupied destination name no longer rejects a proposal. |
| `insufficient_storage` | Storage policy cannot accept the request. |
| `resource_limit` | A bounded local policy rejected the request. |
| `unavailable` | The recipient cannot accept now. |

No file data follows rejection. Both peers return to session idle and restart the 600-second idle timer.

## File Failure

`file_result` reports current-file failure:

| Code | Meaning |
| --- | --- |
| `size_mismatch` | Received bytes do not match the declared size. |
| `hash_mismatch` | The calculated SHA-256 does not match. |
| `write_failed` | Writing or finalizing failed. |
| `destination_exists` | No free copy name was available within the bounded finalization search. |
| `insufficient_storage` | Storage became unavailable. |

The recipient deletes the current partial file, keeps the verified prefix, and does not attempt later files. The peers then close the session because file frames may already be in flight. A future transfer requires fresh pairing.

## Transfer Cancellation

`transfer_cancel` stops the current proposal or transfer. `user_cancelled` means a user stopped it. `source_unavailable` means an approved source can no longer be read unchanged.

Cancellation before `ready` returns the proposal to session idle. Cancellation after `ready` removes the current partial file and closes the session because file frames may already be in flight.

## Session Close

`session_close` ends authorization:

| Code | Meaning |
| --- | --- |
| `user_closed` | A participant manually disconnected. |
| `idle_timeout` | The session was idle for 600 seconds. |
| `shutdown` | The app is exiting. |

The message is not acknowledged. Both peers close transport and clear session secrets. A future session requires full pairing.

## Protocol Error

`error` is terminal. Codes are `unsupported_version`, `invalid_message`, `authentication_failed`, `timeout`, `resource_limit`, and `internal_error`.

Unsafe framing closes without a reply. Safely framed malformed controls may receive `invalid_message`. Never answer `error` with `error`, and never include peer-controlled diagnostics in a response.

## Cleanup

Cleanup is idempotent. Closing a session cancels pending prompts, stops file reads and writes, removes the current partial file, closes sockets, stops timers, and clears temporary authorization. Verified files remain. Cleanup errors are shown locally without exposing paths to the peer.
