//! Authorized session ownership: pairing state, transfer policy, idle timer.
//!
//! One task owns each connection and its mutable session state. After the
//! `session_idle` state is reached, the reusable transfer loop runs here.
//!
//! This module owns the whole connection lifetime: TCP/TLS setup, the `hello`
//! and `pair_request` exchange, the one-time code, the four SPAKE2 records,
//! the authorized idle session, and its close paths. Manual close, peer
//! `session_close`, the fixed 600-second idle deadline, bounded transfer
//! progress deadlines, and app shutdown all end the connection with a
//! best-effort close reason.
//!
//! The manager owns at most one connection at a time, so an extra inbound
//! socket is refused with `pair_response(busy)` and never enters the
//! application event loop. Connection tasks report terminal outcomes through
//! `AppEvent` and only touch application state through those events.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use rand_core::{OsRng, UnwrapErr};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout_at};

use crate::app::action::{ConnectionTarget, PairingPeer};
use crate::app::event::AppEvent;
use crate::app::model::{TransferProgress, TransferProposal};
use crate::app::runtime::EventSender;
use crate::framing::Frame;
use crate::pairing::{self, PairingCode};
use crate::protocol::{
    self, CancelCode, CloseCode, Control, ErrorCode, ErrorMessage, FileEnd, FileResult, Hello,
    PairRejection, PairResponse, PairingRecord, PairingStep, Phase, ProtocolAction, ProtocolState,
    Role, SessionClose, TransferCancel, TransferRejection, TransferRequest, TransferResponse,
};
use crate::storage::{Destination, StorageError};
use crate::transfer::engine::{self, IncomingFile};
use crate::transfer::selection::{FileSelection, SelectedFile};
use crate::transport::{self, FramedConnection, Outbound, split_frame_io};

/// Capacity of the effect-to-session command queue.
const COMMAND_CHANNEL_CAPACITY: usize = 8;
/// Capacity of the accepted-socket queue between the listener and the owner.
const ACCEPTED_CHANNEL_CAPACITY: usize = 4;
/// Capacity of one connection's private command queue.
const CONNECTION_COMMAND_CAPACITY: usize = 4;
/// Capacity of the connection-ended notice queue.
const NOTICE_CHANNEL_CAPACITY: usize = 4;
/// Time allowed for the manager to stop before it is aborted.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
/// Time allowed for an active connection to send `session_close` on shutdown.
const CONNECTION_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// Local deadlines for one pairing attempt and the authorized session.
///
/// The prompt and code defaults match `docs/PROTOCOL.md`: the prompt and the
/// code live for 120 monotonic seconds. The idle default is the fixed
/// 600-second session maximum; tests override it to run quickly. The progress
/// deadline bounds one active transfer that stops making progress and is local
/// policy, not a wire constant.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SessionTimeouts {
    /// TCP and TLS setup for one connection.
    pub(crate) handshake: Duration,
    /// One control message during the initial hello exchange.
    pub(crate) control: Duration,
    /// The responder's prompt decision.
    pub(crate) prompt: Duration,
    /// The code lifetime from acceptance through mutual confirmation.
    pub(crate) code: Duration,
    /// Maximum idle time without a proposal or active transfer.
    pub(crate) idle: Duration,
    /// Maximum time an active transfer may make no progress.
    pub(crate) progress: Duration,
}

impl Default for SessionTimeouts {
    fn default() -> Self {
        Self {
            handshake: Duration::from_secs(10),
            control: Duration::from_secs(15),
            prompt: Duration::from_secs(120),
            code: Duration::from_secs(120),
            idle: Duration::from_secs(600),
            progress: Duration::from_secs(60),
        }
    }
}

/// Commands sent from the effect dispatcher to the session owner.
pub(crate) enum SessionCommand {
    Connect(ConnectionTarget),
    AcceptPairing,
    RejectPairing,
    RejectPairingBusy,
    SubmitPairingCode(PairingCode),
    StartTransfer(FileSelection),
    AcceptTransfer(PathBuf),
    RejectTransfer,
    Disconnect,
}

/// Creates the bounded accepted-socket channel.
pub(crate) fn accepted_channel() -> (mpsc::Sender<TcpStream>, mpsc::Receiver<TcpStream>) {
    mpsc::channel(ACCEPTED_CHANNEL_CAPACITY)
}

/// Owns the session manager task and its command channel.
pub(crate) struct SessionService {
    commands: mpsc::Sender<SessionCommand>,
    task: JoinHandle<()>,
}

impl SessionService {
    /// Starts the session owner with the production deadlines.
    pub(crate) fn start(events: EventSender, accepted: mpsc::Receiver<TcpStream>) -> Self {
        Self::with_timeouts(events, accepted, SessionTimeouts::default())
    }

    /// Starts the session owner with explicit deadlines, for tests.
    fn with_timeouts(
        events: EventSender,
        accepted: mpsc::Receiver<TcpStream>,
        timeouts: SessionTimeouts,
    ) -> Self {
        let (commands, receiver) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        let task = tokio::spawn(manager_loop(events, accepted, receiver, timeouts));
        Self { commands, task }
    }

    /// Sends one command, failing once the owner has stopped.
    pub(crate) async fn send(&self, command: SessionCommand) -> anyhow::Result<()> {
        self.commands
            .send(command)
            .await
            .map_err(|_| anyhow::anyhow!("session owner stopped"))
    }

    /// Stops the owner and lets it close any active connection.
    pub(crate) async fn stop(self) {
        let Self { commands, mut task } = self;
        // Closing the command channel makes the manager take its cleanup
        // path, which asks the active connection to report
        // `session_close(shutdown)` before its transport closes.
        drop(commands);
        if tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

/// One active connection and its private command channel.
struct ActiveConnection {
    commands: mpsc::Sender<ConnectionCommand>,
    task: JoinHandle<()>,
}

/// Commands understood by one connection task.
enum ConnectionCommand {
    Decide(bool),
    RejectBusy,
    SubmitCode(PairingCode),
    StartTransfer(FileSelection),
    AcceptTransfer(PathBuf),
    RejectTransfer,
    /// Ends the connection, sending `session_close(code)` when authorized.
    Close(CloseCode),
}

/// Why a connection task stopped.
enum ConnectionNotice {
    Ended,
}

/// Terminal outcome of one pairing attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowOutcome {
    /// The connection ended before authorization (rejection, timeout, close).
    Ended,
    /// A protocol, transport, or authentication failure.
    Failed,
    /// An authorized session ended.
    SessionEnded,
}

/// Successful work is expressed with `T`; the error is a terminal outcome.
type FlowResult<T> = Result<T, FlowOutcome>;

/// Owns the single active connection, routes commands to it, and starts a
/// held connect request once the previous connection's notice arrives.
async fn manager_loop(
    events: EventSender,
    mut accepted: mpsc::Receiver<TcpStream>,
    mut commands: mpsc::Receiver<SessionCommand>,
    timeouts: SessionTimeouts,
) {
    let (notice_sender, mut notices) = mpsc::channel(NOTICE_CHANNEL_CAPACITY);
    let mut active: Option<ActiveConnection> = None;
    // A connect request that arrived before the previous connection's end
    // notice was processed; it starts as soon as the notice clears `active`.
    let mut pending_connect: Option<ConnectionTarget> = None;
    // At most one extra inbound socket is refused with a busy response.
    let mut busy: Option<JoinHandle<()>> = None;
    // The listener may stop accepting before the app shuts down; an exhausted
    // accepted channel only disables the inbound arm.
    let mut accepting = true;

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                match command {
                    SessionCommand::Connect(target) if active.is_none() => {
                        active = Some(spawn_initiator(target, &events, &notice_sender, timeouts));
                    }
                    // The previous connection's terminal notice has not been
                    // processed yet; hold the request instead of dropping it.
                    SessionCommand::Connect(target) => {
                        pending_connect = Some(target);
                    }
                    command => {
                        if let Some(active) = active.as_mut()
                            && let Some(command) = connection_command(command)
                        {
                            let _ = active.commands.send(command).await;
                        }
                    }
                }
            }
            incoming = accepted.recv(), if accepting => {
                match incoming {
                    None => accepting = false,
                    Some(stream) => {
                        // Exactly one connection is owned at a time. One extra
                        // socket is refused with `pair_response(busy)` without
                        // a prompt; any further socket is closed.
                        if active.is_none() {
                            if let Ok(address) = stream.peer_addr() {
                                active = Some(spawn_responder(
                                    stream,
                                    address,
                                    &events,
                                    &notice_sender,
                                    timeouts,
                                ));
                            }
                        } else if busy
                            .as_ref()
                            .is_none_or(|task| task.is_finished())
                        {
                            busy = Some(tokio::spawn(busy_flow(stream, timeouts)));
                        }
                    }
                }
            }
            notice = notices.recv() => {
                if matches!(notice, Some(ConnectionNotice::Ended)) {
                    active = None;
                    // Start a connect that could not start while the previous
                    // connection was winding down.
                    if let Some(target) = pending_connect.take() {
                        active = Some(spawn_initiator(target, &events, &notice_sender, timeouts));
                    }
                }
            }
        }
    }

    // Shutdown asks the active connection to report `session_close(shutdown)`
    // before its transport closes, then waits briefly before aborting it.
    if let Some(mut active) = active.take() {
        let cleanup = async {
            let _ = active
                .commands
                .send(ConnectionCommand::Close(CloseCode::Shutdown))
                .await;
            let _ = (&mut active.task).await;
        };
        if tokio::time::timeout(CONNECTION_CLOSE_TIMEOUT, cleanup)
            .await
            .is_err()
        {
            active.task.abort();
            let _ = active.task.await;
        }
    }
    if let Some(busy) = busy.take() {
        busy.abort();
        let _ = busy.await;
    }
}

/// Converts an application command into a connection command when relevant.
fn connection_command(command: SessionCommand) -> Option<ConnectionCommand> {
    match command {
        SessionCommand::AcceptPairing => Some(ConnectionCommand::Decide(true)),
        SessionCommand::RejectPairing => Some(ConnectionCommand::Decide(false)),
        SessionCommand::RejectPairingBusy => Some(ConnectionCommand::RejectBusy),
        SessionCommand::SubmitPairingCode(code) => Some(ConnectionCommand::SubmitCode(code)),
        SessionCommand::StartTransfer(selection) => {
            Some(ConnectionCommand::StartTransfer(selection))
        }
        SessionCommand::AcceptTransfer(destination) => {
            Some(ConnectionCommand::AcceptTransfer(destination))
        }
        SessionCommand::RejectTransfer => Some(ConnectionCommand::RejectTransfer),
        SessionCommand::Disconnect => Some(ConnectionCommand::Close(CloseCode::UserClosed)),
        // The manager handles `Connect` before this routing step.
        SessionCommand::Connect(_) => None,
    }
}

/// Spawns the initiator half of one pairing attempt.
fn spawn_initiator(
    target: ConnectionTarget,
    events: &EventSender,
    notices: &mpsc::Sender<ConnectionNotice>,
    timeouts: SessionTimeouts,
) -> ActiveConnection {
    let (commands, receiver) = mpsc::channel(CONNECTION_COMMAND_CAPACITY);
    let task = tokio::spawn(run_initiator(
        target,
        events.clone(),
        receiver,
        notices.clone(),
        timeouts,
    ));
    ActiveConnection { commands, task }
}

