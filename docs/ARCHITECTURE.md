# Architecture

## Initial Shape

The first implementation is one Cargo package with a private library implementation and a thin binary entry point. Modules create clear internal boundaries without committing to public crate APIs.

```text
main
  app event loop
    tui
    discovery
    session
      protocol and framing
      transport and pairing
      transfer and storage
```

Suggested module names are `tui`, `app`, `command`, `discovery`, `session`, `protocol`, `framing`, `transport`, `pairing`, `transfer`, and `storage`.

Keep one package until reuse, platform isolation, dependency control, or independent fuzzing gives a clear reason to split it.

## Implemented Shape

The current shell uses this dependency direction:

```text
main -> public run facade -> bootstrap
                              |-> app runtime and reducer
                              |-> TUI input and rendering
                              `-> service effect dispatch

TUI -> app model, capabilities, and runtime channels
app -/-> TUI or concrete adapters
```

`bootstrap` is the composition root. It creates the terminal, bounded channels,
tasks, and shutdown supervision. `app/model.rs` contains domain-facing state;
`app/interaction.rs` owns temporary UI state; and `tui/view/` contains focused
rendering components. Discovery browsing, mDNS advertisement, and the temporary
TCP listener are owned by the service effect dispatcher; later transport and
session adapters remain placeholders.

| Area | Owns | Must not own |
| --- | --- | --- |
| `bootstrap` | Construction, tasks, shutdown order | Domain policy or rendering |
| `app` | User-visible state, actions, capabilities, reduction | Terminal or adapter APIs |
| `tui` | Key mapping, interaction rendering, terminal lifecycle | Protocol and session policy |
| `session` | Live connection authorization, proposals, timers | Terminal rendering |
| `protocol` | Wire ordering and message validation | Sockets, files, or user consent |
| `transport` | Bounded framed I/O | Authorization and consent policy |
| `storage` | Safe names, temporary files, no-overwrite finalization | TUI or wire policy |

## Application Event Loop

The application loop owns the user-visible state. It receives bounded events from:

- terminal input and resize events;
- discovery updates;
- pairing and session tasks;
- transfer progress and results; and
- shutdown signals.

The loop updates a plain application model and asks the TUI to render it. Network and disk tasks never write directly to the terminal. The TUI never blocks a network task while waiting for user input.

`/` opens a command list built from one command registry. The registry supplies command names, help text, and the states in which each command is available. Typed commands and buttons dispatch the same application actions.

## Dependency Direction

```text
TUI ----------> app/state <---------- discovery adapter
                    |
                    +--> session state
                    +--> protocol validation
                    +--> transfer policy <------ filesystem adapter
                    +--> pairing adapter <------ reviewed PAKE library
                    +--> framed connection <---- TCP/TLS adapter
```

- The TUI renders state and sends user actions. It does not contain protocol rules.
- Discovery emits untrusted device candidates. It starts and stops with the app.
- Session state owns pairing, authorization, one active transfer, and the idle timer.
- Protocol validation has no dependency on the TUI, sockets, mDNS, or filesystem APIs.
- Transport carries bounded frames and reports events. It does not decide user consent.
- Storage validates names, creates temporary files, and never invents overwrite or rename behavior.
- The pairing adapter wraps a reviewed library. Lanweave does not implement group arithmetic.

## Session Ownership

One task owns each connection and its mutable session state. Reader, writer, file I/O, and timer tasks communicate with that owner through bounded channels.

The application model receives a UI-safe snapshot of that state; it does not
duplicate or mutate the live session state machine. Protocol validation owns
wire-order rules, while the session owner decides connection lifecycle and
publishes outcomes to the application loop.

The owner enforces these rules:

- pairing must finish before the session becomes active;
- only one transfer request or transfer is active at a time;
- either participant can become the requester for the next transfer;
- transfer completion or rejection returns to session idle;
- transfer rejection returns to idle, while failure or cancellation after `ready` cleans the transfer and closes the session;
- protocol, authentication, and transport failures close the session;
- `session_close` or 600 seconds of idle time closes the session; and
- closing drops all temporary authorization and requires fresh pairing next time.

A single writer serializes complete frames. Bounded channels carry backpressure to file readers. Blocking filesystem work runs outside async network tasks.

## Discovery Lifecycle

App startup creates the listener before advertising it. Browsing and advertising remain active while the TUI runs. App shutdown stops advertisements, browsing, listeners, connections, and temporary session state in that order where practical.

An active session does not depend on its discovery record. If discovery changes or disappears after connection, the session continues.

## Pairing Roles And Transfer Roles

Pairing roles do not change during one session:

- the **initiator** selects a device, sends `pair_request`, and enters the code;
- the **responder** accepts the request and displays the locally generated code.

Transfer roles can change for every request:

- the **requester** proposes files and sends accepted file bytes;
- the **recipient** reviews metadata and saves accepted files.

Do not use `sender` and `receiver` as session-wide identities.

## Filesystem Boundary

Before accepting a transfer, storage validates every filename against the destination platform, checks conflicts, confirms resource policy, and prepares the first temporary file. Unsafe, duplicate, equivalent, existing, or unpreparable names reject the whole request.

Incoming data is written only to the current temporary file. Exact size and digest verification are required before safe no-overwrite finalization. A later failure keeps the verified prefix and removes the current partial file.
