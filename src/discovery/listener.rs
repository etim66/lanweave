use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::app::event::AppEvent;
use crate::app::failure::FailureKind;
use crate::app::runtime::EventSender;

const STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// Owns the TCP listener advertised through mDNS.
///
/// Until the framed transport is implemented, accepted sockets are closed
/// without entering the application event loop.
pub(crate) struct LocalListener {
    address: SocketAddr,
    listener: Option<TcpListener>,
    stop: Option<watch::Sender<bool>>,
    task: Option<JoinHandle<()>>,
}

impl LocalListener {
    pub(crate) fn bind() -> anyhow::Result<Self> {
        // An IPv4 wildcard is portable across all target platforms and covers
        // every active IPv4 interface. Scoped IPv6 binding remains a PR 7
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

    pub(crate) const fn port(&self) -> u16 {
        self.address.port()
    }

    pub(crate) fn start(&mut self, events: EventSender) -> anyhow::Result<()> {
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
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else {
                            let _ = events.send(AppEvent::Failed(FailureKind::Connection)).await;
                            return;
                        };
                        drop(stream);
                    }
                }
            }
        });

        self.stop = Some(stop_sender);
        self.task = Some(task);
        Ok(())
    }

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

    #[cfg(test)]
    fn loopback_address(&self) -> SocketAddr {
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), self.address.port())
    }
}

impl Drop for LocalListener {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn bind_ipv4() -> anyhow::Result<TcpListener> {
    let socket = TcpSocket::new_v4()?;
    socket.bind(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0))?;
    Ok(socket.listen(128)?)
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpStream;
    use tokio::sync::mpsc::error::TryRecvError;

    use super::LocalListener;
    use crate::app::runtime::{APP_EVENT_CHANNEL_CAPACITY, event_channel};

    #[tokio::test]
    async fn listener_closes_connections_without_per_connection_events() {
        let (events, mut receiver) = event_channel();
        let mut listener = LocalListener::bind().unwrap();
        assert_ne!(listener.port(), 0);
        let address = listener.loopback_address();
        listener.start(events).unwrap();

        for _ in 0..APP_EVENT_CHANNEL_CAPACITY + 8 {
            let stream = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                TcpStream::connect(address),
            )
            .await
            .unwrap()
            .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(1), stream.readable())
                .await
                .unwrap()
                .unwrap();
            let mut byte = [0];
            assert_eq!(stream.try_read(&mut byte).unwrap(), 0);
        }
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));

        listener.stop().await.unwrap();
        listener.stop().await.unwrap();
    }
}