/// Spawns the responder half of one inbound connection.
fn spawn_responder(
    stream: TcpStream,
    address: SocketAddr,
    events: &EventSender,
    notices: &mpsc::Sender<ConnectionNotice>,
    timeouts: SessionTimeouts,
) -> ActiveConnection {
    let (commands, receiver) = mpsc::channel(CONNECTION_COMMAND_CAPACITY);
    let task = tokio::spawn(run_responder(
        stream,
        address,
        events.clone(),
        receiver,
        notices.clone(),
        timeouts,
    ));
    ActiveConnection { commands, task }
}

/// Runs the initiator flow and reports its terminal outcome.
async fn run_initiator(
    target: ConnectionTarget,
    events: EventSender,
    commands: mpsc::Receiver<ConnectionCommand>,
    notices: mpsc::Sender<ConnectionNotice>,
    timeouts: SessionTimeouts,
) {
    let outcome = initiator_flow(&target, &events, commands, timeouts).await;
    emit_outcome(&events, outcome).await;
    let _ = notices.send(ConnectionNotice::Ended).await;
}

/// Runs the responder flow and reports its terminal outcome.
async fn run_responder(
    stream: TcpStream,
    address: SocketAddr,
    events: EventSender,
    commands: mpsc::Receiver<ConnectionCommand>,
    notices: mpsc::Sender<ConnectionNotice>,
    timeouts: SessionTimeouts,
) {
    let outcome = responder_flow(stream, address, &events, commands, timeouts).await;
    emit_outcome(&events, outcome).await;
    let _ = notices.send(ConnectionNotice::Ended).await;
}

/// Publishes the terminal event for a connection.
///
/// Connection-scoped failures return the app to browsing so a later attempt
/// can start a fresh pairing (`docs/ERROR_HANDLING.md`); only an authorized
/// session ending is reported as a session close.
async fn emit_outcome(events: &EventSender, outcome: FlowOutcome) {
    let event = match outcome {
        FlowOutcome::Ended | FlowOutcome::Failed => AppEvent::PairingEnded,
        FlowOutcome::SessionEnded => AppEvent::SessionClosed,
    };
    let _ = events.send(event).await;
}

/// One live TLS connection plus its validated protocol state.
struct SessionConnection {
    connection: FramedConnection<tokio_rustls::TlsStream<TcpStream>>,
    outbound: Outbound,
    protocol: ProtocolState,
    /// The peer's untrusted `hello` display name, shown on transfer review.
    peer_name: Option<String>,
    /// Local close reason to report to the peer when this side ends an
    /// authorized session; `None` when the peer closed or the session failed.
    close_code: Option<CloseCode>,
}

impl SessionConnection {
    /// Wraps a completed TLS connection and starts the ordered writer.
    fn new(stream: tokio_rustls::TlsStream<TcpStream>, role: Role) -> Self {
        let (connection, outbound) = split_frame_io(stream);
        Self {
            connection,
            outbound,
            protocol: ProtocolState::new(role),
            peer_name: None,
            close_code: None,
        }
    }

    /// Validates and queues one control, returning its protocol actions.
    async fn send_control(&mut self, control: &Control) -> FlowResult<Vec<ProtocolAction>> {
        let actions =
            protocol::send(&mut self.protocol, control).map_err(|_| FlowOutcome::Failed)?;
        self.outbound
            .send_control(control)
            .await
            .map_err(|_| FlowOutcome::Failed)?;
        Ok(actions)
    }

    /// Validates and queues one control, waiting until it is flushed.
    async fn send_control_flushed(&mut self, control: &Control) -> FlowResult<Vec<ProtocolAction>> {
        let actions =
            protocol::send(&mut self.protocol, control).map_err(|_| FlowOutcome::Failed)?;
        self.outbound
            .send_control_flushed(control)
            .await
            .map_err(|_| FlowOutcome::Failed)?;
        Ok(actions)
    }

    /// Flushes queued frames, reports the local close reason when the session
    /// is still authorized, and shuts the write half down cleanly.
    ///
    /// The peer observes every queued frame followed by one clean close, so a
    /// rejection response can never be truncated by an abrupt drop. A
    /// `session_close` is not acknowledged.
    async fn close(&mut self) {
        if let Some(code) = self.close_code.take()
            && self.protocol.is_authorized()
        {
            let _ = self
                .send_control_flushed(&Control::SessionClose(SessionClose { code }))
                .await;
        }
        self.outbound.close().await;
    }

    /// Reads and validates one inbound frame.
    ///
    /// `Ok(None)` means the peer closed cleanly; `Err` is a terminal outcome.
    /// A control the protocol layer treats as terminal (a peer `error`, a
    /// rejection, or `session_close`) ends the connection immediately instead
    /// of waiting for the peer to close.
    async fn read(&mut self) -> FlowResult<Option<InboundMessage>> {
        match self
            .connection
            .read_frame()
            .await
            .map_err(|_| FlowOutcome::Failed)?
        {
            None => Ok(None),
            Some(Frame::Control(body)) => {
                let control = Control::decode(&body).map_err(|_| FlowOutcome::Failed)?;
                let actions = protocol::accept(
                    &mut self.protocol,
                    protocol::Inbound::Control(control.clone()),
                )
                .map_err(|_| FlowOutcome::Failed)?;
                if actions.contains(&ProtocolAction::PairingClosed) {
                    return Err(FlowOutcome::Ended);
                }
                if actions.contains(&ProtocolAction::SessionClosed) {
                    return Err(FlowOutcome::SessionEnded);
                }
                Ok(Some(InboundMessage::Control(InboundControl {
                    control,
                    body,
                    actions,
                })))
            }
            Some(Frame::Data(body)) => {
                protocol::accept(&mut self.protocol, protocol::Inbound::Data(body.clone()))
                    .map_err(|_| FlowOutcome::Failed)?;
                Ok(Some(InboundMessage::Data(body)))
            }
        }
    }
}

/// A validated inbound frame: a control with its actions, or file bytes.
enum InboundMessage {
    Control(InboundControl),
    Data(Bytes),
}

/// A validated inbound control with its exact JSON body.
struct InboundControl {
    control: Control,
    body: Bytes,
    /// Protocol work the control produced, such as a busy response.
    actions: Vec<ProtocolAction>,
}

/// One stage result: a control, a command, a close, or the deadline.
enum Stage {
    Control(InboundControl),
    Command(ConnectionCommand),
    /// The peer or the command channel closed.
    Finished,
    /// The stage deadline elapsed.
    TimedOut,
}

/// Waits for the next control, a command, the peer close, or the deadline.
async fn wait_stage(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    deadline: Instant,
) -> FlowResult<Stage> {
    tokio::select! {
        biased;
        command = commands.recv() => Ok(match command {
            Some(command) => Stage::Command(command),
            None => Stage::Finished,
        }),
        result = timeout_at(deadline, connection.read()) => match result {
            Err(_) => Ok(Stage::TimedOut),
            Ok(Ok(Some(InboundMessage::Control(inbound)))) => Ok(Stage::Control(inbound)),
            // DATA is rejected by the protocol state in every stage that uses
            // this wait, so it can only mean a peer bug.
            Ok(Ok(Some(InboundMessage::Data(_)))) => Err(FlowOutcome::Failed),
            Ok(Ok(None)) => Ok(Stage::Finished),
            Ok(Err(outcome)) => Err(outcome),
        },
    }
}

/// Waits for the next control, ignoring unrelated commands until `deadline`.
async fn read_control(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    deadline: Instant,
) -> FlowResult<InboundControl> {
    loop {
        match wait_stage(connection, commands, deadline).await? {
            Stage::Control(inbound) => return Ok(inbound),
            Stage::Finished | Stage::TimedOut | Stage::Command(ConnectionCommand::Close(_)) => {
                return Err(FlowOutcome::Ended);
            }
            Stage::Command(_) => continue,
        }
    }
}

/// Waits for the next validated control until `deadline` without commands.
///
/// The busy-rejection flow owns no command channel, so it uses this narrower
/// wait instead of [`read_control`].
async fn read_control_only(
    connection: &mut SessionConnection,
    deadline: Instant,
) -> FlowResult<InboundControl> {
    match timeout_at(deadline, connection.read()).await {
        Err(_) => Err(FlowOutcome::Ended),
        Ok(Ok(Some(InboundMessage::Control(inbound)))) => Ok(inbound),
        Ok(Ok(Some(InboundMessage::Data(_)))) => Err(FlowOutcome::Failed),
        Ok(Ok(None)) => Err(FlowOutcome::Ended),
        Ok(Err(outcome)) => Err(outcome),
    }
}

/// The responder's answer to an inbound pairing request.
enum Decision {
    Accept,
    Reject,
    Busy,
    Timeout,
    Close,
}

/// Waits for the local user's pairing decision.
async fn wait_decision(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    deadline: Instant,
) -> FlowResult<Decision> {
    loop {
        match wait_stage(connection, commands, deadline).await? {
            Stage::Command(ConnectionCommand::Decide(true)) => return Ok(Decision::Accept),
            Stage::Command(ConnectionCommand::Decide(false)) => return Ok(Decision::Reject),
            Stage::Command(ConnectionCommand::RejectBusy) => return Ok(Decision::Busy),
            Stage::Command(ConnectionCommand::Close(_)) | Stage::Finished => {
                return Ok(Decision::Close);
            }
            Stage::TimedOut => return Ok(Decision::Timeout),
            // An unexpected control is validated by the protocol state and
            // would have failed already; commands that do not belong here are
            // ignored until the deadline.
            Stage::Control(_) | Stage::Command(_) => continue,
        }
    }
}

/// Waits for the locally entered code until `deadline`.
async fn wait_code(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    deadline: Instant,
) -> FlowResult<PairingCode> {
    loop {
        match wait_stage(connection, commands, deadline).await? {
            Stage::Command(ConnectionCommand::SubmitCode(code)) => return Ok(code),
            Stage::Finished | Stage::TimedOut | Stage::Command(ConnectionCommand::Close(_)) => {
                return Err(FlowOutcome::Ended);
            }
            Stage::Control(_) | Stage::Command(_) => continue,
        }
    }
}

/// Resolves the selected route to one connectable socket address.
async fn resolve_target(target: &ConnectionTarget, timeout: Duration) -> FlowResult<SocketAddr> {
    match target {
        ConnectionTarget::Discovered { address, .. } => Ok(*address),
        ConnectionTarget::Direct(endpoint) => {
            let lookup = tokio::net::lookup_host((endpoint.host(), endpoint.port()));
            match tokio::time::timeout(timeout, lookup).await {
                Ok(Ok(mut addresses)) => addresses.next().ok_or(FlowOutcome::Failed),
                _ => Err(FlowOutcome::Failed),
            }
        }
    }
}

/// The initiator: connect, request pairing, enter the code, confirm.
async fn initiator_flow(
    target: &ConnectionTarget,
    events: &EventSender,
    commands: mpsc::Receiver<ConnectionCommand>,
    timeouts: SessionTimeouts,
) -> FlowOutcome {
    let address = match resolve_target(target, timeouts.control).await {
        Ok(address) => address,
        Err(outcome) => return outcome,
    };
    let (stream, exporter) = match transport::connect(address, timeouts.handshake).await {
        Ok(handshake) => handshake,
        Err(_) => return FlowOutcome::Failed,
    };
    let mut connection = SessionConnection::new(stream, Role::Initiator);
    let outcome = initiator_pairing(&mut connection, &*exporter, events, commands, timeouts).await;
    // Flush queued frames and close cleanly on every exit path.
    connection.close().await;
    outcome
}

