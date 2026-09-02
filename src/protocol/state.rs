//! Protocol state validation for version 1 controls and DATA.
//!
//! Owns direction and ordering rules for every control across pairing and
//! authorized sessions (`docs/STATE_MACHINES.md`). Filesystem, cryptography,
//! and sockets stay outside: callers feed decoded controls in and receive
//! actions and state transitions out. An error means the peer or the local
//! caller violated the sequence, which is always session-terminal.
//!
//! Simultaneous transfer proposals follow the fixed initiator-priority rule:
//! the responder withdraws its own pending proposal and later consumes exactly
//! one stale `transfer_response` with reason `busy` for it, in any phase of the
//! winning transfer.

use std::fmt;

use bytes::Bytes;

use super::message::{
    Control, FileStatus, PairResponse, PairingStep, TransferRejection, TransferRequest,
    TransferResponse,
};

/// Number of pairing records exchanged after an accepted `pair_response`.
const RECORD_COUNT: u8 = 4;

/// Fixed pairing role of one connection end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Selects the peer, sends `pair_request`, and enters the code.
    Initiator,
    /// Accepts or rejects the request and displays the code.
    Responder,
}

/// Protocol phase within pairing or an authorized session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// TLS is done; `sent` and `got_peer` track the hello exchange.
    Hello { sent: bool, got_peer: bool },
    /// Both hellos are complete; the request is pending in one direction.
    PairRequest,
    /// The pairing decision is pending.
    PairResponse,
    /// Pairing records exchanged so far (`0..4`).
    Records { records: u8 },
    /// Authorized with no active proposal or transfer.
    Idle,
    /// A local `transfer_request` awaits the peer's response.
    AwaitingResponse { files: u16 },
    /// A peer proposal is under review, or accepted and awaiting `ready`.
    Reviewing { files: u16, accepted: bool },
    /// Files are being sent; `started` means `ready` arrived.
    Sending {
        files: u16,
        index: u16,
        started: bool,
        awaiting_result: bool,
    },
    /// Files are being received after `ready` was sent.
    Receiving { files: u16, index: u16, ended: bool },
    /// Terminal: the session or pairing is closing and rejects all traffic.
    Closing,
}

impl Phase {
    /// Whether the phase belongs to an authorized session.
    const fn is_authorized(self) -> bool {
        matches!(
            self,
            Self::Idle
                | Self::AwaitingResponse { .. }
                | Self::Reviewing { .. }
                | Self::Sending { .. }
                | Self::Receiving { .. }
        )
    }
}

/// Mutable protocol state of one connection end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolState {
    role: Role,
    phase: Phase,
    collision_pending: bool,
}

impl ProtocolState {
    /// Creates the initial state for one connection end.
    pub const fn new(role: Role) -> Self {
        Self {
            role,
            phase: Phase::Hello {
                sent: false,
                got_peer: false,
            },
            collision_pending: false,
        }
    }

    /// Returns the current phase.
    pub const fn phase(&self) -> &Phase {
        &self.phase
    }

    /// Whether the session is authorized and not closing.
    pub const fn is_authorized(&self) -> bool {
        self.phase.is_authorized()
    }
}

/// One decoded message arriving from the peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
    /// A decoded control body.
    Control(Control),
    /// Raw `DATA` frame bytes for the current file.
    Data(Bytes),
}

/// Work requested by the protocol layer after a validated message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolAction {
    /// Send a generated control to the peer.
    Send(Control),
    /// Hand `DATA` bytes to the local transfer sink.
    DeliverData(Bytes),
    /// Pairing completed; the session is authorized and idle.
    Authorized,
    /// The proposal ended before `ready`; the session stays authorized.
    ProposalEnded,
    /// Every file completed; the session stays authorized and idle.
    TransferFinished,
    /// Pairing ended without authorization; close the connection.
    PairingClosed,
    /// The session ended; stop any transfer, clean partial files, close.
    SessionClosed,
}

/// A protocol violation. Every variant is session-terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    /// The control is not acceptable in the current phase.
    WrongState,
    /// `DATA` is not acceptable in the current phase or direction.
    DataNotAllowed,
    /// A value contradicts the tracked sequence (step, file index).
    InvalidSequence,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::WrongState => "control is not valid in the current protocol state",
            Self::DataNotAllowed => "DATA is not allowed in the current protocol state",
            Self::InvalidSequence => "value contradicts the tracked protocol sequence",
        };
        formatter.write_str(text)
    }
}

impl std::error::Error for ProtocolError {}

/// Validates one inbound message and applies it to the state.
pub fn accept(
    state: &mut ProtocolState,
    inbound: Inbound,
) -> Result<Vec<ProtocolAction>, ProtocolError> {
    // The responder consumes exactly one stale busy response for its
    // withdrawn proposal, in any phase of the winning transfer.
    if state.collision_pending
        && let Inbound::Control(Control::TransferResponse(response)) = &inbound
        && matches!(response.reason, Some(TransferRejection::Busy))
    {
        state.collision_pending = false;
        return Ok(Vec::new());
    }

    match inbound {
        Inbound::Control(control) => accept_control(state, control),
        Inbound::Data(data) => accept_data(state, data),
    }
}

