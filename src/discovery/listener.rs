use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::app::event::AppEvent;
use crate::app::failure::FailureKind;
use crate::app::runtime::EventSender;

/// Time allowed for the listener task to stop before shutdown is reported.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// Owns the TCP listener advertised through mDNS.
///
/// Accepted sockets are forwarded to the session owner through a bounded
/// channel. When the receiver is gone the socket is dropped, and when the
/// channel is full the accept loop waits, so connection floods apply
/// backpressure instead of growing unbounded work.
pub(crate) struct LocalListener {
    address: SocketAddr,
    listener: Option<TcpListener>,
    stop: Option<watch::Sender<bool>>,
    task: Option<JoinHandle<()>>,
}

impl LocalListener {
    /// Binds a wildcard IPv4 listener on an ephemeral port.
    pub(crate) fn bind() -> anyhow::Result<Self> {
        // An IPv4 wildcard is portable across all target platforms and covers
        // every active IPv4 interface. Scoped IPv6 binding is a future
        // endpoint-policy decision rather than relying on OS dual-stack defaults.
        let listener = bind_ipv4()?;
        let address = listener
            .local_addr()
            .map_err(|error| anyhow::anyhow!("failed to inspect local listener: {error}"))?;

        Ok(Self {
            address,
            listener: Some(listener),
            stop: None,
            task: None,
        })
    }

    /// Returns the bound port advertised through discovery.
    pub(crate) const fn port(&self) -> u16 {
        self.address.port()
    }

    /// Starts accepting connections, reporting failures through `events` and
    /// forwarding accepted sockets through `accepted`.
    ///
    /// Fails when the listener is already running or its socket is gone.
    pub(crate) fn start(
        &mut self,
        events: EventSender,
        accepted: mpsc::Sender<TcpStream>,
    ) -> anyhow::Result<()> {
        if self.task.is_some() {
            anyhow::bail!("local listener is already running");
        }
        let Some(listener) = self.listener.take() else {
            anyhow::bail!("local listener is unavailable");
        };
        let (stop_sender, mut stop_receiver) = watch::channel(false);

        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = stop_receiver.changed() => {
                        if changed.is_err() || *stop_receiver.borrow() {
                            return;
                        }
                    }
                    incoming = listener.accept() => {
                        let Ok((stream, _)) = incoming else {
                            let _ = events.send(AppEvent::Failed(FailureKind::Connection)).await;
                            return;
                        };
                        // A full channel applies backpressure; a closed one
                        // means the session owner is gone, so drop the socket
                        // and keep the accept loop available.
                        let _ = accepted.send(stream).await;
                    }
                }
            }
        });

        self.stop = Some(stop_sender);
        self.task = Some(task);
        Ok(())
    }

    /// Stops accepting connections and joins the accept task.
    ///
    /// Idempotent: stopping twice succeeds and the socket is released.
    pub(crate) async fn stop(&mut self) -> anyhow::Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(task) = self.task.take() {
            tokio::time::timeout(STOP_TIMEOUT, task)
                .await
                .map_err(|_| anyhow::anyhow!("local listener shutdown timed out"))?
                .map_err(|error| anyhow::anyhow!("local listener task failed: {error}"))?;
        }
        self.listener = None;
        Ok(())
    }

    /// Returns the loopback address of the bound port, for tests.
    #[cfg(test)]
    fn loopback_address(&self) -> SocketAddr {
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), self.address.port())
    }
}

impl Drop for LocalListener {
    /// Best-effort cleanup when the listener is dropped without being stopped.
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Binds a wildcard IPv4 TCP socket on an ephemeral port.
fn bind_ipv4() -> anyhow::Result<TcpListener> {
    let socket = TcpSocket::new_v4()?;
    socket.bind(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0))?;
    Ok(socket.listen(128)?)
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpStream;
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::error::TryRecvError;

    use super::LocalListener;
    use crate::app::runtime::{APP_EVENT_CHANNEL_CAPACITY, event_channel};

    #[tokio::test]
    async fn listener_forwards_connections_without_per_connection_events() {
        let (events, mut receiver) = event_channel();
        let (accepted, mut accepted_receiver) = mpsc::channel(APP_EVENT_CHANNEL_CAPACITY);
        let mut listener = LocalListener::bind().unwrap();
        assert_ne!(listener.port(), 0);
        let address = listener.loopback_address();
        listener.start(events, accepted).unwrap();

        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            TcpStream::connect(address),
        )
        .await
        .unwrap()
        .unwrap();
        let forwarded =
            tokio::time::timeout(std::time::Duration::from_secs(1), accepted_receiver.recv())
                .await
                .unwrap()
                .expect("accepted socket must reach the session owner");
        assert_eq!(
            forwarded.peer_addr().unwrap().ip(),
            address.ip(),
            "the forwarded socket belongs to the accepted connection"
        );
        drop(forwarded);
        drop(stream);

        // Ordinary accepts never enter the application event channel.
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));

        listener.stop().await.unwrap();
        listener.stop().await.unwrap();
    }
}