/// Builds the local `hello` with the computer name as display text.
///
/// The name is untrusted display text to the peer, exactly like any other
/// `display_name`; it is only presented as a hint until pairing confirms the
/// live connection.
fn local_hello() -> Control {
    Control::Hello(Hello::new(Some(crate::hostname::local_hostname())))
}

/// Runs the initiator pairing stages over an established connection.
async fn initiator_pairing(
    connection: &mut SessionConnection,
    exporter: &[u8],
    events: &EventSender,
    mut commands: mpsc::Receiver<ConnectionCommand>,
    timeouts: SessionTimeouts,
) -> FlowOutcome {
    // The initiator sends the first hello and retains the exact JSON body.
    let hello = local_hello();
    let initiator_hello = Bytes::from(hello.encode());
    if let Err(outcome) = connection.send_control(&hello).await {
        return outcome;
    }
    let inbound =
        match read_control(connection, &mut commands, Instant::now() + timeouts.control).await {
            Ok(inbound) => inbound,
            Err(outcome) => return outcome,
        };
    let responder_hello = match inbound.control {
        Control::Hello(hello) => {
            connection.peer_name = hello.display_name;
            inbound.body
        }
        _ => return FlowOutcome::Failed,
    };

    // Ask for the prompt; the responder may use its full decision deadline.
    if let Err(outcome) = connection.send_control(&Control::PairRequest).await {
        return outcome;
    }
    let response_deadline = Instant::now() + timeouts.prompt + timeouts.control;
    let accepted = match read_pair_response(connection, &mut commands, response_deadline).await {
        Ok(accepted) => accepted,
        Err(outcome) => return outcome,
    };
    if !accepted {
        return FlowOutcome::Ended;
    }
    let _ = events.send(AppEvent::PairingAccepted).await;

    // Code entry and the record exchange share one 120-second monotonic
    // deadline, which started when the responder accepted the request.
    let code_deadline = Instant::now() + timeouts.code;
    let code = match wait_code(connection, &mut commands, code_deadline).await {
        Ok(code) => code,
        Err(outcome) => return outcome,
    };

    let mut rng = UnwrapErr(OsRng);
    let binding = match pairing::binding(exporter, &initiator_hello, &responder_hello) {
        Ok(binding) => binding,
        Err(_) => return FlowOutcome::Failed,
    };
    let (share, state) = match pairing::start_initiator(&code, &binding, &mut rng) {
        Ok(pair) => pair,
        Err(_) => return FlowOutcome::Failed,
    };

    // Record 1: the initiator's SPAKE2 share.
    if let Err(outcome) = connection
        .send_control(&Control::Pairing(PairingRecord::new(
            PairingStep::Share,
            share,
        )))
        .await
    {
        return outcome;
    }

    // Record 2: the responder's share completes the key schedule.
    let inbound =
        match read_pairing(connection, &mut commands, code_deadline, PairingStep::Share).await {
            Ok(inbound) => inbound,
            Err(outcome) => return outcome,
        };
    let responder_share = match inbound.control {
        Control::Pairing(record) => record.data,
        _ => return FlowOutcome::Failed,
    };
    let output = match pairing::finish_initiator(state, &responder_share) {
        Ok(output) => output,
        Err(_) => return FlowOutcome::Failed,
    };
    let tag = match pairing::confirmation(&output) {
        Ok(tag) => tag,
        Err(_) => return FlowOutcome::Failed,
    };

    // Record 3: the initiator's confirmation must be fully written and
    // flushed before this side accepts the peer's confirmation.
    if let Err(outcome) = connection
        .send_control_flushed(&Control::Pairing(PairingRecord::new(
            PairingStep::Confirm,
            tag.to_vec(),
        )))
        .await
    {
        return outcome;
    }

    // Record 4: verify the responder's confirmation.
    let inbound = match read_pairing(
        connection,
        &mut commands,
        code_deadline,
        PairingStep::Confirm,
    )
    .await
    {
        Ok(inbound) => inbound,
        Err(outcome) => return outcome,
    };
    let responder_tag = match inbound.control {
        Control::Pairing(record) => record.data,
        _ => return FlowOutcome::Failed,
    };
    if pairing::verify(&output, &responder_tag).is_err() {
        return FlowOutcome::Failed;
    }

    // The code is consumed and the session becomes authorized.
    drop(code);
    let _ = events.send(AppEvent::PairingSucceeded).await;
    idle_authorized(connection, &mut commands, events, timeouts).await
}

/// Reads the responder's `pair_response`, returning whether it accepted.
async fn read_pair_response(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    deadline: Instant,
) -> FlowResult<bool> {
    let inbound = read_control(connection, commands, deadline).await?;
    match inbound.control {
        Control::PairResponse(response) => Ok(response.accepted),
        _ => Err(FlowOutcome::Failed),
    }
}

/// Reads one pairing record of the expected step until `deadline`.
async fn read_pairing(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    deadline: Instant,
    step: PairingStep,
) -> FlowResult<InboundControl> {
    let inbound = read_control(connection, commands, deadline).await?;
    match &inbound.control {
        Control::Pairing(record) if record.step == step => Ok(inbound),
        _ => Err(FlowOutcome::Failed),
    }
}

/// The responder: prompt, accept, display the code, and confirm.
async fn responder_flow(
    stream: TcpStream,
    address: SocketAddr,
    events: &EventSender,
    commands: mpsc::Receiver<ConnectionCommand>,
    timeouts: SessionTimeouts,
) -> FlowOutcome {
    let (stream, exporter) = match transport::accept_stream(stream, timeouts.handshake).await {
        Ok(handshake) => handshake,
        Err(_) => return FlowOutcome::Failed,
    };
    let mut connection = SessionConnection::new(stream, Role::Responder);
    let outcome = responder_pairing(
        &mut connection,
        &*exporter,
        address,
        events,
        commands,
        timeouts,
    )
    .await;
    // Flush queued frames and close cleanly on every exit path.
    connection.close().await;
    outcome
}

/// Refuses one extra inbound connection with `pair_response(busy)`.
///
/// The local user never sees a second prompt and no pairing code is created;
/// the socket is dropped when the peer does not complete the hello exchange.
async fn busy_flow(stream: TcpStream, timeouts: SessionTimeouts) {
    let Ok((stream, _exporter)) = transport::accept_stream(stream, timeouts.handshake).await else {
        return;
    };
    let mut connection = SessionConnection::new(stream, Role::Responder);
    let _ = busy_pairing(&mut connection, timeouts).await;
    connection.close().await;
}

/// Completes the hello exchange, then rejects the request as busy.
async fn busy_pairing(
    connection: &mut SessionConnection,
    timeouts: SessionTimeouts,
) -> FlowResult<()> {
    let inbound = read_control_only(connection, Instant::now() + timeouts.control).await?;
    if !matches!(inbound.control, Control::Hello(_)) {
        return Err(FlowOutcome::Failed);
    }
    connection.send_control(&local_hello()).await?;
    let inbound = read_control_only(connection, Instant::now() + timeouts.control).await?;
    if !matches!(inbound.control, Control::PairRequest) {
        return Err(FlowOutcome::Failed);
    }
    // The rejection is flushed before the socket closes so the initiator
    // reads the reason instead of a truncated connection.
    connection
        .send_control_flushed(&Control::PairResponse(PairResponse::rejected(
            PairRejection::Busy,
        )))
        .await?;
    Ok(())
}

/// Runs the responder pairing stages over an established connection.
async fn responder_pairing(
    connection: &mut SessionConnection,
    exporter: &[u8],
    address: SocketAddr,
    events: &EventSender,
    mut commands: mpsc::Receiver<ConnectionCommand>,
    timeouts: SessionTimeouts,
) -> FlowOutcome {
    // The initiator's hello arrives first, then the responder answers.
    let inbound =
        match read_control(connection, &mut commands, Instant::now() + timeouts.control).await {
            Ok(inbound) => inbound,
            Err(outcome) => return outcome,
        };
    let display_name = match inbound.control {
        Control::Hello(hello) => hello.display_name,
        _ => return FlowOutcome::Failed,
    };
    let initiator_hello = inbound.body;
    connection.peer_name = display_name.clone();
    let hello = local_hello();
    let responder_hello = Bytes::from(hello.encode());
    if let Err(outcome) = connection.send_control(&hello).await {
        return outcome;
    }
    match read_control(connection, &mut commands, Instant::now() + timeouts.control).await {
        Ok(inbound) if matches!(inbound.control, Control::PairRequest) => {}
        Ok(_) => return FlowOutcome::Failed,
        Err(outcome) => return outcome,
    }

    // Ask the local user; the peer name is untrusted and only rendered.
    let peer = PairingPeer::new(display_name, address.to_string());
    let _ = events.send(AppEvent::IncomingPairingRequest(peer)).await;
    let decision =
        match wait_decision(connection, &mut commands, Instant::now() + timeouts.prompt).await {
            Ok(decision) => decision,
            Err(outcome) => return outcome,
        };
    match decision {
        Decision::Accept => {}
        Decision::Reject => {
            let _ = connection
                .send_control(&Control::PairResponse(PairResponse::rejected(
                    PairRejection::UserRejected,
                )))
                .await;
            return FlowOutcome::Ended;
        }
        Decision::Busy => {
            let _ = connection
                .send_control(&Control::PairResponse(PairResponse::rejected(
                    PairRejection::Busy,
                )))
                .await;
            return FlowOutcome::Ended;
        }
        Decision::Timeout => {
            let _ = connection
                .send_control(&Control::PairResponse(PairResponse::rejected(
                    PairRejection::Timeout,
                )))
                .await;
            return FlowOutcome::Ended;
        }
        Decision::Close => return FlowOutcome::Ended,
    }

    // The code is created only after acceptance and stays in memory.
    let mut rng = UnwrapErr(OsRng);
    let code = PairingCode::generate(&mut rng);
    let code_deadline = Instant::now() + timeouts.code;
    if let Err(outcome) = connection
        .send_control(&Control::PairResponse(PairResponse::accepted()))
        .await
    {
        return outcome;
    }
    let _ = events.send(AppEvent::PairingCodeIssued(code.clone())).await;

    let binding = match pairing::binding(exporter, &initiator_hello, &responder_hello) {
        Ok(binding) => binding,
        Err(_) => return FlowOutcome::Failed,
    };
    let (share, state) = match pairing::start_responder(&code, &binding, &mut rng) {
        Ok(pair) => pair,
        Err(_) => return FlowOutcome::Failed,
    };

    // Record 1: the initiator's share arrives; record 2 answers with ours.
    let inbound =
        match read_pairing(connection, &mut commands, code_deadline, PairingStep::Share).await {
            Ok(inbound) => inbound,
            Err(outcome) => return outcome,
        };
    let initiator_share = match inbound.control {
        Control::Pairing(record) => record.data,
        _ => return FlowOutcome::Failed,
    };
    if let Err(outcome) = connection
        .send_control(&Control::Pairing(PairingRecord::new(
            PairingStep::Share,
            share,
        )))
        .await
    {
        return outcome;
    }
    let output = match pairing::finish_responder(state, &initiator_share) {
        Ok(output) => output,
        Err(_) => return FlowOutcome::Failed,
    };

    // Record 3: the initiator's confirmation. A failure closes the connection
    // and destroys the code, so there is exactly one cryptographic attempt.
    let inbound = match read_pairing(
        connection,
        &mut commands,
        code_deadline,
        PairingStep::Confirm,
    )
    .await
    {
        Ok(inbound) => inbound,
        Err(outcome) => return outcome,
    };
    let initiator_tag = match inbound.control {
        Control::Pairing(record) => record.data,
        _ => return FlowOutcome::Failed,
    };
    if pairing::verify(&output, &initiator_tag).is_err() {
        return FlowOutcome::Failed;
    }
    drop(code);

    // Record 4: our confirmation must be flushed before the session counts as
    // authorized on this side.
    let tag = match pairing::confirmation(&output) {
        Ok(tag) => tag,
        Err(_) => return FlowOutcome::Failed,
    };
    if let Err(outcome) = connection
        .send_control_flushed(&Control::Pairing(PairingRecord::new(
            PairingStep::Confirm,
            tag.to_vec(),
        )))
        .await
    {
        return outcome;
    }

    let _ = events.send(AppEvent::PairingSucceeded).await;
    idle_authorized(connection, &mut commands, events, timeouts).await
}