/// Validates one outbound control and applies it to the state.
pub fn send(
    state: &mut ProtocolState,
    control: &Control,
) -> Result<Vec<ProtocolAction>, ProtocolError> {
    let phase = state.phase;
    match (phase, control) {
        (Phase::Closing, _) => Err(ProtocolError::WrongState),

        // A local terminal error closes without further protocol traffic.
        (_, Control::Error(_)) => {
            let action = terminal_action(phase);
            state.phase = Phase::Closing;
            Ok(vec![action])
        }

        (Phase::Hello { sent, got_peer }, Control::Hello(_)) => match state.role {
            Role::Initiator if !sent => {
                state.phase = Phase::Hello {
                    sent: true,
                    got_peer,
                };
                Ok(Vec::new())
            }
            Role::Responder if got_peer && !sent => {
                state.phase = Phase::PairRequest;
                Ok(Vec::new())
            }
            _ => Err(ProtocolError::WrongState),
        },

        (Phase::PairRequest, Control::PairRequest) => match state.role {
            Role::Initiator => {
                state.phase = Phase::PairResponse;
                Ok(Vec::new())
            }
            Role::Responder => Err(ProtocolError::WrongState),
        },

        (Phase::PairResponse, Control::PairResponse(response)) => match state.role {
            Role::Responder => finish_pair_response(state, response),
            Role::Initiator => Err(ProtocolError::WrongState),
        },

        (Phase::Records { records }, Control::Pairing(record)) => {
            if record.step != expected_step(records) {
                return Err(ProtocolError::InvalidSequence);
            }
            if !is_pairing_mover(state.role, records) {
                return Err(ProtocolError::WrongState);
            }
            advance_pairing(state, records + 1)
        }

        (Phase::Idle, Control::TransferRequest(request)) => {
            state.phase = Phase::AwaitingResponse {
                files: manifest_files(request),
            };
            Ok(Vec::new())
        }

        (Phase::AwaitingResponse { .. }, Control::TransferCancel(_)) => {
            state.phase = Phase::Idle;
            Ok(vec![ProtocolAction::ProposalEnded])
        }

        (
            Phase::Reviewing {
                files,
                accepted: false,
            },
            Control::TransferResponse(response),
        ) => {
            if response.accepted {
                state.phase = Phase::Reviewing {
                    files,
                    accepted: true,
                };
                Ok(Vec::new())
            } else {
                state.phase = Phase::Idle;
                Ok(vec![ProtocolAction::ProposalEnded])
            }
        }

        (
            Phase::Reviewing {
                files,
                accepted: true,
            },
            Control::Ready,
        ) => {
            state.phase = Phase::Receiving {
                files,
                index: 0,
                ended: false,
            };
            Ok(Vec::new())
        }

        (Phase::Reviewing { .. }, Control::TransferCancel(_)) => {
            state.phase = Phase::Idle;
            Ok(vec![ProtocolAction::ProposalEnded])
        }

        (
            Phase::Sending {
                files,
                index,
                started: true,
                awaiting_result: false,
            },
            Control::FileEnd(file_end),
        ) => {
            if file_end.index != index {
                return Err(ProtocolError::InvalidSequence);
            }
            state.phase = Phase::Sending {
                files,
                index,
                started: true,
                awaiting_result: true,
            };
            Ok(Vec::new())
        }

        (
            Phase::Receiving {
                files,
                index,
                ended: true,
            },
            Control::FileResult(result),
        ) => {
            if result.index != index {
                return Err(ProtocolError::InvalidSequence);
            }
            match result.status {
                FileStatus::Verified => {
                    if index + 1 < files {
                        state.phase = Phase::Receiving {
                            files,
                            index: index + 1,
                            ended: false,
                        };
                        Ok(Vec::new())
                    } else {
                        state.phase = Phase::Idle;
                        Ok(vec![ProtocolAction::TransferFinished])
                    }
                }
                FileStatus::Failed => close_session(state),
            }
        }

        (Phase::Sending { started: false, .. }, Control::TransferCancel(_)) => {
            state.phase = Phase::Idle;
            Ok(vec![ProtocolAction::ProposalEnded])
        }

        (Phase::Sending { .. }, Control::TransferCancel(_)) => close_session(state),

        (Phase::Receiving { .. }, Control::TransferCancel(_)) => close_session(state),

        (
            Phase::Idle
            | Phase::AwaitingResponse { .. }
            | Phase::Reviewing { .. }
            | Phase::Sending { .. }
            | Phase::Receiving { .. },
            Control::SessionClose(_),
        ) => close_session(state),

        _ => Err(ProtocolError::WrongState),
    }
}

/// Validates one outbound `DATA` frame for the current file.
pub fn send_data(state: &ProtocolState) -> Result<(), ProtocolError> {
    match state.phase {
        Phase::Sending {
            started: true,
            awaiting_result: false,
            ..
        } => Ok(()),
        _ => Err(ProtocolError::DataNotAllowed),
    }
}

