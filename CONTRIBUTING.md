# Contributing

Lanweave is security-sensitive and still under active design. Changes should
preserve the invariants in `docs/` and avoid presenting planned security work as
implemented or audited.

## Local Setup

Install stable Rust at or above the MSRV declared in `Cargo.toml`. Run:

```sh
make check
```

This verifies formatting, Clippy with warnings denied, and all tests using the
locked dependency graph. Use `make ci` before a release-oriented change to also
run dependency policy and advisory checks.

## Module Guide

| Change | Primary location |
| --- | --- |
| Process startup, task spawning, shutdown | `src/bootstrap.rs` |
| Domain state and capabilities | `src/app/model.rs` |
| User intent and key contracts | `src/app/action.rs` |
| Runtime events and requested effects | `src/app/event.rs` |
| Domain state transitions | `src/app/reducer.rs` |
| Overlay and command-palette behavior | `src/app/interaction.rs` |
| Slash-command registry | `src/app/command_palette.rs` |
| Terminal input mapping | `src/tui/event.rs` |
| Screens and visual components | `src/tui/view/` |
| Terminal setup, cleanup, and panic handling | `src/tui/terminal/` |
| Target architecture and ownership | `docs/ARCHITECTURE.md` |

Concrete discovery, session, protocol, transport, transfer, and storage code
must remain independent of the TUI. Add adapter-specific errors at their source
and convert them to safe application failures at the boundary.

## Tests

Keep deterministic unit tests next to the module they exercise. Reserve the
top-level `tests/` directory for behavior crossing real process, network, or
filesystem boundaries. Add protocol vectors and other data-only inputs under
`tests/fixtures/` when those implementations arrive.

Every behavior change should include tests for its state boundaries and failure
path. Security or protocol changes should also update the corresponding design
document.

## Pull Requests

Keep changes focused and explain any ownership or invariant changes. Include the
commands run for verification and call out remaining platform, security, or
interoperability risks.