/// Keeps an authorized connection open across any number of transfers.
///
/// Each round is one idle wait followed by either an outbound or inbound
/// transfer. `Ok(())` means the session returned to idle and a fresh 600-second
/// deadline starts; every error ends the authorized session.
async fn idle_authorized(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    events: &EventSender,
    timeouts: SessionTimeouts,
) -> FlowOutcome {
    loop {
        let idle_deadline = Instant::now() + timeouts.idle;
        match session_round(connection, commands, events, timeouts, idle_deadline).await {
            Ok(()) => continue,
            // Any failure after authorization closes the session cleanly so
            // in-flight DATA cannot enter a later transfer.
            Err(_) => return FlowOutcome::SessionEnded,
        }
    }
}

/// Waits for the next transfer proposal or a local command while idle.
///
/// The idle deadline only covers this wait. Starting a proposal or transfer
/// leaves this function, so active work is governed by its own deadlines.
async fn session_round(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    events: &EventSender,
    timeouts: SessionTimeouts,
    idle_deadline: Instant,
) -> FlowResult<()> {
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None => return Err(FlowOutcome::SessionEnded),
                Some(ConnectionCommand::Close(code)) => {
                    connection.close_code = Some(code);
                    return Err(FlowOutcome::SessionEnded);
                }
                Some(ConnectionCommand::StartTransfer(selection)) => {
                    return run_outbound(connection, commands, events, selection, timeouts).await;
                }
                // A stale prompt decision or a racing local proposal cannot
                // start anything here; the protocol actions are authoritative.
                Some(_) => continue,
            },
            result = connection.read() => match result? {
                None => return Err(FlowOutcome::SessionEnded),
                // DATA is rejected by the protocol state while idle.
                Some(InboundMessage::Data(_)) => return Err(FlowOutcome::Failed),
                Some(InboundMessage::Control(inbound)) => {
                    if let Control::TransferRequest(request) = inbound.control {
                        return run_review(connection, commands, events, request, timeouts).await;
                    }
                    // A stale collision response is consumed by the protocol
                    // state; anything else valid here is ignored.
                }
            },
            // The local close reason is reported by `SessionConnection::close`
            // once the flow ends.
            () = tokio::time::sleep_until(idle_deadline) => {
                connection.close_code = Some(CloseCode::IdleTimeout);
                return Err(FlowOutcome::SessionEnded);
            },
        }
    }
}

/// The peer's decision on a pending outbound proposal.
enum ResponseDecision {
    /// The peer accepted; its `ready` follows.
    Accepted,
    /// The peer rejected or cancelled; only this proposal ended.
    Ended,
    /// The responder withdrew its proposal and this inbound request won.
    Withdrawn(TransferRequest),
    /// No decision arrived before the local deadline.
    TimedOut,
}

/// Sends one immutable manifest and drives the resulting transfer.
async fn run_outbound(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    events: &EventSender,
    selection: FileSelection,
    timeouts: SessionTimeouts,
) -> FlowResult<()> {
    let Some(request) = selection.request() else {
        // The app only proposes reviewed selections; recover instead of
        // stalling if an empty one ever reaches this layer.
        let _ = events.send(AppEvent::ProposalRejected).await;
        return Ok(());
    };
    connection
        .send_control(&Control::TransferRequest(request))
        .await?;

    let response = wait_transfer_response(
        connection,
        commands,
        Instant::now() + timeouts.prompt + timeouts.control,
    )
    .await?;
    match response {
        ResponseDecision::Accepted => {}
        ResponseDecision::Withdrawn(request) => {
            return run_review(connection, commands, events, request, timeouts).await;
        }
        ResponseDecision::Ended => {
            let _ = events.send(AppEvent::ProposalRejected).await;
            return Ok(());
        }
        ResponseDecision::TimedOut => {
            let _ = connection
                .send_control(&Control::TransferCancel(TransferCancel {
                    code: CancelCode::UserCancelled,
                }))
                .await;
            let _ = events.send(AppEvent::ProposalRejected).await;
            return Ok(());
        }
    }

    let ready = wait_ready(connection, commands, Instant::now() + timeouts.control).await?;
    if !ready {
        let _ = events.send(AppEvent::ProposalRejected).await;
        return Ok(());
    }
    run_sending(connection, commands, events, selection, timeouts).await
}

/// Maps the phase after a peer `transfer_response` to its decision.
///
/// A stale `busy` response for a withdrawn proposal is consumed by the
/// protocol state without changing the phase, so `None` means the response
/// did not decide the current proposal and the wait must continue.
fn response_decision(phase: &Phase) -> Option<ResponseDecision> {
    match phase {
        Phase::Idle => Some(ResponseDecision::Ended),
        Phase::Sending { .. } => Some(ResponseDecision::Accepted),
        _ => None,
    }
}

/// Waits for the peer's response, answering any racing proposal as busy.
async fn wait_transfer_response(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    deadline: Instant,
) -> FlowResult<ResponseDecision> {
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None => return Err(FlowOutcome::SessionEnded),
                Some(ConnectionCommand::Close(code)) => {
                    connection.close_code = Some(code);
                    return Err(FlowOutcome::SessionEnded);
                }
                Some(_) => continue,
            },
            result = timeout_at(deadline, connection.read()) => match result {
                Err(_) => return Ok(ResponseDecision::TimedOut),
                Ok(Ok(Some(InboundMessage::Control(inbound)))) => {
                    send_actions(connection, &inbound.actions).await?;
                    match &inbound.control {
                        Control::TransferResponse(_) => {
                            if let Some(decision) =
                                response_decision(connection.protocol.phase())
                            {
                                return Ok(decision);
                            }
                            // The stale busy for the withdrawn proposal was
                            // consumed without deciding this proposal.
                        }
                        Control::TransferCancel(_) => return Ok(ResponseDecision::Ended),
                        // The collision rule withdrew the responder's own
                        // proposal, so the peer's request wins instead.
                        Control::TransferRequest(request)
                            if matches!(connection.protocol.phase(), Phase::Reviewing { .. }) =>
                        {
                            return Ok(ResponseDecision::Withdrawn(request.clone()));
                        }
                        _ => continue,
                    }
                }
                Ok(Ok(Some(InboundMessage::Data(_)))) => continue,
                Ok(Ok(None)) => return Err(FlowOutcome::SessionEnded),
                Ok(Err(outcome)) => return Err(outcome),
            },
        }
    }
}

/// Waits for `ready` after an accepted response.
///
/// A deadline or peer cancellation before any DATA sends a `transfer_cancel`
/// and returns `false`; the session stays authorized.
async fn wait_ready(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    deadline: Instant,
) -> FlowResult<bool> {
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None => return Err(FlowOutcome::SessionEnded),
                Some(ConnectionCommand::Close(code)) => {
                    connection.close_code = Some(code);
                    return Err(FlowOutcome::SessionEnded);
                }
                Some(_) => continue,
            },
            result = timeout_at(deadline, connection.read()) => match result {
                Err(_) => {
                    let _ = connection
                        .send_control(&Control::TransferCancel(TransferCancel {
                            code: CancelCode::UserCancelled,
                        }))
                        .await;
                    return Ok(false);
                }
                Ok(Ok(Some(InboundMessage::Control(inbound)))) => {
                    send_actions(connection, &inbound.actions).await?;
                    match inbound.control {
                        Control::Ready => return Ok(true),
                        Control::TransferCancel(_) => return Ok(false),
                        _ => continue,
                    }
                }
                Ok(Ok(Some(InboundMessage::Data(_)))) => continue,
                Ok(Ok(None)) => return Err(FlowOutcome::SessionEnded),
                Ok(Err(outcome)) => return Err(outcome),
            },
        }
    }
}

/// The recipient's decision on an inbound proposal.
enum ReviewDecision {
    /// The local user accepted and chose the destination directory.
    Accept(PathBuf),
    /// The local user rejected.
    Reject,
    /// The peer cancelled the proposal before any local decision.
    Ended,
    /// The local review deadline elapsed.
    TimedOut,
}

/// Shows one inbound manifest and waits for the local decision.
async fn run_review(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    events: &EventSender,
    request: TransferRequest,
    timeouts: SessionTimeouts,
) -> FlowResult<()> {
    let proposal = TransferProposal::new(&request, connection.peer_name.clone());
    let _ = events
        .send(AppEvent::IncomingTransferRequest(proposal))
        .await;

    let decision =
        wait_transfer_decision(connection, commands, Instant::now() + timeouts.prompt).await?;
    match decision {
        ReviewDecision::Accept(destination) => {
            let destination = Destination::new(destination);
            match prepare_transfer(&request, &destination).await {
                Ok(first) => {
                    connection
                        .send_control(&Control::TransferResponse(TransferResponse::accepted()))
                        .await?;
                    connection.send_control(&Control::Ready).await?;
                    run_receiving(
                        connection,
                        commands,
                        events,
                        request,
                        destination,
                        first,
                        timeouts,
                    )
                    .await
                }
                Err(reason) => {
                    let _ = connection
                        .send_control(&Control::TransferResponse(TransferResponse::rejected(
                            reason,
                        )))
                        .await;
                    let _ = events.send(AppEvent::ProposalRejected).await;
                    Ok(())
                }
            }
        }
        ReviewDecision::Reject => {
            let _ = connection
                .send_control(&Control::TransferResponse(TransferResponse::rejected(
                    TransferRejection::UserRejected,
                )))
                .await;
            Ok(())
        }
        ReviewDecision::Ended => {
            let _ = events.send(AppEvent::ProposalRejected).await;
            Ok(())
        }
        ReviewDecision::TimedOut => {
            let _ = connection
                .send_control(&Control::TransferResponse(TransferResponse::rejected(
                    TransferRejection::Timeout,
                )))
                .await;
            let _ = events.send(AppEvent::ProposalRejected).await;
            Ok(())
        }
    }
}

/// Waits for the local accept or reject decision on an inbound proposal.
async fn wait_transfer_decision(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    deadline: Instant,
) -> FlowResult<ReviewDecision> {
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None => return Err(FlowOutcome::SessionEnded),
                Some(ConnectionCommand::Close(code)) => {
                    connection.close_code = Some(code);
                    return Err(FlowOutcome::SessionEnded);
                }
                Some(ConnectionCommand::AcceptTransfer(destination)) => {
                    return Ok(ReviewDecision::Accept(destination));
                }
                Some(ConnectionCommand::RejectTransfer) => return Ok(ReviewDecision::Reject),
                Some(_) => continue,
            },
            result = timeout_at(deadline, connection.read()) => match result {
                Err(_) => return Ok(ReviewDecision::TimedOut),
                Ok(Ok(Some(InboundMessage::Control(inbound)))) => {
                    send_actions(connection, &inbound.actions).await?;
                    match inbound.control {
                        Control::TransferCancel(_) => return Ok(ReviewDecision::Ended),
                        // A stale busy response for a withdrawn proposal is
                        // consumed by the protocol state; ignore it.
                        _ => continue,
                    }
                }
                Ok(Ok(Some(InboundMessage::Data(_)))) => continue,
                Ok(Ok(None)) => return Err(FlowOutcome::SessionEnded),
                Ok(Err(outcome)) => return Err(outcome),
            },
        }
    }
}