fn accept_control(
    state: &mut ProtocolState,
    control: Control,
) -> Result<Vec<ProtocolAction>, ProtocolError> {
    let phase = state.phase;
    match (phase, control) {
        (Phase::Closing, _) => Err(ProtocolError::WrongState),

        // A peer error is terminal in every phase and is never answered.
        (_, Control::Error(_)) => {
            let action = terminal_action(phase);
            state.phase = Phase::Closing;
            Ok(vec![action])
        }

        // Handshake: the initiator sends the first hello; the responder
        // answers with exactly one hello of its own.
        (Phase::Hello { sent, got_peer }, Control::Hello(_)) => match state.role {
            Role::Initiator if sent && !got_peer => {
                state.phase = Phase::PairRequest;
                Ok(Vec::new())
            }
            Role::Responder if !got_peer => {
                state.phase = Phase::Hello {
                    sent,
                    got_peer: true,
                };
                Ok(Vec::new())
            }
            _ => Err(ProtocolError::WrongState),
        },

        // The responder receives the pairing request after both hellos.
        (Phase::PairRequest, Control::PairRequest) => match state.role {
            Role::Responder => {
                state.phase = Phase::PairResponse;
                Ok(Vec::new())
            }
            Role::Initiator => Err(ProtocolError::WrongState),
        },

        // The initiator consumes the pairing decision.
        (Phase::PairResponse, Control::PairResponse(response)) => match state.role {
            Role::Initiator => finish_pair_response(state, &response),
            Role::Responder => Err(ProtocolError::WrongState),
        },

        // Pairing records alternate: even records belong to the initiator.
        (Phase::Records { records }, Control::Pairing(record)) => {
            if record.step != expected_step(records) {
                return Err(ProtocolError::InvalidSequence);
            }
            if is_pairing_mover(state.role, records) {
                return Err(ProtocolError::WrongState);
            }
            advance_pairing(state, records + 1)
        }

        (Phase::Idle, Control::TransferRequest(request)) => {
            state.phase = Phase::Reviewing {
                files: manifest_files(&request),
                accepted: false,
            };
            Ok(Vec::new())
        }

        (Phase::AwaitingResponse { files }, Control::TransferResponse(response)) => {
            if response.accepted {
                state.phase = Phase::Sending {
                    files,
                    index: 0,
                    started: false,
                    awaiting_result: false,
                };
                Ok(Vec::new())
            } else {
                state.phase = Phase::Idle;
                Ok(vec![ProtocolAction::ProposalEnded])
            }
        }

        // A racing proposal while one is pending: initiator priority.
        (Phase::AwaitingResponse { .. }, Control::TransferRequest(request)) => match state.role {
            Role::Initiator => Ok(vec![ProtocolAction::Send(Control::TransferResponse(
                TransferResponse::rejected(TransferRejection::Busy),
            ))]),
            Role::Responder => {
                state.phase = Phase::Reviewing {
                    files: manifest_files(&request),
                    accepted: false,
                };
                state.collision_pending = true;
                Ok(Vec::new())
            }
        },

        (Phase::AwaitingResponse { .. }, Control::TransferCancel(_)) => {
            state.phase = Phase::Idle;
            Ok(vec![ProtocolAction::ProposalEnded])
        }

        (Phase::Reviewing { .. }, Control::TransferCancel(_)) => {
            state.phase = Phase::Idle;
            Ok(vec![ProtocolAction::ProposalEnded])
        }

        (
            Phase::Sending {
                files,
                index,
                started: false,
                ..
            },
            Control::Ready,
        ) => {
            state.phase = Phase::Sending {
                files,
                index,
                started: true,
                awaiting_result: false,
            };
            Ok(Vec::new())
        }

        (
            Phase::Sending {
                files,
                index,
                awaiting_result: true,
                ..
            },
            Control::FileResult(result),
        ) => {
            if result.index != index {
                return Err(ProtocolError::InvalidSequence);
            }
            match result.status {
                FileStatus::Verified => {
                    if index + 1 < files {
                        state.phase = Phase::Sending {
                            files,
                            index: index + 1,
                            started: true,
                            awaiting_result: false,
                        };
                        Ok(Vec::new())
                    } else {
                        state.phase = Phase::Idle;
                        Ok(vec![ProtocolAction::TransferFinished])
                    }
                }
                FileStatus::Failed => close_session(state),
            }
        }

        (Phase::Sending { started: false, .. }, Control::TransferCancel(_)) => {
            state.phase = Phase::Idle;
            Ok(vec![ProtocolAction::ProposalEnded])
        }

        (Phase::Sending { .. }, Control::TransferCancel(_)) => close_session(state),

        (Phase::Receiving { .. }, Control::TransferCancel(_)) => close_session(state),

        (
            Phase::Receiving {
                files,
                index,
                ended: false,
            },
            Control::FileEnd(file_end),
        ) => {
            if file_end.index != index {
                return Err(ProtocolError::InvalidSequence);
            }
            state.phase = Phase::Receiving {
                files,
                index,
                ended: true,
            };
            Ok(Vec::new())
        }

        (
            Phase::Idle
            | Phase::AwaitingResponse { .. }
            | Phase::Reviewing { .. }
            | Phase::Sending { .. }
            | Phase::Receiving { .. },
            Control::SessionClose(_),
        ) => close_session(state),

        _ => Err(ProtocolError::WrongState),
    }
}

fn accept_data(
    state: &mut ProtocolState,
    data: Bytes,
) -> Result<Vec<ProtocolAction>, ProtocolError> {
    match state.phase {
        Phase::Receiving { ended: false, .. } => Ok(vec![ProtocolAction::DeliverData(data)]),
        _ => Err(ProtocolError::DataNotAllowed),
    }
}

