//! Authorized session ownership: pairing state, transfer policy, idle timer.
//!
//! One task owns each connection and its mutable session state. After the
//! `session_idle` state is reached, the reusable transfer loop runs here.
//!
//! This feature implements the pairing half: TCP/TLS setup, the `hello` and
//! `pair_request` exchange, the one-time code, the four SPAKE2 records, and
//! the authorized idle session. Transfer policy arrives in a later feature.
//!
//! The manager owns at most one connection at a time, so an extra inbound
//! socket is refused with `pair_response(busy)` and never enters the
//! application event loop. Connection tasks report terminal outcomes through
//! `AppEvent` and only touch application state through those events.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use rand_core::{OsRng, UnwrapErr};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout_at};

use crate::app::action::{ConnectionTarget, PairingPeer};
use crate::app::event::AppEvent;
use crate::app::runtime::EventSender;
use crate::framing::Frame;
use crate::pairing::{self, PairingCode};
use crate::protocol::{
    self, Control, Hello, PairRejection, PairResponse, PairingRecord, PairingStep, ProtocolAction,
    ProtocolState, Role,
};
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

/// Local deadlines for one pairing attempt.
///
/// The defaults match `docs/PROTOCOL.md`: the prompt and the code live for
/// 120 monotonic seconds. Tests override them to run quickly.
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
}

impl Default for SessionTimeouts {
    fn default() -> Self {
        Self {
            handshake: Duration::from_secs(10),
            control: Duration::from_secs(15),
            prompt: Duration::from_secs(120),
            code: Duration::from_secs(120),
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
        // path, which aborts the active connection before returning.
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
    Close,
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

    // Shutdown closes the active connection before the manager returns.
    if let Some(active) = active.take() {
        active.task.abort();
        let _ = active.task.await;
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
        SessionCommand::Disconnect => Some(ConnectionCommand::Close),
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
}

impl SessionConnection {
    /// Wraps a completed TLS connection and starts the ordered writer.
    fn new(stream: tokio_rustls::TlsStream<TcpStream>, role: Role) -> Self {
        let (connection, outbound) = split_frame_io(stream);
        Self {
            connection,
            outbound,
            protocol: ProtocolState::new(role),
        }
    }

    /// Validates and queues one control, returning its exact JSON body.
    async fn send_control(&mut self, control: &Control) -> FlowResult<Bytes> {
        protocol::send(&mut self.protocol, control).map_err(|_| FlowOutcome::Failed)?;
        let body = Bytes::from(control.encode());
        self.outbound
            .send_control(control)
            .await
            .map_err(|_| FlowOutcome::Failed)?;
        Ok(body)
    }

    /// Validates and queues one control, waiting until it is flushed.
    async fn send_control_flushed(&mut self, control: &Control) -> FlowResult<()> {
        protocol::send(&mut self.protocol, control).map_err(|_| FlowOutcome::Failed)?;
        self.outbound
            .send_control_flushed(control)
            .await
            .map_err(|_| FlowOutcome::Failed)
    }

    /// Flushes queued frames and shuts the write half down cleanly.
    ///
    /// The peer observes every queued frame followed by one clean close, so a
    /// rejection response can never be truncated by an abrupt drop.
    async fn close(&mut self) {
        self.outbound.close().await;
    }

    /// Reads and validates one inbound control, keeping its exact body.
    ///
    /// `Ok(None)` means the peer closed cleanly; `Err` is a terminal outcome.
    /// A control the protocol layer treats as terminal (a peer `error`, a
    /// rejection, or `session_close`) ends the connection immediately instead
    /// of waiting for the peer to close.
    async fn read(&mut self) -> FlowResult<Option<InboundControl>> {
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
                Ok(Some(InboundControl { control, body }))
            }
            // This feature has no transfer, so DATA is always a violation.
            Some(Frame::Data(_)) => Err(FlowOutcome::Failed),
        }
    }
}

/// A validated inbound control with its exact JSON body.
struct InboundControl {
    control: Control,
    body: Bytes,
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
            Ok(Ok(Some(inbound))) => Ok(Stage::Control(inbound)),
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
            Stage::Finished | Stage::TimedOut | Stage::Command(ConnectionCommand::Close) => {
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
        Ok(Ok(Some(inbound))) => Ok(inbound),
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
            Stage::Command(ConnectionCommand::Close) | Stage::Finished => {
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
            Stage::Finished | Stage::TimedOut | Stage::Command(ConnectionCommand::Close) => {
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

/// Runs the initiator pairing stages over an established connection.
async fn initiator_pairing(
    connection: &mut SessionConnection,
    exporter: &[u8],
    events: &EventSender,
    mut commands: mpsc::Receiver<ConnectionCommand>,
    timeouts: SessionTimeouts,
) -> FlowOutcome {
    // The initiator sends the first hello and retains the exact JSON body.
    let initiator_hello = match connection
        .send_control(&Control::Hello(Hello::new(None)))
        .await
    {
        Ok(body) => body,
        Err(outcome) => return outcome,
    };
    let inbound =
        match read_control(connection, &mut commands, Instant::now() + timeouts.control).await {
            Ok(inbound) => inbound,
            Err(outcome) => return outcome,
        };
    let responder_hello = match inbound.control {
        Control::Hello(_) => inbound.body,
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
    idle_authorized(connection, &mut commands).await
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
    connection
        .send_control(&Control::Hello(Hello::new(None)))
        .await?;
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
    let responder_hello = match connection
        .send_control(&Control::Hello(Hello::new(None)))
        .await
    {
        Ok(body) => body,
        Err(outcome) => return outcome,
    };
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
    idle_authorized(connection, &mut commands).await
}

/// Keeps an authorized connection open until either side ends it.
///
/// Transfer policy is a later feature; a `transfer_request` is validated by
/// the protocol state and then ignored here.
async fn idle_authorized(
    connection: &mut SessionConnection,
    commands: &mut mpsc::Receiver<ConnectionCommand>,
) -> FlowOutcome {
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None | Some(ConnectionCommand::Close) => return FlowOutcome::SessionEnded,
                Some(_) => continue,
            },
            result = connection.read() => match result {
                // Transfer controls are validated by the protocol state and
                // ignored until the transfer feature lands.
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => return FlowOutcome::SessionEnded,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::time::Duration;

    use tokio::net::TcpListener;
    use tokio::time::timeout;

    use super::{SessionCommand, SessionService, SessionTimeouts, accepted_channel};
    use crate::app::action::{ConnectionTarget, DirectEndpoint};
    use crate::app::event::AppEvent;
    use crate::app::runtime::{EventReceiver, event_channel};
    use crate::framing::Frame;
    use crate::pairing::PairingCode;
    use crate::protocol::{Control, ErrorCode, ErrorMessage, Hello, PairRejection};

    /// Short deadlines so one test covers several stages quickly.
    fn test_timeouts() -> SessionTimeouts {
        SessionTimeouts {
            handshake: Duration::from_secs(5),
            control: Duration::from_secs(5),
            prompt: Duration::from_secs(5),
            code: Duration::from_secs(5),
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
}