/// Validates the manifest and prepares the first partial file.
async fn prepare_transfer(
    request: &TransferRequest,
    destination: &Destination,
) -> Result<IncomingFile, TransferRejection> {
    destination
        .check_manifest(&request.files)
        .map_err(rejection_for)?;
    let first = request
        .files
        .first()
        .ok_or(TransferRejection::InvalidManifest)?;
    IncomingFile::begin(destination, first)
        .await
        .map_err(rejection_for)
}

/// Maps one local storage failure to the closed wire rejection.
const fn rejection_for(error: StorageError) -> TransferRejection {
    match error {
        StorageError::InvalidManifest => TransferRejection::InvalidManifest,
        StorageError::InvalidName => TransferRejection::InvalidFilename,
        StorageError::NameConflict => TransferRejection::NameConflict,
        StorageError::DestinationExists => TransferRejection::DestinationExists,
        StorageError::InvalidDestination | StorageError::Io => TransferRejection::Unavailable,
    }
}

/// Streams every reviewed file in manifest order.
async fn run_sending(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    events: &EventSender,
    selection: FileSelection,
    timeouts: SessionTimeouts,
) -> FlowResult<()> {
    let files = u16::try_from(selection.len()).map_err(|_| FlowOutcome::Failed)?;
    let _ = events.send(AppEvent::TransferStarted).await;
    for index in 0..files {
        let file = &selection.files()[usize::from(index)];
        let _ = events.send(progress_event(index, files, 0)).await;
        if stream_one_file(connection, commands, file, index, timeouts).await? {
            let _ = events.send(AppEvent::TransferFinished).await;
            return Ok(());
        }
        let _ = events.send(progress_event(index, files, file.size())).await;
    }
    Err(FlowOutcome::Failed)
}

/// Streams one reviewed file and waits for its verified result.
///
/// Returns `true` when the result completed the whole transfer. Queuing one
/// DATA frame is bounded by the progress deadline so a stalled peer closes the
/// session instead of blocking the sender forever.
async fn stream_one_file(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    file: &SelectedFile,
    index: u16,
    timeouts: SessionTimeouts,
) -> FlowResult<bool> {
    let (sender, mut receiver) = mpsc::channel(engine::DATA_CHANNEL_CAPACITY);
    let path = file.path().to_owned();
    let size = file.size();
    let sender_task = tokio::spawn(async move { engine::send_file(&path, size, &sender).await });

    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None => return Err(FlowOutcome::SessionEnded),
                Some(ConnectionCommand::Close(code)) => {
                    connection.close_code = Some(code);
                    return Err(FlowOutcome::SessionEnded);
                }
                Some(_) => continue,
            },
            chunk = receiver.recv() => match chunk {
                Some(chunk) => {
                    protocol::send_data(&connection.protocol).map_err(|_| FlowOutcome::Failed)?;
                    // The writer prefers queued DATA, so the file_end queued
                    // after the last chunk can never overtake it.
                    match timeout_at(
                        Instant::now() + timeouts.progress,
                        connection.outbound.send_data(chunk),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(_)) => return Err(FlowOutcome::Failed),
                        Err(_) => {
                            report_timeout(connection).await;
                            return Err(FlowOutcome::SessionEnded);
                        }
                    }
                }
                None => break,
            },
            result = connection.read() => match result? {
                None => return Err(FlowOutcome::SessionEnded),
                // No control and no DATA are valid before this file_end.
                Some(_) => return Err(FlowOutcome::Failed),
            },
        }
    }

    let digest = match sender_task.await {
        Ok(Ok(digest)) => digest,
        Ok(Err(_)) | Err(_) => {
            // The reviewed source changed or failed: withdraw after ready,
            // which closes the session because DATA may be in flight.
            let _ = connection
                .send_control(&Control::TransferCancel(TransferCancel {
                    code: CancelCode::SourceUnavailable,
                }))
                .await;
            return Err(FlowOutcome::SessionEnded);
        }
    };

    connection
        .send_control(&Control::FileEnd(FileEnd {
            index,
            sha256: digest,
        }))
        .await?;
    wait_file_result(connection, commands, timeouts.progress).await
}

/// Waits for the recipient's verified result for the current file.
///
/// The wait is bounded by the progress deadline so a silent recipient closes
/// the session instead of holding the sender open forever.
async fn wait_file_result(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    progress: Duration,
) -> FlowResult<bool> {
    let mut deadline = Instant::now() + progress;
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None => return Err(FlowOutcome::SessionEnded),
                Some(ConnectionCommand::Close(code)) => {
                    connection.close_code = Some(code);
                    return Err(FlowOutcome::SessionEnded);
                }
                Some(_) => continue,
            },
            result = timeout_at(deadline, connection.read()) => match result {
                Err(_) => {
                    report_timeout(connection).await;
                    return Err(FlowOutcome::SessionEnded);
                }
                Ok(Ok(None)) => return Err(FlowOutcome::SessionEnded),
                Ok(Ok(Some(InboundMessage::Data(_)))) => return Err(FlowOutcome::Failed),
                Ok(Ok(Some(InboundMessage::Control(inbound)))) => {
                    deadline = Instant::now() + progress;
                    send_actions(connection, &inbound.actions).await?;
                    match inbound.control {
                        Control::FileResult(_) => {
                            return Ok(inbound
                                .actions
                                .contains(&ProtocolAction::TransferFinished));
                        }
                        Control::TransferCancel(_) => return Err(FlowOutcome::SessionEnded),
                        _ => continue,
                    }
                }
                Ok(Err(outcome)) => return Err(outcome),
            },
        }
    }
}

/// Receives every manifest entry in order and verifies each digest.
///
/// Every inbound frame resets the progress deadline, so a peer that goes silent
/// mid-transfer closes the session after cleanup instead of hanging.
async fn run_receiving(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
    events: &EventSender,
    request: TransferRequest,
    destination: Destination,
    mut incoming: IncomingFile,
    timeouts: SessionTimeouts,
) -> FlowResult<()> {
    let files = u16::try_from(request.files.len()).map_err(|_| FlowOutcome::Failed)?;
    let _ = events.send(AppEvent::TransferStarted).await;
    let _ = events.send(progress_event(0, files, 0)).await;
    let mut index = 0u16;
    let mut deadline = Instant::now() + timeouts.progress;
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None => return Err(FlowOutcome::SessionEnded),
                Some(ConnectionCommand::Close(code)) => {
                    connection.close_code = Some(code);
                    return Err(FlowOutcome::SessionEnded);
                }
                Some(_) => continue,
            },
            result = timeout_at(deadline, connection.read()) => match result {
                Err(_) => {
                    report_timeout(connection).await;
                    return Err(FlowOutcome::SessionEnded);
                }
                Ok(Ok(None)) => return Err(FlowOutcome::SessionEnded),
                Ok(Ok(Some(InboundMessage::Data(chunk)))) => {
                    deadline = Instant::now() + timeouts.progress;
                    if incoming.write(&chunk).await.is_err() {
                        // The partial file is removed when it is dropped.
                        report_internal_error(connection).await;
                        return Err(FlowOutcome::SessionEnded);
                    }
                }
                Ok(Ok(Some(InboundMessage::Control(inbound)))) => {
                    deadline = Instant::now() + timeouts.progress;
                    match inbound.control {
                        Control::FileEnd(file_end) => {
                            if file_end.index != index {
                                return Err(FlowOutcome::Failed);
                            }
                            let finished_size = request.files[usize::from(index)].size;
                            match incoming.finish(file_end.sha256).await {
                                Ok(()) => {
                                    let actions = connection
                                        .send_control(&Control::FileResult(
                                            FileResult::verified(index),
                                        ))
                                        .await?;
                                    if actions.contains(&ProtocolAction::TransferFinished) {
                                        let _ = events.send(AppEvent::TransferFinished).await;
                                        return Ok(());
                                    }
                                }
                                Err(failure) => {
                                    // Protocol state closes the session after
                                    // a failed result; keep the verified prefix.
                                    let _ = connection
                                        .send_control(&Control::FileResult(
                                            FileResult::failed(index, failure),
                                        ))
                                        .await;
                                    return Err(FlowOutcome::SessionEnded);
                                }
                            }
                            let _ = events
                                .send(progress_event(index, files, finished_size))
                                .await;
                            index += 1;
                            let entry = &request.files[usize::from(index)];
                            incoming = match IncomingFile::begin(&destination, entry).await {
                                Ok(incoming) => incoming,
                                Err(_) => {
                                    report_internal_error(connection).await;
                                    return Err(FlowOutcome::SessionEnded);
                                }
                            };
                            let _ = events.send(progress_event(index, files, 0)).await;
                        }
                        Control::TransferCancel(_) => return Err(FlowOutcome::SessionEnded),
                        _ => continue,
                    }
                }
                Ok(Err(outcome)) => return Err(outcome),
            },
        }
    }
}

/// Reports a local failure that makes safe continuation impossible.
async fn report_internal_error(connection: &mut SessionConnection) {
    let _ = connection
        .send_control_flushed(&Control::Error(ErrorMessage {
            code: ErrorCode::InternalError,
        }))
        .await;
}

/// Reports an expired data-progress deadline, which closes the session.
async fn report_timeout(connection: &mut SessionConnection) {
    let _ = connection
        .send_control_flushed(&Control::Error(ErrorMessage {
            code: ErrorCode::Timeout,
        }))
        .await;
}

/// Queues every protocol action that asks for a control response.
async fn send_actions(
    connection: &mut SessionConnection,
    actions: &[ProtocolAction],
) -> FlowResult<()> {
    for action in actions {
        if let ProtocolAction::Send(control) = action {
            connection.send_control(control).await?;
        }
    }
    Ok(())
}