/// Applies the pairing decision: acceptance starts the record exchange and
/// rejection ends pairing without authorization.
fn finish_pair_response(
    state: &mut ProtocolState,
    response: &PairResponse,
) -> Result<Vec<ProtocolAction>, ProtocolError> {
    if response.accepted {
        state.phase = Phase::Records { records: 0 };
        Ok(Vec::new())
    } else {
        state.phase = Phase::Closing;
        Ok(vec![ProtocolAction::PairingClosed])
    }
}

/// Applies the next record count; the fourth record authorizes the session.
///
/// The responder must flush its queued confirmation before treating the
/// session as authorized; the transport owns that guarantee.
fn advance_pairing(
    state: &mut ProtocolState,
    next: u8,
) -> Result<Vec<ProtocolAction>, ProtocolError> {
    if next < RECORD_COUNT {
        state.phase = Phase::Records { records: next };
        Ok(Vec::new())
    } else {
        state.phase = Phase::Idle;
        Ok(vec![ProtocolAction::Authorized])
    }
}

/// Ends the session: the caller stops any active transfer, cleans partial
/// files, and closes the connection.
fn close_session(state: &mut ProtocolState) -> Result<Vec<ProtocolAction>, ProtocolError> {
    state.phase = Phase::Closing;
    Ok(vec![ProtocolAction::SessionClosed])
}

/// Picks the terminal action from the phase the connection was leaving.
const fn terminal_action(phase: Phase) -> ProtocolAction {
    if phase.is_authorized() {
        ProtocolAction::SessionClosed
    } else {
        ProtocolAction::PairingClosed
    }
}

/// Even records belong to the initiator, odd records to the responder.
const fn is_pairing_mover(role: Role, records: u8) -> bool {
    matches!(
        (records % 2, role),
        (0, Role::Initiator) | (1, Role::Responder)
    )
}

/// The record kind expected at `records`.
const fn expected_step(records: u8) -> PairingStep {
    if records < 2 {
        PairingStep::Share
    } else {
        PairingStep::Confirm
    }
}

