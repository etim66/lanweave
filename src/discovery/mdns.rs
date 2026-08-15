use std::net::IpAddr;
use std::time::Duration;

use mdns_sd::{ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::{
    DiscoveredService, DiscoveryEvent, DiscoverySender, InterfaceScope, MAX_SERVICE_INSTANCE_BYTES,
    SERVICE_TYPE, ScopedAddress,
};

const STOP_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) struct MdnsDiscoveryService {
    daemon: Option<ServiceDaemon>,
    stop: Option<watch::Sender<bool>>,
    task: Option<JoinHandle<()>>,
}

impl MdnsDiscoveryService {
    pub(crate) const fn new() -> Self {
        Self {
            daemon: None,
            stop: None,
            task: None,
        }
    }
}

impl super::DiscoveryService for MdnsDiscoveryService {
    fn start(&mut self, events: DiscoverySender) -> anyhow::Result<()> {
        if self.daemon.is_some() {
            anyhow::bail!("discovery service is already running");
        }

        let daemon = ServiceDaemon::new()
            .map_err(|error| anyhow::anyhow!("failed to initialize discovery: {error}"))?;
        let receiver = match daemon.browse(SERVICE_TYPE) {
            Ok(receiver) => receiver,
            Err(error) => {
                let _ = daemon.shutdown();
                return Err(anyhow::anyhow!("failed to start discovery: {error}"));
            }
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
                    event = receiver.recv_async() => {
                        let Ok(event) = event else {
                            return;
                        };
                        if let Some(event) = convert_event(event, Instant::now()) {
                            tokio::select! {
                                changed = stop_receiver.changed() => {
                                    if changed.is_err() || *stop_receiver.borrow() {
                                        return;
                                    }
                                }
                                result = events.send(event) => {
                                    if result.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        self.daemon = Some(daemon);
        self.stop = Some(stop_sender);
        self.task = Some(task);
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        let Some(daemon) = self.daemon.take() else {
            return Ok(());
        };

        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }

        let stop_browse_result = daemon.stop_browse(SERVICE_TYPE);
        let shutdown_result = daemon.shutdown();

        if let Some(task) = self.task.take() {
            task.await
                .map_err(|error| anyhow::anyhow!("discovery task failed: {error}"))?;
        }

        stop_browse_result
            .map_err(|error| anyhow::anyhow!("failed to stop discovery browse: {error}"))?;
        let shutdown = shutdown_result
            .map_err(|error| anyhow::anyhow!("failed to stop discovery daemon: {error}"))?;
        tokio::time::timeout(STOP_TIMEOUT, shutdown.recv_async())
            .await
            .map_err(|_| anyhow::anyhow!("discovery daemon shutdown timed out"))?
            .map_err(|_| anyhow::anyhow!("discovery daemon stopped without confirmation"))?;
        Ok(())
    }
}

impl Drop for MdnsDiscoveryService {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Some(daemon) = self.daemon.take() {
            let _ = daemon.stop_browse(SERVICE_TYPE);
            let _ = daemon.shutdown();
        }
    }
}

fn convert_event(event: ServiceEvent, observed_at: Instant) -> Option<DiscoveryEvent> {
    match event {
        ServiceEvent::ServiceResolved(service) => {
            normalize_service(&service, observed_at).map(DiscoveryEvent::Resolved)
        }
        ServiceEvent::ServiceRemoved(service_type, service_instance)
            if service_type.eq_ignore_ascii_case(SERVICE_TYPE)
                && !service_instance.is_empty()
                && service_instance.len() <= MAX_SERVICE_INSTANCE_BYTES =>
        {
            Some(DiscoveryEvent::Removed { service_instance })
        }
        _ => None,
    }
}

fn normalize_service(service: &ResolvedService, observed_at: Instant) -> Option<DiscoveredService> {
    if !service.is_valid()
        || !service.ty_domain.eq_ignore_ascii_case(SERVICE_TYPE)
        || !supports_version(service.get_property_val("v"))
    {
        return None;
    }

    let display_name = service_display_name(&service.fullname)?;
    let mut addresses = Vec::new();

    for scoped_ip in &service.addresses {
        match scoped_ip {
            ScopedIp::V4(scoped) => {
                if scoped.interface_ids().is_empty() {
                    addresses.push(ScopedAddress::new(
                        IpAddr::V4(*scoped.addr()),
                        InterfaceScope::new("", 0),
                    ));
                } else {
                    addresses.extend(scoped.interface_ids().iter().map(|interface| {
                        ScopedAddress::new(
                            IpAddr::V4(*scoped.addr()),
                            InterfaceScope::new(&interface.name, interface.index),
                        )
                    }));
                }
            }
            ScopedIp::V6(scoped) => {
                let interface = scoped.scope_id();
                addresses.push(ScopedAddress::new(
                    IpAddr::V6(*scoped.addr()),
                    InterfaceScope::new(&interface.name, interface.index),
                ));
            }
            _ => {}
        }
    }

    DiscoveredService::new(
        service.fullname.clone(),
        super::text::escape_display(display_name),
        super::text::escape_display(&service.host),
        addresses,
        service.port,
        observed_at,
    )
}

fn supports_version(value: Option<Option<&[u8]>>) -> bool {
    value == Some(Some(b"1"))
}

fn service_display_name(fullname: &str) -> Option<&str> {
    if fullname.len() <= SERVICE_TYPE.len() {
        return None;
    }

    let suffix_start = fullname.len() - SERVICE_TYPE.len();
    if !fullname.as_bytes()[suffix_start..].eq_ignore_ascii_case(SERVICE_TYPE.as_bytes()) {
        return None;
    }

    fullname[..suffix_start].strip_suffix('.')
}

#[cfg(test)]
mod tests {
    use mdns_sd::ServiceEvent;
    use tokio::time::Instant;

    use super::{convert_event, service_display_name, supports_version};
    use crate::discovery::{DiscoveryEvent, SERVICE_TYPE};

    #[test]
    fn converts_daemon_removal_for_goodbye_or_cache_expiry() {
        let service_instance = "peer._lanweave._tcp.local.".to_owned();

        assert_eq!(
            convert_event(
                ServiceEvent::ServiceRemoved(SERVICE_TYPE.to_owned(), service_instance.clone()),
                Instant::now(),
            ),
            Some(DiscoveryEvent::Removed { service_instance })
        );
    }

    #[test]
    fn extracts_instance_name_without_service_suffix() {
        assert_eq!(
            service_display_name("Workstation._lanweave._tcp.local."),
            Some("Workstation")
        );
        assert_eq!(
            service_display_name("Workstation._LANWEAVE._TCP.LOCAL."),
            Some("Workstation")
        );
    }

    #[test]
    fn rejects_other_service_types_and_empty_instances() {
        assert_eq!(service_display_name("_lanweave._tcp.local."), None);
        assert_eq!(service_display_name("peer._http._tcp.local."), None);
        assert_eq!(service_display_name("éabcdefghijklmnopqrstuv"), None);
    }

    #[test]
    fn accepts_only_the_exact_version_one_hint() {
        assert!(supports_version(Some(Some(b"1"))));
        assert!(!supports_version(None));
        assert!(!supports_version(Some(None)));
        assert!(!supports_version(Some(Some(b"01"))));
        assert!(!supports_version(Some(Some(&[0xff]))));
    }
}
