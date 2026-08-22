use std::collections::VecDeque;
use std::net::IpAddr;
use std::time::Duration;

use mdns_sd::{
    DaemonEvent, DaemonStatus, IfKind, ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent,
    ServiceInfo, UnregisterStatus,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::{
    DiscoveredService, DiscoveryEvent, DiscoverySender, InterfaceScope, MAX_SERVICE_INSTANCE_BYTES,
    SERVICE_TYPE, ScopedAddress,
};

/// Time allowed for daemon shutdown steps before they are reported as failures.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
/// Maximum number of own service names tracked as conflict aliases.
const MAX_OWN_SERVICE_NAMES: usize = 32;

/// mDNS/DNS-SD adapter backed by the `mdns-sd` daemon.
///
/// Advertises the local listener, browses for peers, and relays peer events
/// onto the discovery channel while filtering out own advertisements.
pub(crate) struct MdnsDiscoveryService {
    daemon: Option<ServiceDaemon>,
    registered_fullname: Option<String>,
    stop: Option<watch::Sender<bool>>,
    task: Option<JoinHandle<()>>,
}

impl MdnsDiscoveryService {
    /// Creates a stopped service with no daemon yet.
    pub(crate) const fn new() -> Self {
        Self {
            daemon: None,
            registered_fullname: None,
            stop: None,
            task: None,
        }
    }
}

impl super::DiscoveryService for MdnsDiscoveryService {
    /// Starts the daemon, registers the local advertisement, and begins browsing.
    ///
    /// Fails without side effects when the daemon cannot initialize, browse,
    /// or register. IPv6 discovery is disabled so only IPv4 peers appear.
    fn start(&mut self, events: DiscoverySender, listener_port: u16) -> anyhow::Result<()> {
        if self.daemon.is_some() {
            anyhow::bail!("discovery service is already running");
        }
        if listener_port == 0 {
            anyhow::bail!("cannot advertise an unbound listener");
        }

        let service = local_service(listener_port)?;
        let registered_fullname = service.get_fullname().to_owned();
        let daemon = ServiceDaemon::new()
            .map_err(|error| anyhow::anyhow!("failed to initialize discovery: {error}"))?;
        daemon.disable_interface(IfKind::IPv6).map_err(|error| {
            let _ = daemon.shutdown();
            anyhow::anyhow!("failed to apply the IPv4 discovery policy: {error}")
        })?;
        let monitor = match daemon.monitor() {
            Ok(monitor) => monitor,
            Err(error) => {
                let _ = daemon.shutdown();
                return Err(anyhow::anyhow!(
                    "failed to monitor discovery name conflicts: {error}"
                ));
            }
        };
        let receiver = match daemon.browse(SERVICE_TYPE) {
            Ok(receiver) => receiver,
            Err(error) => {
                let _ = daemon.shutdown();
                return Err(anyhow::anyhow!("failed to start discovery: {error}"));
            }
        };
        if let Err(error) = daemon.register(service) {
            let _ = daemon.stop_browse(SERVICE_TYPE);
            let _ = daemon.shutdown();
            return Err(anyhow::anyhow!(
                "failed to advertise the local listener: {error}"
            ));
        }
        let (stop_sender, mut stop_receiver) = watch::channel(false);
        let task_registered_fullname = registered_fullname.clone();

        let task = tokio::spawn(async move {
            let mut own_fullnames = VecDeque::from([task_registered_fullname.clone()]);
            let mut monitor_open = true;

            loop {
                tokio::select! {
                    changed = stop_receiver.changed() => {
                        if changed.is_err() || *stop_receiver.borrow() {
                            return;
                        }
                    }
                    event = monitor.recv_async(), if monitor_open => {
                        match event {
                            Ok(event) => record_own_name_change(
                                &mut own_fullnames,
                                &task_registered_fullname,
                                &event,
                            ),
                            Err(_) => monitor_open = false,
                        }
                    }
                    event = receiver.recv_async() => {
                        let Ok(event) = event else {
                            return;
                        };
                        while let Ok(event) = monitor.try_recv() {
                            record_own_name_change(
                                &mut own_fullnames,
                                &task_registered_fullname,
                                &event,
                            );
                        }
                        if event_fullname(&event)
                            .is_some_and(|name| is_own_fullname(&own_fullnames, name))
                        {
                            continue;
                        }
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
        self.registered_fullname = Some(registered_fullname);
        self.stop = Some(stop_sender);
        self.task = Some(task);
        Ok(())
    }

    /// Stops the daemon, unregistering the advertisement and confirming shutdown.
    ///
    /// Every step is attempted even when an earlier one fails; the first
    /// error is reported.
    async fn stop(&mut self) -> anyhow::Result<()> {
        let Some(daemon) = self.daemon.take() else {
            return Ok(());
        };

        let mut first_error = None;

        if let Some(fullname) = self.registered_fullname.take() {
            let unregister_result = match daemon.unregister(&fullname) {
                Ok(receiver) => {
                    match tokio::time::timeout(STOP_TIMEOUT, receiver.recv_async()).await {
                        Ok(Ok(UnregisterStatus::OK | UnregisterStatus::NotFound)) => Ok(()),
                        Ok(Err(_)) => Err(anyhow::anyhow!(
                            "discovery daemon stopped without unregister confirmation"
                        )),
                        Err(_) => Err(anyhow::anyhow!("discovery unregister timed out")),
                    }
                }
                Err(error) => Err(anyhow::anyhow!(
                    "failed to unregister local advertisement: {error}"
                )),
            };
            remember_first_error(&mut first_error, unregister_result);
        }

        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }

        let stop_browse_result = daemon.stop_browse(SERVICE_TYPE);
        let shutdown_result = daemon.shutdown();

        if let Some(task) = self.task.take()
            && let Err(error) = task.await
        {
            remember_first_error(
                &mut first_error,
                Err(anyhow::anyhow!("discovery task failed: {error}")),
            );
        }

        remember_first_error(
            &mut first_error,
            stop_browse_result
                .map_err(|error| anyhow::anyhow!("failed to stop discovery browse: {error}")),
        );
        let shutdown_result = match shutdown_result {
            Ok(shutdown) => match tokio::time::timeout(STOP_TIMEOUT, shutdown.recv_async()).await {
                Ok(Ok(DaemonStatus::Shutdown)) => Ok(()),
                Ok(Ok(_)) => Err(anyhow::anyhow!(
                    "discovery daemon returned an unexpected shutdown status"
                )),
                Ok(Err(_)) => Err(anyhow::anyhow!(
                    "discovery daemon stopped without confirmation"
                )),
                Err(_) => Err(anyhow::anyhow!("discovery daemon shutdown timed out")),
            },
            Err(error) => Err(anyhow::anyhow!("failed to stop discovery daemon: {error}")),
        };
        remember_first_error(&mut first_error, shutdown_result);

        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for MdnsDiscoveryService {
    /// Best-effort cleanup when the service is dropped without being stopped.
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Some(daemon) = self.daemon.take() {
            if let Some(fullname) = self.registered_fullname.take() {
                let _ = daemon.unregister(&fullname);
            }
            let _ = daemon.stop_browse(SERVICE_TYPE);
            let _ = daemon.shutdown();
        }
    }
}

/// Builds the local advertisement for the given listener port.
fn local_service(port: u16) -> anyhow::Result<ServiceInfo> {
    let label = local_dns_label();
    service_info(&label, &format!("{label}.local."), port)
}

/// Builds a version-one advertisement with the given instance and hostname.
fn service_info(instance: &str, hostname: &str, port: u16) -> anyhow::Result<ServiceInfo> {
    ServiceInfo::new(
        SERVICE_TYPE,
        instance,
        hostname,
        "",
        port,
        &[("v", "1")][..],
    )
    .map(ServiceInfo::enable_addr_auto)
    .map_err(|error| anyhow::anyhow!("failed to build local advertisement: {error}"))
}

/// Derives a run-specific DNS label from the host name.
fn local_dns_label() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "lanweave".to_owned());
    dns_label(&host, fastrand::u64(..))
}

/// Sanitizes `host` into a DNS label and appends `nonce` for uniqueness.
fn dns_label(host: &str, nonce: u64) -> String {
    let mut label = String::with_capacity(48);
    let mut previous_was_dash = false;

    for character in host.chars() {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_alphanumeric() {
            label.push(character);
            previous_was_dash = false;
        } else if !previous_was_dash && !label.is_empty() {
            label.push('-');
            previous_was_dash = true;
        }
        if label.len() == 40 {
            break;
        }
    }
    while label.ends_with('-') {
        label.pop();
    }
    if label.is_empty() {
        label.push_str("lanweave");
    }
    // A per-run nonce reduces ordinary conflicts; daemon name-change events are
    // still tracked because peers can deliberately claim an advertised name.
    format!("{label}-{nonce:016x}")
}

/// Returns the fullname an event refers to, when it names a service.
fn event_fullname(event: &ServiceEvent) -> Option<&str> {
    match event {
        ServiceEvent::ServiceResolved(service) => Some(&service.fullname),
        ServiceEvent::ServiceRemoved(_, fullname) => Some(fullname),
        _ => None,
    }
}

/// Records a daemon name-conflict alias of our own advertisement.
///
/// Only changes that rename the registered service are tracked, and the
/// alias list never exceeds [`MAX_OWN_SERVICE_NAMES`] entries.
fn record_own_name_change(
    own_fullnames: &mut VecDeque<String>,
    registered_fullname: &str,
    event: &DaemonEvent,
) {
    let DaemonEvent::NameChange(change) = event else {
        return;
    };
    if !change.original.eq_ignore_ascii_case(registered_fullname)
        || is_own_fullname(own_fullnames, &change.new_name)
    {
        return;
    }

    if own_fullnames.len() == MAX_OWN_SERVICE_NAMES {
        // Preserve the original unregister key and discard the oldest alias.
        own_fullnames.remove(1);
    }
    own_fullnames.push_back(change.new_name.clone());
}

/// Returns whether `fullname` is one of our own advertised names.
fn is_own_fullname(own_fullnames: &VecDeque<String>, fullname: &str) -> bool {
    own_fullnames
        .iter()
        .any(|own| own.eq_ignore_ascii_case(fullname))
}

/// Keeps the first error while later cleanup steps still run.
fn remember_first_error(first: &mut Option<anyhow::Error>, result: anyhow::Result<()>) {
    if first.is_none()
        && let Err(error) = result
    {
        *first = Some(error);
    }
}

/// Converts a daemon browse event into a discovery event.
///
/// Resolved services are validated and normalized; removals are forwarded
/// only when they match the Lanweave service type and a sane instance name.
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

/// Validates a resolved service and maps its scoped addresses.
///
/// Returns `None` when the service is invalid, is not the Lanweave service
/// type, or does not advertise a supported protocol version.
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

/// Returns whether the advertised protocol version is supported.
fn supports_version(value: Option<Option<&[u8]>>) -> bool {
    value == Some(Some(b"1"))
}

/// Extracts the instance name from a fullname, without the service suffix.
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
    use std::collections::VecDeque;

    use mdns_sd::{DaemonEvent, DnsNameChange, RRType, ServiceEvent};
    use tokio::time::Instant;

    use super::{
        MAX_OWN_SERVICE_NAMES, convert_event, dns_label, event_fullname, is_own_fullname,
        record_own_name_change, service_display_name, service_info, supports_version,
    };
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

    #[test]
    fn advertisement_uses_the_listener_port_and_only_version_one() {
        let service = service_info("workstation-1", "workstation-1.local.", 4567).unwrap();

        assert_eq!(service.get_type(), SERVICE_TYPE);
        assert_eq!(service.get_port(), 4567);
        assert_eq!(service.get_property_val("v"), Some(Some(b"1".as_slice())));
        assert_eq!(service.get_properties().iter().count(), 1);
        assert!(service.is_addr_auto());
    }

    #[test]
    fn name_conflict_aliases_remain_filtered_and_bounded() {
        let registered = "workstation-1._lanweave._tcp.local.";
        let mut own_fullnames = VecDeque::from([registered.to_owned()]);

        for index in 2..=MAX_OWN_SERVICE_NAMES + 4 {
            let renamed = format!("workstation-1 ({index})._lanweave._tcp.local.");
            let event = DaemonEvent::NameChange(DnsNameChange {
                original: registered.to_owned(),
                new_name: renamed.clone(),
                rr_type: RRType::SRV,
                intf_name: "eth0".to_owned(),
            });
            record_own_name_change(&mut own_fullnames, registered, &event);
            record_own_name_change(&mut own_fullnames, registered, &event);
            assert!(is_own_fullname(&own_fullnames, &renamed));
        }

        assert_eq!(own_fullnames.len(), MAX_OWN_SERVICE_NAMES);
        assert!(is_own_fullname(&own_fullnames, registered));
        let latest = format!(
            "workstation-1 ({})._lanweave._tcp.local.",
            MAX_OWN_SERVICE_NAMES + 4
        );
        let removal = ServiceEvent::ServiceRemoved(SERVICE_TYPE.to_owned(), latest);
        assert!(event_fullname(&removal).is_some_and(|name| is_own_fullname(&own_fullnames, name)));
    }

    #[test]
    fn hostname_conflict_changes_do_not_become_service_aliases() {
        let registered = "workstation-1._lanweave._tcp.local.";
        let mut own_fullnames = VecDeque::from([registered.to_owned()]);
        let event = DaemonEvent::NameChange(DnsNameChange {
            original: "workstation-1.local.".to_owned(),
            new_name: "workstation-1-2.local.".to_owned(),
            rr_type: RRType::A,
            intf_name: "eth0".to_owned(),
        });

        record_own_name_change(&mut own_fullnames, registered, &event);

        assert_eq!(own_fullnames.len(), 1);
        assert!(is_own_fullname(&own_fullnames, registered));
    }

    #[test]
    fn local_dns_labels_are_bounded_sanitized_and_run_specific() {
        let first = dns_label(" Workstation # 1 ", 1);
        let second = dns_label(" Workstation # 1 ", 2);

        assert_eq!(first, "workstation-1-0000000000000001");
        assert_eq!(second, "workstation-1-0000000000000002");
        assert_ne!(first, second);
        assert!(dns_label(&"x".repeat(100), u64::MAX).len() <= 63);
        assert_eq!(dns_label("---", 0), "lanweave-0000000000000000");
    }
}
