//! Framed connection plumbing: one bounded ordered writer and one frame reader.
//!
//! [`split_frame_io`] takes any byte stream and returns a [`FramedConnection`]
//! for reading complete frames plus an [`Outbound`] handle that serializes
//! control and DATA frames over one writer task. Control and DATA arrive on
//! separate bounded queues for explicit backpressure, and order is per queue:
//! the writer prefers DATA while any is queued, so a control queued after a DATA
//! frame cannot overtake it and a `file_end` control can never overtake the
//! trailing DATA of the frame sequence that precedes it. A control queued before
//! a DATA frame may still be overtaken by it, so callers must not rely on
//! cross-queue send order.
//!
//! The writer flushes after every frame and shuts the write half down when it
//! stops. [`Outbound::close`] finishes queued frames before stopping, so the
//! peer observes one clean EOF. [`Outbound::send_control_flushed`] resolves
//! only after the queued control and every earlier frame were written and
//! flushed, which the pairing responder needs before it reports authorization.
//! Dropping the `Outbound` without closing aborts the writer, which can
//! truncate an in-flight frame and surface as a [`ReadError::Truncated`] on the
//! peer instead of a clean close.
#![cfg_attr(not(test), allow(dead_code))]

use std::fmt;
use std::io;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};

use crate::framing::{Frame, FrameError, MAX_CONTROL_BODY_BYTES, MAX_DATA_BODY_BYTES, decode};
use crate::protocol::Control;

/// Capacity of the ordered control frame queue.
pub(crate) const CONTROL_QUEUE_CAPACITY: usize = 64;
/// Capacity of the DATA frame queue (one frame is at most 1 MiB).
pub(crate) const DATA_QUEUE_CAPACITY: usize = 8;
/// Deadline for writing and flushing one frame to the socket.
pub(crate) const FRAME_WRITE_DEADLINE: Duration = Duration::from_secs(30);

/// Why an outbound frame could not be queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendError {
    /// The writer stopped because the connection was closed or failed.
    Closed,
    /// The frame body is empty or larger than the wire limit.
    InvalidData,
}

impl fmt::Display for SendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Closed => "connection closed",
            Self::InvalidData => "invalid frame body",
        };
        formatter.write_str(text)
    }
}

impl std::error::Error for SendError {}

/// Why a frame read failed.
///
/// Only transport-level failures reach here; frame bodies stay exact bytes and
/// message-level errors belong to callers.
#[derive(Debug)]
pub(crate) enum ReadError {
    /// An underlying stream failure.
    Io(io::Error),
    /// The peer sent a malformed frame.
    Frame(FrameError),
    /// The peer closed the stream mid-frame.
    Truncated,
}

impl fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "frame read failed: {error}"),
            Self::Frame(error) => write!(formatter, "malformed frame: {error}"),
            Self::Truncated => write!(formatter, "peer closed mid-frame"),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Frame(error) => Some(error),
            Self::Truncated => None,
        }
    }
}

/// Reader half of a framed connection.
///
/// [`FramedConnection::read_frame`] returns `Ok(None)` once the peer closed its
/// write side, after every frame that arrived before the close has been read.
pub(crate) struct FramedConnection<S> {
    reader: ReadHalf<S>,
    buffer: BytesMut,
}

impl<S> FramedConnection<S>
where
    S: AsyncRead + Unpin,
{
    fn new(reader: ReadHalf<S>) -> Self {
        Self {
            reader,
            buffer: BytesMut::with_capacity(8192),
        }
    }

    /// Reads the next complete frame, waiting for more bytes as needed.
    ///
    /// Partial and coalesced reads are handled by the shared frame codec; the
    /// buffer stays bounded by the validated per-kind body limits. `Ok(None)`
    /// is returned only for a clean close after complete frames; a closed
    /// stream with a partial frame in the buffer is a truncation, not a close.
    pub(crate) async fn read_frame(&mut self) -> Result<Option<Frame>, ReadError> {
        loop {
            if let Some(frame) = decode(&mut self.buffer).map_err(ReadError::Frame)? {
                return Ok(Some(frame));
            }
            let read = self
                .reader
                .read_buf(&mut self.buffer)
                .await
                .map_err(ReadError::Io)?;
            if read == 0 {
                return if self.buffer.is_empty() {
                    Ok(None)
                } else {
                    Err(ReadError::Truncated)
                };
            }
        }
    }
}