/// Manifest entry count of a request, already bounded at decode time.
const fn manifest_files(request: &TransferRequest) -> u16 {
    request.files.len() as u16
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::{
        Inbound, Phase, ProtocolAction, ProtocolError, ProtocolState, Role, accept, send, send_data,
    };
    use crate::protocol::message::{
        CancelCode, CloseCode, Control, ErrorCode, ErrorMessage, FileEnd, FileEntry, FileFailure,
        FileResult, Hello, PairRejection, PairResponse, PairingRecord, PairingStep, SessionClose,
        TransferCancel, TransferRejection, TransferRequest, TransferResponse,
    };
    use crate::protocol::{CONFIRM_BYTES, SHARE_BYTES};

    /// Both peers directly in the authorized idle state.
    fn paired() -> (ProtocolState, ProtocolState) {
        (
            ProtocolState {
                role: Role::Initiator,
                phase: Phase::Idle,
                collision_pending: false,
            },
            ProtocolState {
                role: Role::Responder,
                phase: Phase::Idle,
                collision_pending: false,
            },
        )
    }

    /// Advances both peers through the hello and pair_request exchange.
    fn pair_request_sent(initiator: &mut ProtocolState, responder: &mut ProtocolState) {
        let hello = Control::Hello(Hello::new(None));
        send(initiator, &hello).unwrap();
        accept(responder, Inbound::Control(hello.clone())).unwrap();
        send(responder, &hello).unwrap();
        accept(initiator, Inbound::Control(hello)).unwrap();
        send(initiator, &Control::PairRequest).unwrap();
        accept(responder, Inbound::Control(Control::PairRequest)).unwrap();
    }

    /// Advances a pair into an active outbound transfer from `sender`.
    fn active_transfer(sender: &mut ProtocolState, recipient: &mut ProtocolState) {
        let request = transfer_request();
        send(sender, &request).unwrap();
        accept(recipient, Inbound::Control(request)).unwrap();
        let accepted = Control::TransferResponse(TransferResponse::accepted());
        send(recipient, &accepted).unwrap();
        accept(sender, Inbound::Control(accepted)).unwrap();
        send(recipient, &Control::Ready).unwrap();
        accept(sender, Inbound::Control(Control::Ready)).unwrap();
    }

    fn deliver(
        sender: &mut ProtocolState,
        receiver: &mut ProtocolState,
        control: Control,
    ) -> Result<Vec<ProtocolAction>, ProtocolError> {
        send(sender, &control)?;
        accept(receiver, Inbound::Control(control))
    }

    fn transfer_request() -> Control {
        Control::TransferRequest(
            TransferRequest::new(vec![
                FileEntry {
                    name: "a.bin".to_owned(),
                    size: 5,
                },
                FileEntry {
                    name: "b.txt".to_owned(),
                    size: 0,
                },
            ])
            .unwrap(),
        )
    }

    fn pairing_record(step: PairingStep) -> Control {
        Control::Pairing(PairingRecord::new(
            step,
            match step {
                PairingStep::Share => vec![0; SHARE_BYTES],
                PairingStep::Confirm => vec![0; CONFIRM_BYTES],
            },
        ))
    }

    #[test]
    fn pairing_and_transfers_complete_in_both_directions() {
        let mut initiator = ProtocolState::new(Role::Initiator);
        let mut responder = ProtocolState::new(Role::Responder);
        pair_request_sent(&mut initiator, &mut responder);

        deliver(
            &mut responder,
            &mut initiator,
            Control::PairResponse(PairResponse::accepted()),
        )
        .unwrap();
        assert_eq!(initiator.phase(), &Phase::Records { records: 0 });
        assert_eq!(responder.phase(), &Phase::Records { records: 0 });

        deliver(
            &mut initiator,
            &mut responder,
            pairing_record(PairingStep::Share),
        )
        .unwrap();
        deliver(
            &mut responder,
            &mut initiator,
            pairing_record(PairingStep::Share),
        )
        .unwrap();
        deliver(
            &mut initiator,
            &mut responder,
            pairing_record(PairingStep::Confirm),
        )
        .unwrap();
        let sent = send(&mut responder, &pairing_record(PairingStep::Confirm)).unwrap();
        let received = accept(
            &mut initiator,
            Inbound::Control(pairing_record(PairingStep::Confirm)),
        )
        .unwrap();
        assert_eq!(sent, vec![ProtocolAction::Authorized]);
        assert_eq!(received, vec![ProtocolAction::Authorized]);
        assert_eq!(initiator.phase(), &Phase::Idle);
        assert_eq!(responder.phase(), &Phase::Idle);
        assert!(initiator.is_authorized());

        // One approved transfer from the initiator.
        deliver(&mut initiator, &mut responder, transfer_request()).unwrap();
        assert_eq!(initiator.phase(), &Phase::AwaitingResponse { files: 2 });
        assert_eq!(
            responder.phase(),
            &Phase::Reviewing {
                files: 2,
                accepted: false
            }
        );
        deliver(
            &mut responder,
            &mut initiator,
            Control::TransferResponse(TransferResponse::accepted()),
        )
        .unwrap();
        assert_eq!(
            responder.phase(),
            &Phase::Reviewing {
                files: 2,
                accepted: true
            }
        );
        assert_eq!(
            initiator.phase(),
            &Phase::Sending {
                files: 2,
                index: 0,
                started: false,
                awaiting_result: false
            }
        );
        deliver(&mut responder, &mut initiator, Control::Ready).unwrap();
        assert_eq!(
            responder.phase(),
            &Phase::Receiving {
                files: 2,
                index: 0,
                ended: false
            }
        );
        assert_eq!(
            initiator.phase(),
            &Phase::Sending {
                files: 2,
                index: 0,
                started: true,
                awaiting_result: false
            }
        );

        let digest = [7; 32];
        for index in 0..2u16 {
            send_data(&initiator).unwrap();
            let data = Bytes::from_static(b"payload");
            let actions = accept(&mut responder, Inbound::Data(data.clone())).unwrap();
            assert_eq!(actions, vec![ProtocolAction::DeliverData(data)]);
            deliver(
                &mut initiator,
                &mut responder,
                Control::FileEnd(FileEnd {
                    index,
                    sha256: digest,
                }),
            )
            .unwrap();
            assert_eq!(
                initiator.phase(),
                &Phase::Sending {
                    files: 2,
                    index,
                    started: true,
                    awaiting_result: true
                }
            );
            let actions = deliver(
                &mut responder,
                &mut initiator,
                Control::FileResult(FileResult::verified(index)),
            )
            .unwrap();
            if index == 0 {
                assert!(actions.is_empty());
                assert_eq!(
                    initiator.phase(),
                    &Phase::Sending {
                        files: 2,
                        index: 1,
                        started: true,
                        awaiting_result: false
                    }
                );
                assert_eq!(
                    responder.phase(),
                    &Phase::Receiving {
                        files: 2,
                        index: 1,
                        ended: false
                    }
                );
            } else {
                assert_eq!(actions, vec![ProtocolAction::TransferFinished]);
                assert_eq!(initiator.phase(), &Phase::Idle);
                assert_eq!(responder.phase(), &Phase::Idle);
            }
        }

        // The responder proposes the next transfer on the same session.
        deliver(&mut responder, &mut initiator, transfer_request()).unwrap();
        assert_eq!(responder.phase(), &Phase::AwaitingResponse { files: 2 });
        assert_eq!(
            initiator.phase(),
            &Phase::Reviewing {
                files: 2,
                accepted: false
            }
        );
        deliver(
            &mut initiator,
            &mut responder,
            Control::TransferResponse(TransferResponse::accepted()),
        )
        .unwrap();
        // The recipient readies; the proposer starts sending.
        deliver(&mut initiator, &mut responder, Control::Ready).unwrap();
        assert_eq!(
            initiator.phase(),
            &Phase::Receiving {
                files: 2,
                index: 0,
                ended: false
            }
        );
        assert_eq!(
            responder.phase(),
            &Phase::Sending {
                files: 2,
                index: 0,
                started: true,
                awaiting_result: false
            }
        );
    }

    #[test]
    fn simultaneous_proposals_converge_on_the_initiator_request() {
        let (mut a, mut b) = paired();
        let initiator_request = transfer_request();
        let responder_request = Control::TransferRequest(
            TransferRequest::new(vec![FileEntry {
                name: "c.bin".to_owned(),
                size: 1,
            }])
            .unwrap(),
        );
        send(&mut a, &initiator_request).unwrap();
        send(&mut b, &responder_request).unwrap();

        // The initiator keeps its proposal and answers the responder with busy.
        let actions = accept(&mut a, Inbound::Control(responder_request)).unwrap();
        assert_eq!(
            actions,
            vec![ProtocolAction::Send(Control::TransferResponse(
                TransferResponse::rejected(TransferRejection::Busy)
            ))]
        );
        assert_eq!(a.phase(), &Phase::AwaitingResponse { files: 2 });

        // The responder withdraws its proposal and reviews the initiator's.
        accept(&mut b, Inbound::Control(initiator_request)).unwrap();
        assert_eq!(
            b.phase(),
            &Phase::Reviewing {
                files: 2,
                accepted: false
            }
        );
        assert!(b.collision_pending);

        // The stale busy response for the withdrawn proposal is consumed once.
        let stale = Control::TransferResponse(TransferResponse::rejected(TransferRejection::Busy));
        assert_eq!(
            accept(&mut b, Inbound::Control(stale.clone())).unwrap(),
            Vec::<ProtocolAction>::new()
        );
        assert!(!b.collision_pending);
        assert_eq!(
            b.phase(),
            &Phase::Reviewing {
                files: 2,
                accepted: false
            }
        );
        assert_eq!(
            accept(&mut b, Inbound::Control(stale)),
            Err(ProtocolError::WrongState)
        );

        // The winning transfer proceeds normally back to idle.
        deliver(
            &mut b,
            &mut a,
            Control::TransferResponse(TransferResponse::accepted()),
        )
        .unwrap();
        deliver(&mut b, &mut a, Control::Ready).unwrap();
        for index in 0..2u16 {
            let file_end = Control::FileEnd(FileEnd {
                index,
                sha256: [0; 32],
            });
            send(&mut a, &file_end).unwrap();
            accept(&mut b, Inbound::Control(file_end)).unwrap();
            let actions = deliver(
                &mut b,
                &mut a,
                Control::FileResult(FileResult::verified(index)),
            )
            .unwrap();
            let expected = if index == 0 {
                Vec::new()
            } else {
                vec![ProtocolAction::TransferFinished]
            };
            assert_eq!(actions, expected);
        }
        assert_eq!(a.phase(), &Phase::Idle);
        assert_eq!(b.phase(), &Phase::Idle);
    }

    #[test]
    fn wrong_state_inputs_are_rejected() {
        let mut initiator = ProtocolState::new(Role::Initiator);
        let mut responder = ProtocolState::new(Role::Responder);

        // Hello order is strict on both sides, and hellos happen once.
        assert_eq!(
            accept(
                &mut initiator,
                Inbound::Control(Control::Hello(Hello::new(None)))
            ),
            Err(ProtocolError::WrongState)
        );
        assert_eq!(
            send(&mut responder, &Control::Hello(Hello::new(None))),
            Err(ProtocolError::WrongState)
        );
        send(&mut initiator, &Control::Hello(Hello::new(None))).unwrap();
        assert_eq!(
            send(&mut initiator, &Control::Hello(Hello::new(None))),
            Err(ProtocolError::WrongState)
        );
        accept(
            &mut responder,
            Inbound::Control(Control::Hello(Hello::new(None))),
        )
        .unwrap();
        assert_eq!(
            accept(
                &mut responder,
                Inbound::Control(Control::Hello(Hello::new(None)))
            ),
            Err(ProtocolError::WrongState)
        );
        send(&mut responder, &Control::Hello(Hello::new(None))).unwrap();
        accept(
            &mut initiator,
            Inbound::Control(Control::Hello(Hello::new(None))),
        )
        .unwrap();

        // The request is received once, by the responder only.
        deliver(&mut initiator, &mut responder, Control::PairRequest).unwrap();
        assert_eq!(
            accept(&mut responder, Inbound::Control(Control::PairRequest)),
            Err(ProtocolError::WrongState)
        );
        assert_eq!(
            send(&mut initiator, &Control::PairRequest),
            Err(ProtocolError::WrongState)
        );

        deliver(
            &mut responder,
            &mut initiator,
            Control::PairResponse(PairResponse::accepted()),
        )
        .unwrap();
        // The initiator must send the first record, not receive it.
        assert_eq!(
            accept(
                &mut initiator,
                Inbound::Control(pairing_record(PairingStep::Share))
            ),
            Err(ProtocolError::WrongState)
        );
        // Wrong record step.
        assert_eq!(
            accept(
                &mut responder,
                Inbound::Control(pairing_record(PairingStep::Confirm))
            ),
            Err(ProtocolError::InvalidSequence)
        );
        // No DATA and no session_close before authorization.
        assert_eq!(
            accept(&mut responder, Inbound::Data(Bytes::from_static(b"early"))),
            Err(ProtocolError::DataNotAllowed)
        );
        assert_eq!(
            accept(
                &mut responder,
                Inbound::Control(Control::SessionClose(SessionClose {
                    code: CloseCode::UserClosed
                }))
            ),
            Err(ProtocolError::WrongState)
        );

        deliver(
            &mut initiator,
            &mut responder,
            pairing_record(PairingStep::Share),
        )
        .unwrap();
        deliver(
            &mut responder,
            &mut initiator,
            pairing_record(PairingStep::Share),
        )
        .unwrap();
        deliver(
            &mut initiator,
            &mut responder,
            pairing_record(PairingStep::Confirm),
        )
        .unwrap();
        deliver(
            &mut responder,
            &mut initiator,
            pairing_record(PairingStep::Confirm),
        )
        .unwrap();
        assert_eq!(initiator.phase(), &Phase::Idle);

        // Session gates.
        deliver(&mut initiator, &mut responder, transfer_request()).unwrap();
        assert_eq!(
            send(&mut initiator, &transfer_request()),
            Err(ProtocolError::WrongState)
        );
        // The reviewing recipient never receives a response for its own review.
        assert_eq!(
            accept(
                &mut responder,
                Inbound::Control(Control::TransferResponse(TransferResponse::accepted()))
            ),
            Err(ProtocolError::WrongState)
        );
        assert_eq!(
            accept(&mut responder, Inbound::Control(transfer_request())),
            Err(ProtocolError::WrongState)
        );
        assert_eq!(
            send(&mut responder, &Control::Ready),
            Err(ProtocolError::WrongState)
        );
        assert_eq!(
            accept(&mut initiator, Inbound::Data(Bytes::from_static(b"early"))),
            Err(ProtocolError::DataNotAllowed)
        );
        assert_eq!(
            accept(&mut responder, Inbound::Data(Bytes::from_static(b"early"))),
            Err(ProtocolError::DataNotAllowed)
        );
        assert_eq!(send_data(&initiator), Err(ProtocolError::DataNotAllowed));

        // After acceptance but before ready, DATA stays blocked.
        deliver(
            &mut responder,
            &mut initiator,
            Control::TransferResponse(TransferResponse::accepted()),
        )
        .unwrap();
        assert_eq!(send_data(&initiator), Err(ProtocolError::DataNotAllowed));
        assert_eq!(
            accept(&mut responder, Inbound::Data(Bytes::from_static(b"early"))),
            Err(ProtocolError::DataNotAllowed)
        );
        deliver(&mut responder, &mut initiator, Control::Ready).unwrap();
        assert_eq!(
            accept(&mut initiator, Inbound::Data(Bytes::from_static(b"early"))),
            Err(ProtocolError::DataNotAllowed)
        );

        // Transfer-order gates: wrong index, premature and repeated results.
        assert_eq!(
            send(
                &mut initiator,
                &Control::FileEnd(FileEnd {
                    index: 1,
                    sha256: [0; 32]
                })
            ),
            Err(ProtocolError::InvalidSequence)
        );
        assert_eq!(
            accept(
                &mut responder,
                Inbound::Control(Control::FileResult(FileResult::verified(0)))
            ),
            Err(ProtocolError::WrongState)
        );
        send(
            &mut initiator,
            &Control::FileEnd(FileEnd {
                index: 0,
                sha256: [0; 32],
            }),
        )
        .unwrap();
        assert_eq!(
            send(
                &mut initiator,
                &Control::FileEnd(FileEnd {
                    index: 0,
                    sha256: [0; 32]
                })
            ),
            Err(ProtocolError::WrongState)
        );
        assert_eq!(send_data(&initiator), Err(ProtocolError::DataNotAllowed));
        // A wrong index at the recipient is a sequence violation too.
        assert_eq!(
            accept(
                &mut responder,
                Inbound::Control(Control::FileEnd(FileEnd {
                    index: 1,
                    sha256: [0; 32]
                }))
            ),
            Err(ProtocolError::InvalidSequence)
        );
    }

    #[test]
    fn pre_ready_outcomes_return_to_idle() {
        let (mut a, mut b) = paired();

        // A rejection with any reason returns the requester to idle.
        send(&mut a, &transfer_request()).unwrap();
        let actions = accept(
            &mut a,
            Inbound::Control(Control::TransferResponse(TransferResponse::rejected(
                TransferRejection::Busy,
            ))),
        )
        .unwrap();
        assert_eq!(actions, vec![ProtocolAction::ProposalEnded]);
        assert_eq!(a.phase(), &Phase::Idle);
        assert!(a.is_authorized());

        // Cancellation while reviewing ends only the proposal.
        accept(&mut b, Inbound::Control(transfer_request())).unwrap();
        let actions = accept(
            &mut b,
            Inbound::Control(Control::TransferCancel(TransferCancel {
                code: CancelCode::UserCancelled,
            })),
        )
        .unwrap();
        assert_eq!(actions, vec![ProtocolAction::ProposalEnded]);
        assert_eq!(b.phase(), &Phase::Idle);

        // A local withdrawal while awaiting the response does the same.
        send(&mut a, &transfer_request()).unwrap();
        let actions = send(
            &mut a,
            &Control::TransferCancel(TransferCancel {
                code: CancelCode::SourceUnavailable,
            }),
        )
        .unwrap();
        assert_eq!(actions, vec![ProtocolAction::ProposalEnded]);
        assert_eq!(a.phase(), &Phase::Idle);
        assert!(a.is_authorized());
    }

    #[test]
    fn post_ready_failures_close_the_session() {
        // The recipient cancels after ready.
        let (mut a, mut b) = paired();
        active_transfer(&mut a, &mut b);
        let actions = accept(
            &mut a,
            Inbound::Control(Control::TransferCancel(TransferCancel {
                code: CancelCode::UserCancelled,
            })),
        )
        .unwrap();
        assert_eq!(actions, vec![ProtocolAction::SessionClosed]);
        assert_eq!(a.phase(), &Phase::Closing);

        // The requester cancels after ready.
        let (mut a, mut b) = paired();
        active_transfer(&mut a, &mut b);
        let actions = send(
            &mut a,
            &Control::TransferCancel(TransferCancel {
                code: CancelCode::UserCancelled,
            }),
        )
        .unwrap();
        assert_eq!(actions, vec![ProtocolAction::SessionClosed]);
        assert_eq!(a.phase(), &Phase::Closing);

        // A failed file result closes on both sides.
        let (mut a, mut b) = paired();
        active_transfer(&mut a, &mut b);
        let file_end = Control::FileEnd(FileEnd {
            index: 0,
            sha256: [0; 32],
        });
        send(&mut a, &file_end).unwrap();
        accept(&mut b, Inbound::Control(file_end)).unwrap();
        let actions = send(
            &mut b,
            &Control::FileResult(FileResult::failed(0, FileFailure::HashMismatch)),
        )
        .unwrap();
        assert_eq!(actions, vec![ProtocolAction::SessionClosed]);
        assert_eq!(b.phase(), &Phase::Closing);
        let actions = accept(
            &mut a,
            Inbound::Control(Control::FileResult(FileResult::failed(
                0,
                FileFailure::HashMismatch,
            ))),
        )
        .unwrap();
        assert_eq!(actions, vec![ProtocolAction::SessionClosed]);
        assert_eq!(a.phase(), &Phase::Closing);

        // The recipient cancels an active receive; the sender accepts it.
        let (mut a, mut b) = paired();
        active_transfer(&mut a, &mut b);
        let cancel = Control::TransferCancel(TransferCancel {
            code: CancelCode::UserCancelled,
        });
        let actions = send(&mut b, &cancel).unwrap();
        assert_eq!(actions, vec![ProtocolAction::SessionClosed]);
        assert_eq!(b.phase(), &Phase::Closing);
        let actions = accept(&mut a, Inbound::Control(cancel)).unwrap();
        assert_eq!(actions, vec![ProtocolAction::SessionClosed]);
        assert_eq!(a.phase(), &Phase::Closing);
    }

    #[test]
    fn pairing_rejection_and_peer_errors_terminate() {
        // Rejection closes pairing on both sides without authorization.
        let mut initiator = ProtocolState::new(Role::Initiator);
        let mut responder = ProtocolState::new(Role::Responder);
        pair_request_sent(&mut initiator, &mut responder);
        let rejected = Control::PairResponse(PairResponse::rejected(PairRejection::UserRejected));
        let actions = send(&mut responder, &rejected).unwrap();
        assert_eq!(actions, vec![ProtocolAction::PairingClosed]);
        assert_eq!(responder.phase(), &Phase::Closing);
        let actions = accept(&mut initiator, Inbound::Control(rejected)).unwrap();
        assert_eq!(actions, vec![ProtocolAction::PairingClosed]);
        assert_eq!(initiator.phase(), &Phase::Closing);

        // A peer error during pairing is terminal and is never answered.
        let mut initiator = ProtocolState::new(Role::Initiator);
        let mut responder = ProtocolState::new(Role::Responder);
        pair_request_sent(&mut initiator, &mut responder);
        deliver(
            &mut responder,
            &mut initiator,
            Control::PairResponse(PairResponse::accepted()),
        )
        .unwrap();
        let actions = accept(
            &mut initiator,
            Inbound::Control(Control::Error(ErrorMessage {
                code: ErrorCode::InvalidMessage,
            })),
        )
        .unwrap();
        assert_eq!(actions, vec![ProtocolAction::PairingClosed]);
        assert_eq!(initiator.phase(), &Phase::Closing);
        assert_eq!(
            send(
                &mut initiator,
                &Control::Error(ErrorMessage {
                    code: ErrorCode::InternalError
                })
            ),
            Err(ProtocolError::WrongState)
        );

        // Local and peer errors in an authorized session close it.
        let (mut a, _) = paired();
        let actions = send(
            &mut a,
            &Control::Error(ErrorMessage {
                code: ErrorCode::InternalError,
            }),
        )
        .unwrap();
        assert_eq!(actions, vec![ProtocolAction::SessionClosed]);
        assert_eq!(a.phase(), &Phase::Closing);

        let (mut a, _) = paired();
        let actions = accept(
            &mut a,
            Inbound::Control(Control::Error(ErrorMessage {
                code: ErrorCode::Timeout,
            })),
        )
        .unwrap();
        assert_eq!(actions, vec![ProtocolAction::SessionClosed]);
        assert_eq!(a.phase(), &Phase::Closing);
    }

    #[test]
    fn session_close_is_terminal_from_both_sides() {
        let (mut a, mut b) = paired();

        let actions = accept(
            &mut a,
            Inbound::Control(Control::SessionClose(SessionClose {
                code: CloseCode::UserClosed,
            })),
        )
        .unwrap();
        assert_eq!(actions, vec![ProtocolAction::SessionClosed]);
        assert_eq!(a.phase(), &Phase::Closing);
        // Closing rejects all further traffic.
        assert_eq!(
            accept(&mut a, Inbound::Control(Control::Ready)),
            Err(ProtocolError::WrongState)
        );
        assert_eq!(
            accept(&mut a, Inbound::Data(Bytes::from_static(b"x"))),
            Err(ProtocolError::DataNotAllowed)
        );

        let actions = send(
            &mut b,
            &Control::SessionClose(SessionClose {
                code: CloseCode::IdleTimeout,
            }),
        )
        .unwrap();
        assert_eq!(actions, vec![ProtocolAction::SessionClosed]);
        assert_eq!(b.phase(), &Phase::Closing);
    }
}