/// Builds one per-file progress event.
fn progress_event(index: u16, files: u16, transferred: u64) -> AppEvent {
    AppEvent::TransferProgress(TransferProgress {
        index,
        files,
        transferred,
    })
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use bytes::Bytes;
    use rand_core::{OsRng, UnwrapErr};
    use tokio::net::TcpListener;
    use tokio::time::timeout;

    use super::{
        ResponseDecision, SessionCommand, SessionService, SessionTimeouts, accepted_channel,
        response_decision,
    };
    use crate::app::action::{ConnectionTarget, DirectEndpoint};
    use crate::app::event::AppEvent;
    use crate::app::runtime::{EventReceiver, event_channel};
    use crate::framing::Frame;
    use crate::pairing::PairingCode;
    use crate::protocol::{
        CloseCode, Control, ErrorCode, ErrorMessage, FileEntry, Hello, PairRejection,
        PairingRecord, PairingStep, Phase, TransferRejection, TransferRequest, TransferResponse,
    };
    use crate::transfer::selection::FileSelection;
    use crate::transport::{FramedConnection, Outbound, TlsHandshake};

    /// Short deadlines so one test covers several stages quickly.
    ///
    /// The idle and progress deadlines stay long unless a test overrides them,
    /// so no existing flow test is cut short by either new deadline.
    fn test_timeouts() -> SessionTimeouts {
        SessionTimeouts {
            handshake: Duration::from_secs(5),
            control: Duration::from_secs(5),
            prompt: Duration::from_secs(5),
            code: Duration::from_secs(5),
            idle: Duration::from_secs(60),
            progress: Duration::from_secs(60),
        }
    }

    /// Waits for the next application event with a generous test deadline.
    async fn next_event(events: &mut EventReceiver) -> AppEvent {
        timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("application event before the test deadline")
            .expect("application event channel stays open")
    }

    /// Waits for the responder's one-time code event.
    async fn next_code(events: &mut EventReceiver) -> PairingCode {
        match next_event(events).await {
            AppEvent::PairingCodeIssued(code) => code,
            other => panic!("unexpected responder event: {other:?}"),
        }
    }

    /// Builds a direct connection target for a loopback address.
    fn direct(address: std::net::SocketAddr) -> ConnectionTarget {
        ConnectionTarget::Direct(DirectEndpoint::parse(&address.to_string()).unwrap())
    }

    /// Starts a responder whose listener forwards every accepted socket.
    async fn responder_under_test(
        timeouts: SessionTimeouts,
    ) -> (SessionService, EventReceiver, std::net::SocketAddr) {
        let (events, receiver) = event_channel();
        let (accepted, accepted_receiver) = accepted_channel();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let service = SessionService::with_timeouts(events, accepted_receiver, timeouts);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                if accepted.send(stream).await.is_err() {
                    break;
                }
            }
        });
        (service, receiver, address)
    }

    /// Starts an initiator that never receives inbound connections.
    async fn initiator_under_test(timeouts: SessionTimeouts) -> (SessionService, EventReceiver) {
        let (events, receiver) = event_channel();
        let (accepted, accepted_receiver) = accepted_channel();
        // Dropping the sender would close the accept side of the manager, so
        // the initiator-only tests leak one sender for the process lifetime.
        std::mem::forget(accepted);
        let service = SessionService::with_timeouts(events, accepted_receiver, timeouts);
        (service, receiver)
    }

    /// Rejects a code used by a test so a random collision cannot flake.
    fn another_code(code: &PairingCode) -> PairingCode {
        let mut value = 0u32;
        loop {
            let candidate = PairingCode::parse(&format!("{value:08}")).unwrap();
            if &candidate != code {
                return candidate;
            }
            value += 1;
        }
    }

    #[tokio::test]
    async fn loopback_pairing_authorizes_both_peers() {
        let (responder, mut responder_events, address) =
            responder_under_test(test_timeouts()).await;
        let (initiator, mut initiator_events) = initiator_under_test(test_timeouts()).await;

        initiator
            .send(SessionCommand::Connect(direct(address)))
            .await
            .unwrap();

        let AppEvent::IncomingPairingRequest(peer) = next_event(&mut responder_events).await else {
            panic!("the responder must show the pairing prompt");
        };
        // The prompt shows the live TCP source address, not the listener.
        assert!(peer.endpoint().starts_with("127.0.0.1:"));
        responder.send(SessionCommand::AcceptPairing).await.unwrap();

        let code = next_code(&mut responder_events).await;
        assert!(matches!(
            next_event(&mut initiator_events).await,
            AppEvent::PairingAccepted
        ));

        initiator
            .send(SessionCommand::SubmitPairingCode(code))
            .await
            .unwrap();

        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::PairingSucceeded
        ));
        assert!(matches!(
            next_event(&mut initiator_events).await,
            AppEvent::PairingSucceeded
        ));

        responder.stop().await;
        initiator.stop().await;
    }

    #[tokio::test]
    async fn rejection_closes_both_peers_without_authorization() {
        let (responder, mut responder_events, address) =
            responder_under_test(test_timeouts()).await;
        let (initiator, mut initiator_events) = initiator_under_test(test_timeouts()).await;

        initiator
            .send(SessionCommand::Connect(direct(address)))
            .await
            .unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::IncomingPairingRequest(_)
        ));

        responder.send(SessionCommand::RejectPairing).await.unwrap();

        assert!(matches!(
            next_event(&mut initiator_events).await,
            AppEvent::PairingEnded
        ));
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::PairingEnded
        ));

        responder.stop().await;
        initiator.stop().await;
    }

    #[tokio::test]
    async fn wrong_code_never_authorizes_either_peer() {
        let (responder, mut responder_events, address) =
            responder_under_test(test_timeouts()).await;
        let (initiator, mut initiator_events) = initiator_under_test(test_timeouts()).await;

        initiator
            .send(SessionCommand::Connect(direct(address)))
            .await
            .unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::IncomingPairingRequest(_)
        ));
        responder.send(SessionCommand::AcceptPairing).await.unwrap();
        let code = next_code(&mut responder_events).await;
        assert!(matches!(
            next_event(&mut initiator_events).await,
            AppEvent::PairingAccepted
        ));

        initiator
            .send(SessionCommand::SubmitPairingCode(another_code(&code)))
            .await
            .unwrap();

        // The responder rejects the confirmation and closes; both peers
        // return to browsing without authorization so a later attempt can
        // start a fresh pairing.
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::PairingEnded
        ));
        assert!(matches!(
            next_event(&mut initiator_events).await,
            AppEvent::PairingEnded
        ));

        responder.stop().await;
        initiator.stop().await;
    }

    #[tokio::test]
    async fn failed_inbound_handshake_returns_to_browsing() {
        let (responder, mut responder_events, address) =
            responder_under_test(test_timeouts()).await;

        // A connection that never completes TLS is a per-connection failure,
        // not a terminal application error.
        let stream = tokio::net::TcpStream::connect(address).await.unwrap();
        drop(stream);

        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::PairingEnded
        ));

        responder.stop().await;
    }

    #[tokio::test]
    async fn a_second_inbound_connection_is_rejected_busy() {
        let (responder, mut responder_events, address) =
            responder_under_test(test_timeouts()).await;
        let (first, mut first_events) = initiator_under_test(test_timeouts()).await;

        first
            .send(SessionCommand::Connect(direct(address)))
            .await
            .unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::IncomingPairingRequest(_)
        ));

        // A second peer completes the hello exchange and must read a busy
        // rejection instead of creating a second local prompt.
        let (stream, _exporter) = crate::transport::connect(address, test_timeouts().handshake)
            .await
            .unwrap();
        let (mut connection, outbound) = crate::transport::split_frame_io(stream);
        outbound
            .send_control(&Control::Hello(Hello::new(None)))
            .await
            .unwrap();
        assert!(connection.read_frame().await.unwrap().is_some());
        outbound.send_control(&Control::PairRequest).await.unwrap();
        let Some(Frame::Control(body)) = connection.read_frame().await.unwrap() else {
            panic!("the busy response must arrive as a control frame");
        };
        assert!(matches!(
            Control::decode(&body),
            Ok(Control::PairResponse(response))
                if !response.accepted && response.reason == Some(PairRejection::Busy)
        ));

        // The first prompt is untouched and can still pair normally.
        responder.send(SessionCommand::AcceptPairing).await.unwrap();
        let code = next_code(&mut responder_events).await;
        assert!(matches!(
            next_event(&mut first_events).await,
            AppEvent::PairingAccepted
        ));
        first
            .send(SessionCommand::SubmitPairingCode(code))
            .await
            .unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::PairingSucceeded
        ));
        assert!(matches!(
            next_event(&mut first_events).await,
            AppEvent::PairingSucceeded
        ));

        responder.stop().await;
        first.stop().await;
    }

    #[tokio::test]
    async fn peer_error_ends_pairing_without_waiting_for_the_deadline() {
        // A long prompt deadline proves the flow ends on the peer's error
        // message itself rather than on the local timeout.
        let timeouts = SessionTimeouts {
            prompt: Duration::from_secs(30),
            ..test_timeouts()
        };
        let (responder, mut responder_events, address) = responder_under_test(timeouts).await;

        let (stream, _exporter) = crate::transport::connect(address, timeouts.handshake)
            .await
            .unwrap();
        let (mut connection, outbound) = crate::transport::split_frame_io(stream);
        outbound
            .send_control(&Control::Hello(Hello::new(None)))
            .await
            .unwrap();
        assert!(connection.read_frame().await.unwrap().is_some());
        outbound.send_control(&Control::PairRequest).await.unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::IncomingPairingRequest(_)
        ));

        // The peer gives up while the local user is still deciding.
        outbound
            .send_control(&Control::Error(ErrorMessage {
                code: ErrorCode::InvalidMessage,
            }))
            .await
            .unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::PairingEnded
        ));

        responder.stop().await;
    }

    #[tokio::test]
    async fn a_connect_held_while_busy_starts_after_the_connection_ends() {
        let (responder, mut responder_events, address) =
            responder_under_test(test_timeouts()).await;

        // A raw peer holds the single active connection open in the prompt.
        let (stream, _exporter) = crate::transport::connect(address, test_timeouts().handshake)
            .await
            .unwrap();
        let (mut connection, outbound) = crate::transport::split_frame_io(stream);
        outbound
            .send_control(&Control::Hello(Hello::new(None)))
            .await
            .unwrap();
        assert!(connection.read_frame().await.unwrap().is_some());
        outbound.send_control(&Control::PairRequest).await.unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::IncomingPairingRequest(_)
        ));

        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let target_address = target.local_addr().unwrap();

        // The app believes it is browsing, so this connect must be held until
        // the previous connection's notice arrives, not dropped.
        responder
            .send(SessionCommand::Connect(direct(target_address)))
            .await
            .unwrap();

        // Ending the active connection must start the held connect.
        responder.send(SessionCommand::RejectPairing).await.unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::PairingEnded
        ));
        let (accepted, _) = timeout(Duration::from_secs(5), target.accept())
            .await
            .expect("the held connect must reach its target")
            .unwrap();
        drop(accepted);

        responder.stop().await;
    }

    #[tokio::test]
    async fn prompt_timeout_rejects_without_a_code() {
        let timeouts = SessionTimeouts {
            prompt: Duration::from_millis(100),
            ..test_timeouts()
        };
        let (responder, mut responder_events, address) = responder_under_test(timeouts).await;
        let (initiator, mut initiator_events) = initiator_under_test(timeouts).await;

        initiator
            .send(SessionCommand::Connect(direct(address)))
            .await
            .unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::IncomingPairingRequest(_)
        ));

        // The responder sends pair_response(timeout) and closes; the initiator
        // sees the rejection as a clean pairing end.
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::PairingEnded
        ));
        assert!(matches!(
            next_event(&mut initiator_events).await,
            AppEvent::PairingEnded
        ));

        responder.stop().await;
        initiator.stop().await;
    }

    #[tokio::test]
    async fn code_expiry_closes_the_pairing_without_authorization() {
        let timeouts = SessionTimeouts {
            code: Duration::from_millis(100),
            ..test_timeouts()
        };
        let (responder, mut responder_events, address) = responder_under_test(timeouts).await;
        let (initiator, mut initiator_events) = initiator_under_test(timeouts).await;

        initiator
            .send(SessionCommand::Connect(direct(address)))
            .await
            .unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::IncomingPairingRequest(_)
        ));
        responder.send(SessionCommand::AcceptPairing).await.unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::PairingCodeIssued(_)
        ));
        assert!(matches!(
            next_event(&mut initiator_events).await,
            AppEvent::PairingAccepted
        ));

        // No code is entered: the initiator's deadline closes the connection
        // and the responder observes the close before its own deadline.
        assert!(matches!(
            next_event(&mut initiator_events).await,
            AppEvent::PairingEnded
        ));
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::PairingEnded
        ));

        responder.stop().await;
        initiator.stop().await;
    }

    #[tokio::test]
    async fn disconnect_closes_an_authorized_session() {
        let (responder, mut responder_events, address) =
            responder_under_test(test_timeouts()).await;
        let (initiator, mut initiator_events) = initiator_under_test(test_timeouts()).await;

        initiator
            .send(SessionCommand::Connect(direct(address)))
            .await
            .unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::IncomingPairingRequest(_)
        ));
        responder.send(SessionCommand::AcceptPairing).await.unwrap();
        let code = next_code(&mut responder_events).await;
        assert!(matches!(
            next_event(&mut initiator_events).await,
            AppEvent::PairingAccepted
        ));
        initiator
            .send(SessionCommand::SubmitPairingCode(code))
            .await
            .unwrap();
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::PairingSucceeded
        ));
        assert!(matches!(
            next_event(&mut initiator_events).await,
            AppEvent::PairingSucceeded
        ));

        initiator.send(SessionCommand::Disconnect).await.unwrap();
        assert!(matches!(
            next_event(&mut initiator_events).await,
            AppEvent::SessionClosed
        ));
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::SessionClosed
        ));

        responder.stop().await;
        initiator.stop().await;
    }

    /// Waits for the first event matching `predicate`, skipping progress noise.
    async fn next_matching(
        events: &mut EventReceiver,
        mut predicate: impl FnMut(&AppEvent) -> bool,
    ) -> AppEvent {
        loop {
            let event = next_event(events).await;
            if predicate(&event) {
                return event;
            }
        }
    }

    /// Pairs two services over a real loopback connection.
    async fn pair_services(
        initiator: &SessionService,
        initiator_events: &mut EventReceiver,
        responder: &SessionService,
        responder_events: &mut EventReceiver,
        address: std::net::SocketAddr,
    ) {
        initiator
            .send(SessionCommand::Connect(direct(address)))
            .await
            .unwrap();
        assert!(matches!(
            next_event(responder_events).await,
            AppEvent::IncomingPairingRequest(_)
        ));
        responder.send(SessionCommand::AcceptPairing).await.unwrap();
        let code = next_code(responder_events).await;
        assert!(matches!(
            next_event(initiator_events).await,
            AppEvent::PairingAccepted
        ));
        initiator
            .send(SessionCommand::SubmitPairingCode(code))
            .await
            .unwrap();
        assert!(matches!(
            next_event(responder_events).await,
            AppEvent::PairingSucceeded
        ));
        assert!(matches!(
            next_event(initiator_events).await,
            AppEvent::PairingSucceeded
        ));
    }

    /// Creates an empty temporary directory for one test.
    fn temp_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lanweave-session-{tag}-{:016x}", fastrand::u64(..)));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// Reviews existing files into one outbound selection.
    fn selection(paths: &[&Path]) -> FileSelection {
        let mut selection = FileSelection::default();
        let input = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(selection.add_text(&input).is_empty());
        selection
    }

    /// Waits until the transfer reaches its running state.
    async fn wait_started(events: &mut EventReceiver) {
        assert!(matches!(
            next_matching(events, |event| matches!(event, AppEvent::TransferStarted)).await,
            AppEvent::TransferStarted
        ));
    }

    /// Waits until the transfer finishes.
    async fn wait_finished(events: &mut EventReceiver) {
        assert!(matches!(
            next_matching(events, |event| matches!(event, AppEvent::TransferFinished)).await,
            AppEvent::TransferFinished
        ));
    }

    /// Waits for the next inbound proposal and returns its file names.
    async fn next_proposal(events: &mut EventReceiver) -> Vec<(String, u64)> {
        let AppEvent::IncomingTransferRequest(proposal) = next_matching(events, |event| {
            matches!(event, AppEvent::IncomingTransferRequest(_))
        })
        .await
        else {
            unreachable!("the predicate only matches proposals");
        };
        proposal
            .files()
            .iter()
            .map(|entry| (entry.name.clone(), entry.size))
            .collect()
    }

    #[tokio::test]
    async fn loopback_transfers_reuse_one_session_in_both_directions() {
        let source = temp_root("source");
        let first_destination = temp_root("first-destination");
        let reverse_destination = temp_root("reverse-destination");
        let retry_destination = temp_root("retry-destination");

        let report = source.join("report.txt");
        std::fs::write(&report, b"report body").unwrap();
        let empty = source.join("empty.bin");
        std::fs::write(&empty, b"").unwrap();

        let (responder, mut responder_events, address) =
            responder_under_test(test_timeouts()).await;
        let (initiator, mut initiator_events) = initiator_under_test(test_timeouts()).await;
        pair_services(
            &initiator,
            &mut initiator_events,
            &responder,
            &mut responder_events,
            address,
        )
        .await;

        // Forward transfer: the responder chooses where files are stored.
        let outbound = selection(&[&report, &empty]);
        initiator
            .send(SessionCommand::StartTransfer(outbound))
            .await
            .unwrap();
        assert_eq!(
            next_proposal(&mut responder_events).await,
            [("report.txt".to_owned(), 11), ("empty.bin".to_owned(), 0)]
        );
        responder
            .send(SessionCommand::AcceptTransfer(first_destination.clone()))
            .await
            .unwrap();
        wait_started(&mut initiator_events).await;
        wait_started(&mut responder_events).await;
        wait_finished(&mut initiator_events).await;
        wait_finished(&mut responder_events).await;
        assert_eq!(
            std::fs::read(first_destination.join("report.txt")).unwrap(),
            b"report body"
        );
        assert_eq!(
            std::fs::read(first_destination.join("empty.bin")).unwrap(),
            b""
        );

        // The same session carries a reverse transfer.
        responder
            .send(SessionCommand::StartTransfer(selection(&[&report])))
            .await
            .unwrap();
        assert_eq!(
            next_proposal(&mut initiator_events).await,
            [("report.txt".to_owned(), 11)]
        );
        initiator
            .send(SessionCommand::AcceptTransfer(reverse_destination.clone()))
            .await
            .unwrap();
        wait_started(&mut initiator_events).await;
        wait_started(&mut responder_events).await;
        wait_finished(&mut initiator_events).await;
        wait_finished(&mut responder_events).await;
        assert_eq!(
            std::fs::read(reverse_destination.join("report.txt")).unwrap(),
            b"report body"
        );

        // A rejection before ready keeps the session open for another try.
        let retry = selection(&[&report]);
        initiator
            .send(SessionCommand::StartTransfer(retry.clone()))
            .await
            .unwrap();
        assert_eq!(
            next_proposal(&mut responder_events).await,
            [("report.txt".to_owned(), 11)]
        );
        responder
            .send(SessionCommand::RejectTransfer)
            .await
            .unwrap();
        assert!(matches!(
            next_matching(&mut initiator_events, |event| matches!(
                event,
                AppEvent::ProposalRejected
            ))
            .await,
            AppEvent::ProposalRejected
        ));

        initiator
            .send(SessionCommand::StartTransfer(retry))
            .await
            .unwrap();
        assert_eq!(
            next_proposal(&mut responder_events).await,
            [("report.txt".to_owned(), 11)]
        );
        responder
            .send(SessionCommand::AcceptTransfer(retry_destination.clone()))
            .await
            .unwrap();
        wait_started(&mut initiator_events).await;
        wait_started(&mut responder_events).await;
        wait_finished(&mut initiator_events).await;
        wait_finished(&mut responder_events).await;
        assert_eq!(
            std::fs::read(retry_destination.join("report.txt")).unwrap(),
            b"report body"
        );

        responder.stop().await;
        initiator.stop().await;
        for root in [
            source,
            first_destination,
            reverse_destination,
            retry_destination,
        ] {
            let _ = std::fs::remove_dir_all(root);
        }
    }

    /// Reads one frame from a raw test peer before the test deadline.
    async fn raw_frame<S>(connection: &mut FramedConnection<S>) -> Frame
    where
        S: tokio::io::AsyncRead + Unpin,
    {
        timeout(Duration::from_secs(10), connection.read_frame())
            .await
            .expect("a frame before the test deadline")
            .unwrap()
            .expect("the peer must not close during the test")
    }

    /// Reads one decoded control from a raw test peer.
    async fn raw_control<S>(connection: &mut FramedConnection<S>) -> Control
    where
        S: tokio::io::AsyncRead + Unpin,
    {
        let Frame::Control(body) = raw_frame(connection).await else {
            panic!("expected a control frame");
        };
        Control::decode(&body).unwrap()
    }

    /// Reads one pairing record of the expected step from a raw test peer.
    async fn raw_pairing_record<S>(
        connection: &mut FramedConnection<S>,
        step: PairingStep,
    ) -> Vec<u8>
    where
        S: tokio::io::AsyncRead + Unpin,
    {
        let Control::Pairing(record) = raw_control(connection).await else {
            panic!("expected a pairing record");
        };
        assert_eq!(record.step, step);
        record.data
    }

    /// Completes pairing as a raw initiator so the test can order every later
    /// frame itself.
    async fn pair_raw_initiator(
        handshake: TlsHandshake,
        responder: &SessionService,
        responder_events: &mut EventReceiver,
    ) -> (
        FramedConnection<tokio_rustls::TlsStream<tokio::net::TcpStream>>,
        Outbound,
    ) {
        let (stream, exporter) = handshake;
        let (mut connection, outbound) = crate::transport::split_frame_io(stream);

        let hello = Control::Hello(Hello::new(None));
        let initiator_hello = Bytes::from(hello.encode());
        outbound.send_control(&hello).await.unwrap();
        let Frame::Control(responder_hello) = raw_frame(&mut connection).await else {
            panic!("the responder hello must be a control frame");
        };

        outbound.send_control(&Control::PairRequest).await.unwrap();
        assert!(matches!(
            next_event(responder_events).await,
            AppEvent::IncomingPairingRequest(_)
        ));
        responder.send(SessionCommand::AcceptPairing).await.unwrap();
        let code = next_code(responder_events).await;
        assert!(matches!(
            raw_control(&mut connection).await,
            Control::PairResponse(response) if response.accepted
        ));

        let binding =
            crate::pairing::binding(&exporter[..], &initiator_hello, &responder_hello).unwrap();
        let mut rng = UnwrapErr(OsRng);
        let (share, state) = crate::pairing::start_initiator(&code, &binding, &mut rng).unwrap();
        outbound
            .send_control(&Control::Pairing(PairingRecord::new(
                PairingStep::Share,
                share,
            )))
            .await
            .unwrap();
        let peer_share = raw_pairing_record(&mut connection, PairingStep::Share).await;
        let output = crate::pairing::finish_initiator(state, &peer_share).unwrap();

        let tag = crate::pairing::confirmation(&output).unwrap();
        outbound
            .send_control_flushed(&Control::Pairing(PairingRecord::new(
                PairingStep::Confirm,
                tag.to_vec(),
            )))
            .await
            .unwrap();
        let peer_tag = raw_pairing_record(&mut connection, PairingStep::Confirm).await;
        crate::pairing::verify(&output, &peer_tag).unwrap();
        assert!(matches!(
            next_event(responder_events).await,
            AppEvent::PairingSucceeded
        ));

        (connection, outbound)
    }

    #[test]
    fn transfer_response_decisions_follow_the_resulting_phase() {
        assert!(matches!(
            response_decision(&Phase::Idle),
            Some(ResponseDecision::Ended)
        ));
        assert!(matches!(
            response_decision(&Phase::Sending {
                files: 1,
                index: 0,
                started: false,
                awaiting_result: false,
            }),
            Some(ResponseDecision::Accepted)
        ));
        // A stale busy consumed by the collision rule leaves the phase
        // untouched, so the wait must continue instead of accepting.
        assert!(response_decision(&Phase::AwaitingResponse { files: 1 }).is_none());
    }

    #[tokio::test]
    async fn a_stale_busy_after_a_withdrawn_proposal_does_not_decide_the_next_one() {
        let (responder, mut responder_events, address) =
            responder_under_test(test_timeouts()).await;
        let handshake = crate::transport::connect(address, test_timeouts().handshake)
            .await
            .unwrap();
        let (mut peer, outbound) =
            pair_raw_initiator(handshake, &responder, &mut responder_events).await;

        // The responder proposes first; the raw peer reads the request but
        // withholds the busy answer.
        let queued = FileSelection::for_test(&[("queued.bin", 4)]);
        responder
            .send(SessionCommand::StartTransfer(queued.clone()))
            .await
            .unwrap();
        assert!(matches!(
            raw_control(&mut peer).await,
            Control::TransferRequest(_)
        ));

        // The raw peer's proposal crosses it: the responder withdraws its own
        // request and reviews the incoming manifest.
        outbound
            .send_control(&Control::TransferRequest(
                TransferRequest::new(vec![FileEntry {
                    name: "incoming.bin".to_owned(),
                    size: 3,
                }])
                .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(
            next_proposal(&mut responder_events).await,
            [("incoming.bin".to_owned(), 3)]
        );

        // Reject the review and immediately propose again, before the stale
        // busy for the withdrawn proposal has been read.
        responder
            .send(SessionCommand::RejectTransfer)
            .await
            .unwrap();
        responder
            .send(SessionCommand::StartTransfer(queued))
            .await
            .unwrap();
        assert!(matches!(
            raw_control(&mut peer).await,
            Control::TransferResponse(response) if !response.accepted
        ));
        assert!(matches!(
            raw_control(&mut peer).await,
            Control::TransferRequest(_)
        ));

        // The withheld stale busy arrives during the new proposal's wait. It
        // must not count as that proposal's decision: only the real rejection
        // that follows may end it.
        outbound
            .send_control(&Control::TransferResponse(TransferResponse::rejected(
                TransferRejection::Busy,
            )))
            .await
            .unwrap();
        outbound
            .send_control(&Control::TransferResponse(TransferResponse::rejected(
                TransferRejection::UserRejected,
            )))
            .await
            .unwrap();
        timeout(
            Duration::from_secs(2),
            next_matching(&mut responder_events, |event| {
                matches!(event, AppEvent::ProposalRejected)
            }),
        )
        .await
        .expect("the real rejection must decide the proposal");

        responder.stop().await;
    }

    #[tokio::test]
    async fn manual_close_sends_session_close_user_closed() {
        let (responder, mut responder_events, address) =
            responder_under_test(test_timeouts()).await;
        let handshake = crate::transport::connect(address, test_timeouts().handshake)
            .await
            .unwrap();
        let (mut peer, _outbound) =
            pair_raw_initiator(handshake, &responder, &mut responder_events).await;

        responder.send(SessionCommand::Disconnect).await.unwrap();

        let Frame::Control(body) = raw_frame(&mut peer).await else {
            panic!("the close reason must arrive as a control frame");
        };
        assert!(matches!(
            Control::decode(&body),
            Ok(Control::SessionClose(close)) if close.code == CloseCode::UserClosed
        ));
        // The message is not acknowledged; the transport closes after it.
        assert!(peer.read_frame().await.unwrap().is_none());
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::SessionClosed
        ));
    }

    #[tokio::test]
    async fn idle_expiry_sends_session_close_idle_timeout() {
        let timeouts = SessionTimeouts {
            idle: Duration::from_millis(200),
            ..test_timeouts()
        };
        let (responder, mut responder_events, address) = responder_under_test(timeouts).await;
        let handshake = crate::transport::connect(address, timeouts.handshake)
            .await
            .unwrap();
        let (mut peer, _outbound) =
            pair_raw_initiator(handshake, &responder, &mut responder_events).await;

        let Frame::Control(body) = raw_frame(&mut peer).await else {
            panic!("the idle close reason must arrive as a control frame");
        };
        assert!(matches!(
            Control::decode(&body),
            Ok(Control::SessionClose(close)) if close.code == CloseCode::IdleTimeout
        ));
        assert!(peer.read_frame().await.unwrap().is_none());
        assert!(matches!(
            next_event(&mut responder_events).await,
            AppEvent::SessionClosed
        ));
    }

    #[tokio::test]
    async fn a_pending_proposal_stops_the_idle_deadline_and_it_restarts_after_finish() {
        let timeouts = SessionTimeouts {
            idle: Duration::from_millis(200),
            ..test_timeouts()
        };
        let source = temp_root("idle-source");
        let destination = temp_root("idle-destination");
        let report = source.join("report.txt");
        std::fs::write(&report, b"report body").unwrap();

        let (responder, mut responder_events, address) = responder_under_test(timeouts).await;
        let (initiator, mut initiator_events) = initiator_under_test(timeouts).await;
        pair_services(
            &initiator,
            &mut initiator_events,
            &responder,
            &mut responder_events,
            address,
        )
        .await;

        // A pending proposal outlives the idle deadline without closing.
        initiator
            .send(SessionCommand::StartTransfer(selection(&[&report])))
            .await
            .unwrap();
        assert_eq!(
            next_proposal(&mut responder_events).await,
            [("report.txt".to_owned(), 11)]
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        responder
            .send(SessionCommand::AcceptTransfer(destination.clone()))
            .await
            .unwrap();
        wait_started(&mut initiator_events).await;
        wait_started(&mut responder_events).await;
        wait_finished(&mut initiator_events).await;
        wait_finished(&mut responder_events).await;
        assert_eq!(
            std::fs::read(destination.join("report.txt")).unwrap(),
            b"report body"
        );

        // Returning to idle starts a fresh deadline, which then closes both.
        assert!(matches!(
            next_matching(&mut initiator_events, |event| matches!(
                event,
                AppEvent::SessionClosed
            ))
            .await,
            AppEvent::SessionClosed
        ));
        assert!(matches!(
            next_matching(&mut responder_events, |event| matches!(
                event,
                AppEvent::SessionClosed
            ))
            .await,
            AppEvent::SessionClosed
        ));

        responder.stop().await;
        initiator.stop().await;
        let _ = std::fs::remove_dir_all(source);
        let _ = std::fs::remove_dir_all(destination);
    }

    #[tokio::test]
    async fn a_stalled_sender_closes_at_the_receivers_progress_deadline() {
        let timeouts = SessionTimeouts {
            progress: Duration::from_millis(200),
            ..test_timeouts()
        };
        let destination = temp_root("stalled-destination");
        let (responder, mut responder_events, address) = responder_under_test(timeouts).await;
        let handshake = crate::transport::connect(address, timeouts.handshake)
            .await
            .unwrap();
        let (mut peer, outbound) =
            pair_raw_initiator(handshake, &responder, &mut responder_events).await;

        // The raw peer proposes a file, the responder accepts, and then no
        // DATA ever arrives.
        outbound
            .send_control(&Control::TransferRequest(
                TransferRequest::new(vec![FileEntry {
                    name: "stalled.bin".to_owned(),
                    size: 4,
                }])
                .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(
            next_proposal(&mut responder_events).await,
            [("stalled.bin".to_owned(), 4)]
        );
        responder
            .send(SessionCommand::AcceptTransfer(destination.clone()))
            .await
            .unwrap();
        assert!(matches!(
            raw_control(&mut peer).await,
            Control::TransferResponse(response) if response.accepted
        ));
        assert!(matches!(raw_control(&mut peer).await, Control::Ready));

        // The progress deadline expires, the partial file is removed, and the
        // session closes.
        let Frame::Control(body) = raw_frame(&mut peer).await else {
            panic!("the timeout report must arrive as a control frame");
        };
        assert!(matches!(
            Control::decode(&body),
            Ok(Control::Error(error)) if error.code == ErrorCode::Timeout
        ));
        assert!(matches!(
            next_matching(&mut responder_events, |event| matches!(
                event,
                AppEvent::SessionClosed
            ))
            .await,
            AppEvent::SessionClosed
        ));
        assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);

        responder.stop().await;
        let _ = std::fs::remove_dir_all(destination);
    }

    #[tokio::test]
    async fn a_silent_recipient_closes_the_sender_at_the_progress_deadline() {
        let timeouts = SessionTimeouts {
            progress: Duration::from_millis(200),
            ..test_timeouts()
        };
        let source = temp_root("silent-source");
        let report = source.join("report.txt");
        std::fs::write(&report, b"report body").unwrap();

        let (responder, mut responder_events, address) = responder_under_test(timeouts).await;
        let handshake = crate::transport::connect(address, timeouts.handshake)
            .await
            .unwrap();
        let (mut peer, outbound) =
            pair_raw_initiator(handshake, &responder, &mut responder_events).await;

        // The local side proposes; the raw peer accepts the manifest and reads
        // every DATA frame, then never answers with `file_result`.
        responder
            .send(SessionCommand::StartTransfer(selection(&[&report])))
            .await
            .unwrap();
        assert!(matches!(
            raw_control(&mut peer).await,
            Control::TransferRequest(_)
        ));
        outbound
            .send_control(&Control::TransferResponse(TransferResponse::accepted()))
            .await
            .unwrap();
        outbound.send_control(&Control::Ready).await.unwrap();
        loop {
            match raw_frame(&mut peer).await {
                Frame::Data(_) => continue,
                Frame::Control(body) => match Control::decode(&body) {
                    Ok(Control::FileEnd(_)) => break,
                    Ok(_) => continue,
                    Err(_) => panic!("the file stream carries valid controls"),
                },
            }
        }

        // The file-result deadline expires, the sender reports the timeout,
        // and the session closes.
        let Frame::Control(body) = raw_frame(&mut peer).await else {
            panic!("the timeout report must arrive as a control frame");
        };
        assert!(matches!(
            Control::decode(&body),
            Ok(Control::Error(error)) if error.code == ErrorCode::Timeout
        ));
        assert!(matches!(
            next_matching(&mut responder_events, |event| matches!(
                event,
                AppEvent::SessionClosed
            ))
            .await,
            AppEvent::SessionClosed
        ));
        assert!(peer.read_frame().await.unwrap().is_none());

        responder.stop().await;
        let _ = std::fs::remove_dir_all(source);
    }

    #[tokio::test]
    async fn app_shutdown_sends_session_close_shutdown() {
        let (responder, mut responder_events, address) =
            responder_under_test(test_timeouts()).await;
        let handshake = crate::transport::connect(address, test_timeouts().handshake)
            .await
            .unwrap();
        let (mut peer, _outbound) =
            pair_raw_initiator(handshake, &responder, &mut responder_events).await;

        responder.stop().await;

        let Frame::Control(body) = raw_frame(&mut peer).await else {
            panic!("the shutdown close reason must arrive as a control frame");
        };
        assert!(matches!(
            Control::decode(&body),
            Ok(Control::SessionClose(close)) if close.code == CloseCode::Shutdown
        ));
        assert!(peer.read_frame().await.unwrap().is_none());
    }
}
