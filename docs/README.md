# Documentation

<!-- SPDX-FileCopyrightText: 2026 Unyime Etim -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

See the [README](../README.md) for install and a quick start. For product behavior and scope, start with the [project overview](PROJECT_OVERVIEW.md); for commands and keyboard shortcuts, see the [usage guide](USAGE.md).

## Using Lanweave

| Document | Purpose |
| --- | --- |
| [Usage guide](USAGE.md) | Commands, keyboard shortcuts, dialogs, and session flow |
| [Project overview](PROJECT_OVERVIEW.md) | Product model, goals, non-goals, and success criteria |
| [Glossary](GLOSSARY.md) | Plain-language project terms |
| [Updates](UPDATES.md) | `/update` eligibility, behavior, and trust model |

## Design

| Document | Purpose |
| --- | --- |
| [Architecture](ARCHITECTURE.md) | Module boundaries, events, and task ownership |
| [Protocol](PROTOCOL.md) | Required connection, pairing, session, and transfer behavior |
| [Message format](MESSAGE_FORMAT.md) | JSON controls, binary file frames, and limits |
| [State machines](STATE_MACHINES.md) | TUI, pairing, session, and transfer states |
| [Sequence diagrams](SEQUENCE_DIAGRAMS.md) | Main user and network flows |
| [Discovery](DISCOVERY.md) | Live device discovery and its trust limits |
| [Transport](TRANSPORT.md) | TCP/TLS transport and session lifetime |
| [File transfer](FILE_TRANSFER.md) | File selection, approval, streaming, and cleanup |
| [Error handling](ERROR_HANDLING.md) | Stable error codes and local diagnostics |
| [Design decisions](DESIGN_DECISIONS.md) | Accepted decisions and their rationale |
| [Versioning](VERSIONING.md) | Application, specification, protocol, and frame versions |

## Security

| Document | Purpose |
| --- | --- |
| [Security](SECURITY.md) | Security requirements and boundaries |
| [Threat model](THREAT_MODEL.md) | Threats, controls, and remaining risks |
| [Cryptography](CRYPTOGRAPHY.md) | Pairing profile and security review gates |

## Development

| Document | Purpose |
| --- | --- |
| [Contributing](../CONTRIBUTING.md) | Development workflow, checks, and module ownership |
| [Testing strategy](TESTING_STRATEGY.md) | TUI, protocol, filesystem, and security tests |
| [Implementation roadmap](IMPLEMENTATION_ROADMAP.md) | High-level dependency gates |
| [Research plan](RESEARCH_PLAN.md) | Experiments and evidence needed for design decisions |
| [Releasing](RELEASING.md) | Version, tag, and release workflow |