/// Splits a byte stream into a frame reader and a bounded ordered writer.
pub(crate) fn split_frame_io<S>(io: S) -> (FramedConnection<S>, Outbound)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (reader, writer) = tokio::io::split(io);
    (FramedConnection::new(reader), Outbound::spawn(writer))
}

/// One entry in the ordered control queue.
///
/// A barrier is queued behind the frames it must observe, so the single writer
/// can resolve it only after every earlier frame was written and flushed.
enum WriterItem {
    Frame(Frame),
    Barrier(oneshot::Sender<()>),
}

/// One bounded, ordered output path.
///
/// A send completes only once the frame is queued; a full queue applies
/// backpressure instead of growing memory.
pub(crate) struct Outbound {
    controls: mpsc::Sender<WriterItem>,
    data: mpsc::Sender<Frame>,
    stop: watch::Sender<bool>,
    writer: Option<JoinHandle<()>>,
}

impl Outbound {
    /// Spawns the single writer task for the given write half.
    fn spawn<S>(writer: WriteHalf<S>) -> Self
    where
        S: AsyncWrite + Unpin + Send + 'static,
    {
        let (controls, control_receiver) = mpsc::channel(CONTROL_QUEUE_CAPACITY);
        let (data, data_receiver) = mpsc::channel(DATA_QUEUE_CAPACITY);
        let (stop, stop_receiver) = watch::channel(false);
        let writer_task = tokio::spawn(writer_loop(
            writer,
            control_receiver,
            data_receiver,
            stop_receiver,
        ));
        Self {
            controls,
            data,
            stop,
            writer: Some(writer_task),
        }
    }

    /// Queues one control message behind the currently ordered frames.
    pub(crate) async fn send_control(&self, control: &Control) -> Result<(), SendError> {
        let frame = encode_control(control)?;
        self.controls
            .send(WriterItem::Frame(frame))
            .await
            .map_err(|_| SendError::Closed)
    }

    /// Queues one control message and waits until it is written and flushed.
    ///
    /// The wait also covers every frame queued before it, which is what the
    /// pairing responder needs before it treats the session as authorized.
    pub(crate) async fn send_control_flushed(&self, control: &Control) -> Result<(), SendError> {
        let frame = encode_control(control)?;
        let (acknowledge, acknowledged) = oneshot::channel();
        self.controls
            .send(WriterItem::Frame(frame))
            .await
            .map_err(|_| SendError::Closed)?;
        self.controls
            .send(WriterItem::Barrier(acknowledge))
            .await
            .map_err(|_| SendError::Closed)?;
        acknowledged.await.map_err(|_| SendError::Closed)
    }

    /// Queues one bounded DATA frame.
    pub(crate) async fn send_data(&self, data: Bytes) -> Result<(), SendError> {
        if data.is_empty() || data.len() > MAX_DATA_BODY_BYTES {
            return Err(SendError::InvalidData);
        }
        self.data
            .send(Frame::data(data))
            .await
            .map_err(|_| SendError::Closed)
    }

    /// Queues a DATA frame without backpressure, for boundedness tests.
    #[cfg(test)]
    fn try_send_data(&self, data: Bytes) -> Result<(), SendError> {
        if data.is_empty() || data.len() > MAX_DATA_BODY_BYTES {
            return Err(SendError::InvalidData);
        }
        self.data
            .try_send(Frame::data(data))
            .map_err(|_| SendError::Closed)
    }

    /// Stops the writer after queued frames are written and flushes the stream.
    ///
    /// The peer observes the queued frames followed by one clean EOF. The stop
    /// signal takes effect at the next scheduling point of the writer; a stalled
    /// write still finishes first. A hard abort means dropping the `Outbound`.
    pub(crate) async fn close(&mut self) {
        let _ = self.stop.send(true);
        if let Some(writer) = self.writer.take() {
            let _ = writer.await;
        }
    }
}

