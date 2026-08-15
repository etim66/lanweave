//! Live device discovery over mDNS/DNS-SD.
//!
//! Discovery answers only "where might a Lanweave listener be?". It does not
//! prove identity or authorize a connection. The adapter trait and candidate
//! store live here, and the local listener is bound before advertising.

mod mdns;
mod store;
mod text;

use std::net::IpAddr;

use tokio::sync::mpsc;
use tokio::time::Instant;

pub(crate) use mdns::MdnsDiscoveryService;
pub(crate) use store::{Candidate, CandidateStore};

pub(crate) const SERVICE_TYPE: &str = "_lanweave._tcp.local.";
pub(crate) const DISCOVERY_EVENT_CHANNEL_CAPACITY: usize = 32;
pub(crate) const MAX_CANDIDATES: usize = 64;
pub(crate) const MAX_ENDPOINTS_PER_CANDIDATE: usize = 16;
pub(crate) const MAX_SERVICE_INSTANCE_BYTES: usize = 255;
pub(crate) const MAX_HOST_BYTES: usize = 255;
pub(crate) const MAX_INTERFACE_NAME_BYTES: usize = 64;

pub(crate) type DiscoverySender = mpsc::Sender<DiscoveryEvent>;
pub(crate) type DiscoveryReceiver = mpsc::Receiver<DiscoveryEvent>;

pub(crate) fn event_channel() -> (DiscoverySender, DiscoveryReceiver) {
    mpsc::channel(DISCOVERY_EVENT_CHANNEL_CAPACITY)
}

pub(crate) trait DiscoveryService {
    fn start(&mut self, events: DiscoverySender) -> anyhow::Result<()>;

    async fn stop(&mut self) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiscoveryEvent {
    Resolved(DiscoveredService),
    /// The daemon emits this for both goodbye records and cache expiry.
    Removed {
        service_instance: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredService {
    service_instance: String,
    display_name: String,
    host: String,
    addresses: Vec<ScopedAddress>,
    port: u16,
    observed_at: Instant,
}

impl DiscoveredService {
    fn new(
        service_instance: String,
        display_name: String,
        host: String,
        addresses: Vec<ScopedAddress>,
        port: u16,
        observed_at: Instant,
    ) -> Option<Self> {
        if service_instance.is_empty()
            || service_instance.len() > MAX_SERVICE_INSTANCE_BYTES
            || host.is_empty()
            || host.len() > MAX_HOST_BYTES
            || addresses.is_empty()
            || port == 0
        {
            return None;
        }

        let mut addresses = addresses;
        addresses.sort();
        addresses.dedup();
        addresses.truncate(MAX_ENDPOINTS_PER_CANDIDATE);

        Some(Self {
            service_instance,
            display_name,
            host,
            addresses,
            port,
            observed_at,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(name: &str, observed_at: Instant) -> Self {
        Self::new(
            format!("{name}.{SERVICE_TYPE}"),
            text::escape_display(name),
            format!("{name}.local."),
            vec![ScopedAddress::new(
                IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                InterfaceScope::new("loopback", 1),
            )],
            4242,
            observed_at,
        )
        .expect("test discovery service is valid")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ScopedAddress {
    address: IpAddr,
    interface: InterfaceScope,
}

impl ScopedAddress {
    fn new(address: IpAddr, interface: InterfaceScope) -> Self {
        Self { address, interface }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn address(&self) -> IpAddr {
        self.address
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn interface_index(&self) -> u32 {
        self.interface.index
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct InterfaceScope {
    name: String,
    index: u32,
}

impl InterfaceScope {
    fn new(name: &str, index: u32) -> Self {
        Self {
            name: text::truncate_utf8(&text::escape_display(name), MAX_INTERFACE_NAME_BYTES),
            index,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use tokio::time::Instant;

    use super::{
        DISCOVERY_EVENT_CHANNEL_CAPACITY, DiscoveredService, DiscoveryEvent, DiscoverySender,
        DiscoveryService, InterfaceScope, MAX_ENDPOINTS_PER_CANDIDATE, ScopedAddress,
        event_channel,
    };

    struct FakeDiscoveryService {
        started: bool,
        stopped: bool,
    }

    impl DiscoveryService for FakeDiscoveryService {
        fn start(&mut self, events: DiscoverySender) -> anyhow::Result<()> {
            self.started = true;
            events
                .try_send(DiscoveryEvent::Resolved(DiscoveredService::for_test(
                    "fake",
                    Instant::now(),
                )))
                .map_err(|error| anyhow::anyhow!("fake discovery send failed: {error}"))
        }

        async fn stop(&mut self) -> anyhow::Result<()> {
            self.stopped = true;
            Ok(())
        }
    }

    #[tokio::test]
    async fn fake_adapter_starts_sends_and_stops() {
        let (sender, mut receiver) = event_channel();
        let mut service = FakeDiscoveryService {
            started: false,
            stopped: false,
        };

        service.start(sender).unwrap();
        assert!(matches!(
            receiver.recv().await,
            Some(DiscoveryEvent::Resolved(_))
        ));
        service.stop().await.unwrap();

        assert!(service.started);
        assert!(service.stopped);
    }

    #[test]
    fn discovery_channel_is_bounded() {
        let (sender, _receiver) = event_channel();
        for _ in 0..DISCOVERY_EVENT_CHANNEL_CAPACITY {
            sender
                .try_send(DiscoveryEvent::Removed {
                    service_instance: "peer._lanweave._tcp.local.".to_owned(),
                })
                .unwrap();
        }

        assert!(
            sender
                .try_send(DiscoveryEvent::Removed {
                    service_instance: "peer._lanweave._tcp.local.".to_owned(),
                })
                .is_err()
        );
    }

    #[test]
    fn service_validation_deduplicates_and_bounds_endpoints() {
        let addresses = (0..MAX_ENDPOINTS_PER_CANDIDATE + 5)
            .flat_map(|index| {
                let address = ScopedAddress::new(
                    IpAddr::V4(Ipv4Addr::new(192, 0, 2, index as u8)),
                    InterfaceScope::new("eth0", 2),
                );
                [address.clone(), address]
            })
            .collect();

        let service = DiscoveredService::new(
            format!("peer.{}", super::SERVICE_TYPE),
            "peer".to_owned(),
            "peer.local.".to_owned(),
            addresses,
            4242,
            Instant::now(),
        )
        .unwrap();

        assert_eq!(service.addresses.len(), MAX_ENDPOINTS_PER_CANDIDATE);
    }

    #[test]
    fn service_validation_rejects_unusable_routes() {
        assert!(
            DiscoveredService::new(
                format!("peer.{}", super::SERVICE_TYPE),
                "peer".to_owned(),
                "peer.local.".to_owned(),
                Vec::new(),
                4242,
                Instant::now(),
            )
            .is_none()
        );
    }
}