impl Drop for Outbound {
    /// Cancels queued frames and stops the writer task best-effort.
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(writer) = self.writer.take() {
            writer.abort();
        }
    }
}

/// The single serialized output path, preferring queued DATA to controls.
async fn writer_loop<W>(
    mut writer: W,
    mut controls: mpsc::Receiver<WriterItem>,
    mut data: mpsc::Receiver<Frame>,
    mut stop: watch::Receiver<bool>,
) where
    W: AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            biased;
            frame = data.recv() => {
                let Some(frame) = frame else { break };
                if write_frame(&mut writer, &frame).await.is_err() { break; }
            }
            item = controls.recv() => {
                match item {
                    None => break,
                    Some(WriterItem::Frame(frame)) => {
                        if write_frame(&mut writer, &frame).await.is_err() { break; }
                    }
                    // Every earlier control frame was written and flushed, so
                    // the waiting sender may stop treating it as in flight.
                    Some(WriterItem::Barrier(acknowledge)) => {
                        let _ = acknowledge.send(());
                    }
                }
            }
            stopped = stop.changed() => {
                if stopped.is_err() || *stop.borrow() { break; }
            }
        }
    }
    let _ = timeout(FRAME_WRITE_DEADLINE, writer.shutdown()).await;
}

/// Encodes one bounded control message into a frame.
fn encode_control(control: &Control) -> Result<Frame, SendError> {
    let body = control.encode();
    if body.len() > MAX_CONTROL_BODY_BYTES {
        return Err(SendError::InvalidData);
    }
    Ok(Frame::control(Bytes::from(body)))
}

/// Encodes and writes one complete frame, flushing the underlying stream.
async fn write_frame<W>(writer: &mut W, frame: &Frame) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut bytes = BytesMut::new();
    frame
        .encode(&mut bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    timeout(FRAME_WRITE_DEADLINE, async {
        writer.write_all(&bytes).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "frame write deadline exceeded"))?
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    use bytes::Bytes;
    use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf, duplex};

    use super::{DATA_QUEUE_CAPACITY, ReadError, SendError, split_frame_io};
    use crate::framing::Frame;
    use crate::protocol::{Control, FileEnd};

    fn data_frame(bytes: &'static [u8]) -> Frame {
        Frame::data(Bytes::from_static(bytes))
    }

    #[tokio::test]
    async fn coalesced_reads_decode_in_order() {
        let (client, writer) = duplex(8192);
        let (mut connection, _outbound) = split_frame_io(writer);
        let expected = [
            Frame::control(Bytes::from_static(br#"{"type":"pair_request"}"#)),
            data_frame(b"file-a"),
            Frame::control(Bytes::from_static(br#"{"type":"ready"}"#)),
        ];
        let copy = expected.clone();
        tokio::spawn(async move {
            let mut wire = bytes::BytesMut::new();
            for frame in &copy {
                frame.encode(&mut wire).unwrap();
            }
            let mut client = client;
            client.write_all(&wire).await.unwrap();
            client.flush().await.unwrap();
        });

        let mut decoded = Vec::new();
        for _ in 0..expected.len() {
            decoded.push(connection.read_frame().await.unwrap().unwrap());
        }
        assert_eq!(decoded, expected);
    }

    #[tokio::test]
    async fn byte_by_byte_writes_are_reassembled() {
        let (client, writer) = duplex(8192);
        let (mut connection, _outbound) = split_frame_io(writer);
        let expected = [
            data_frame(b"z"),
            Frame::control(Bytes::from_static(br#"{"type":"ready"}"#)),
        ];
        let copy = expected.clone();
        tokio::spawn(async move {
            let mut wire = bytes::BytesMut::new();
            for frame in &copy {
                frame.encode(&mut wire).unwrap();
            }
            let mut client = client;
            for byte in wire {
                client.write_all(&[byte]).await.unwrap();
                tokio::task::yield_now().await;
            }
            client.flush().await.unwrap();
        });

        let mut decoded = Vec::new();
        for _ in 0..expected.len() {
            decoded.push(connection.read_frame().await.unwrap().unwrap());
        }
        assert_eq!(decoded, expected);
    }

    #[tokio::test]
    async fn data_frames_stay_ahead_of_their_file_end() {
        let (peer, local) = duplex(8192);
        let (_local_connection, mut outbound) = split_frame_io(local);
        let (mut peer_connection, _peer_outbound) = split_frame_io(peer);
        let file_end = Control::FileEnd(FileEnd {
            index: 0,
            sha256: [7; 32],
        });
        let data_count = DATA_QUEUE_CAPACITY + 2;

        for _ in 0..data_count {
            outbound.send_data(Bytes::from_static(b"d")).await.unwrap();
        }
        outbound.send_control(&file_end).await.unwrap();
        outbound.close().await;

        let mut data_seen = 0;
        let mut last_was_control = false;
        while let Some(frame) = peer_connection.read_frame().await.unwrap() {
            match frame {
                Frame::Data(body) => {
                    assert_eq!(body, Bytes::from_static(b"d"));
                    assert!(!last_was_control, "DATA arrived after a control frame");
                    data_seen += 1;
                }
                Frame::Control(body) => {
                    last_was_control = true;
                    assert_eq!(body, Bytes::from_static(br#"{"type":"file_end","index":0,"sha256":"0707070707070707070707070707070707070707070707070707070707070707"}"#));
                }
            }
        }
        assert_eq!(data_seen, data_count);
        assert!(last_was_control, "file_end was not delivered");
    }

    #[tokio::test]
    async fn flushed_control_resolves_only_after_the_frame_is_written() {
        let (peer, local) = duplex(8192);
        let (_local_connection, outbound) = split_frame_io(local);
        let (mut peer_connection, _peer_outbound) = split_frame_io(peer);

        // The barrier resolves only once the writer flushed the frame, so the
        // peer is guaranteed to observe it before the sender proceeds.
        outbound
            .send_control_flushed(&Control::Ready)
            .await
            .unwrap();
        let frame = peer_connection.read_frame().await.unwrap().unwrap();
        assert_eq!(
            frame.into_body(),
            Bytes::from_static(br#"{"type":"ready"}"#)
        );
    }

    #[tokio::test]
    async fn clean_close_flushes_queued_frames_then_eof() {
        let (peer, local) = duplex(8192);
        let (_local_connection, mut outbound) = split_frame_io(local);
        let (mut peer_connection, _peer_outbound) = split_frame_io(peer);

        outbound.send_control(&Control::Ready).await.unwrap();
        outbound.close().await;

        let frame = peer_connection.read_frame().await.unwrap().unwrap();
        assert_eq!(
            frame.into_body(),
            Bytes::from_static(br#"{"type":"ready"}"#)
        );
        assert_eq!(peer_connection.read_frame().await.unwrap(), None);
        assert_eq!(peer_connection.read_frame().await.unwrap(), None);
    }

    #[tokio::test]
    async fn abrupt_peer_drop_yields_one_eof() {
        let (client, dropped) = duplex(8192);
        let (mut connection, _outbound) = split_frame_io(client);
        drop(dropped);

        assert_eq!(connection.read_frame().await.unwrap(), None);
        assert_eq!(connection.read_frame().await.unwrap(), None);
    }

    #[tokio::test]
    async fn partial_frame_before_close_reports_truncation() {
        let (client, writer) = duplex(8192);
        let (mut connection, _outbound) = split_frame_io(writer);
        let mut wire = bytes::BytesMut::new();
        Frame::control(Bytes::from_static(br#"{"type":"ready"}"#))
            .encode(&mut wire)
            .unwrap();
        tokio::spawn(async move {
            let mut client = client;
            client.write_all(&wire[..wire.len() - 3]).await.unwrap();
            client.flush().await.unwrap();
            client.shutdown().await.unwrap();
        });

        assert!(matches!(
            connection.read_frame().await,
            Err(ReadError::Truncated)
        ));
        assert!(matches!(
            connection.read_frame().await,
            Err(ReadError::Truncated)
        ));
    }

    #[tokio::test]
    async fn outbound_rejects_invalid_data_frames() {
        let (_client, writer) = duplex(8192);
        let (_connection, outbound) = split_frame_io(writer);
        assert_eq!(
            outbound.send_data(Bytes::new()).await,
            Err(SendError::InvalidData)
        );
        assert_eq!(
            outbound
                .send_data(Bytes::from(vec![
                    0;
                    crate::framing::MAX_DATA_BODY_BYTES + 1
                ]))
                .await,
            Err(SendError::InvalidData)
        );
    }

    #[tokio::test]
    async fn full_data_queue_applies_backpressure() {
        let gate = GatedWriter::paused();
        let (_connection, outbound) = split_frame_io(gate);
        let mut accepted = 0;
        for _ in 0..DATA_QUEUE_CAPACITY + 4 {
            match outbound.try_send_data(Bytes::from_static(b"d")) {
                Ok(()) => accepted += 1,
                Err(SendError::Closed) => break,
                Err(other) => panic!("unexpected send error: {other}"),
            }
        }
        // One frame may already be held by the stalled writer task.
        assert!(
            accepted <= DATA_QUEUE_CAPACITY + 1,
            "queue accepted {accepted} frames, exceeding its bound"
        );
    }

    #[tokio::test]
    async fn blocked_send_completes_after_the_writer_drains() {
        let gate = GatedWriter::paused();
        let open = gate.open_handle();
        let (_connection, outbound) = split_frame_io(gate);

        // Fill the queue, let the writer take its in-flight frame and stall,
        // then refill to capacity: one payload is held by the writer and the
        // queue is exactly full, so a further send must block.
        for _ in 0..DATA_QUEUE_CAPACITY {
            assert!(
                outbound.try_send_data(Bytes::from_static(b"d")).is_ok(),
                "initial fill must succeed"
            );
        }
        tokio::task::yield_now().await;
        for _ in 0..DATA_QUEUE_CAPACITY {
            let _ = outbound.try_send_data(Bytes::from_static(b"d"));
        }

        let (done_sender, mut done_receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            outbound.send_data(Bytes::from_static(b"d")).await.unwrap();
            let _ = done_sender.send(());
        });

        tokio::task::yield_now().await;
        assert!(
            done_receiver.try_recv().is_err(),
            "send completed while stalled"
        );

        open.open();
        let mut completed = false;
        for _ in 0..128 {
            tokio::task::yield_now().await;
            if done_receiver.try_recv().is_ok() {
                completed = true;
                break;
            }
        }
        assert!(completed, "send never completed after the writer drained");
    }

    /// A writer that stays paused until [`OpenHandle::open`] is called.
    ///
    /// The waker is stored explicitly and stays registered because poll_write
    /// future registrations would be dropped with the future itself.
    struct GatedWriter {
        paused: Arc<AtomicBool>,
        parked_waker: Arc<std::sync::Mutex<Option<std::task::Waker>>>,
    }

    struct OpenHandle {
        paused: Arc<AtomicBool>,
        parked_waker: Arc<std::sync::Mutex<Option<std::task::Waker>>>,
    }

    impl GatedWriter {
        fn paused() -> Self {
            Self {
                paused: Arc::new(AtomicBool::new(true)),
                parked_waker: Arc::new(std::sync::Mutex::new(None)),
            }
        }

        fn open_handle(&self) -> OpenHandle {
            OpenHandle {
                paused: self.paused.clone(),
                parked_waker: self.parked_waker.clone(),
            }
        }
    }

    impl OpenHandle {
        fn open(&self) {
            self.paused.store(false, Ordering::SeqCst);
            if let Some(waker) = self.parked_waker.lock().unwrap().take() {
                waker.wake();
            }
        }
    }

    impl AsyncWrite for GatedWriter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            if self.paused.load(Ordering::SeqCst) {
                *self.parked_waker.lock().unwrap() = Some(cx.waker().clone());
                return Poll::Pending;
            }
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncRead for GatedWriter {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
}
