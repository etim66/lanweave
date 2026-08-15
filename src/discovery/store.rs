use tokio::time::Instant;

use super::{DiscoveredService, DiscoveryEvent, MAX_CANDIDATES, ScopedAddress};
use crate::app::action::DeviceId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    id: DeviceId,
    service_instance: String,
    display_name: String,
    host: String,
    addresses: Vec<ScopedAddress>,
    port: u16,
    last_update: Instant,
}

impl Candidate {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn id(&self) -> DeviceId {
        self.id
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn display_name(&self) -> &str {
        &self.display_name
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn addresses(&self) -> &[ScopedAddress] {
        &self.addresses
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn port(&self) -> u16 {
        self.port
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn last_update(&self) -> Instant {
        self.last_update
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CandidateStore {
    candidates: Vec<Candidate>,
    next_id: u64,
}

impl CandidateStore {
    pub(crate) const fn new() -> Self {
        Self {
            candidates: Vec::new(),
            next_id: 1,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn candidate(&self, id: DeviceId) -> Option<&Candidate> {
        self.candidates.iter().find(|candidate| candidate.id == id)
    }

    pub(crate) fn apply(&mut self, event: DiscoveryEvent) {
        match event {
            DiscoveryEvent::Resolved(service) => self.upsert(service),
            DiscoveryEvent::Removed { service_instance } => {
                self.candidates.retain(|candidate| {
                    !candidate
                        .service_instance
                        .eq_ignore_ascii_case(&service_instance)
                });
            }
        }
    }

    fn upsert(&mut self, service: DiscoveredService) {
        if let Some(candidate) = self.candidates.iter_mut().find(|candidate| {
            candidate
                .service_instance
                .eq_ignore_ascii_case(&service.service_instance)
        }) {
            candidate.service_instance = service.service_instance;
            candidate.display_name = service.display_name;
            candidate.host = service.host;
            candidate.addresses = service.addresses;
            candidate.port = service.port;
            candidate.last_update = service.observed_at;
            return;
        }

        let Some(next_id) = self.next_id.checked_add(1) else {
            return;
        };

        if self.candidates.len() == MAX_CANDIDATES {
            let oldest = self
                .candidates
                .iter()
                .enumerate()
                .min_by(|(_, left), (_, right)| {
                    left.last_update.cmp(&right.last_update).then_with(|| {
                        left.service_instance
                            .to_ascii_lowercase()
                            .cmp(&right.service_instance.to_ascii_lowercase())
                    })
                })
                .map(|(index, _)| index)
                .expect("a full candidate store has an oldest entry");
            self.candidates.swap_remove(oldest);
        }

        self.candidates.push(Candidate {
            id: DeviceId::new(self.next_id),
            service_instance: service.service_instance,
            display_name: service.display_name,
            host: service.host,
            addresses: service.addresses,
            port: service.port,
            last_update: service.observed_at,
        });
        self.next_id = next_id;
    }
}

impl Default for CandidateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;

    use tokio::time::Instant;

    use super::CandidateStore;
    use crate::discovery::{
        DiscoveredService, DiscoveryEvent, InterfaceScope, MAX_CANDIDATES, ScopedAddress,
    };

    fn service(name: &str, address: Ipv4Addr, observed_at: Instant) -> DiscoveredService {
        DiscoveredService::new(
            format!("{name}._lanweave._tcp.local."),
            name.to_owned(),
            format!("{name}.local."),
            vec![ScopedAddress::new(
                IpAddr::V4(address),
                InterfaceScope::new("eth0", 2),
            )],
            4242,
            observed_at,
        )
        .unwrap()
    }

    #[test]
    fn updates_merge_case_insensitively_and_preserve_device_id() {
        let now = Instant::now();
        let mut store = CandidateStore::new();
        store.apply(DiscoveryEvent::Resolved(service(
            "Peer",
            Ipv4Addr::new(192, 0, 2, 1),
            now,
        )));
        let id = store.candidates()[0].id();

        let mut update = service(
            "peer",
            Ipv4Addr::new(192, 0, 2, 2),
            now + Duration::from_secs(1),
        );
        update.service_instance = "peer._LANWEAVE._TCP.LOCAL.".to_owned();
        store.apply(DiscoveryEvent::Resolved(update));

        assert_eq!(store.candidates().len(), 1);
        assert_eq!(store.candidates()[0].id(), id);
        assert_eq!(store.candidate(id).unwrap().display_name(), "peer");
        assert_eq!(
            store.candidate(id).unwrap().service_instance,
            "peer._LANWEAVE._TCP.LOCAL."
        );
        assert_eq!(store.candidate(id).unwrap().host(), "peer.local.");
        assert_eq!(store.candidate(id).unwrap().port(), 4242);
        assert_eq!(
            store.candidate(id).unwrap().last_update(),
            now + Duration::from_secs(1)
        );
        assert_eq!(
            store.candidate(id).unwrap().addresses()[0].interface_index(),
            2
        );
        assert_eq!(
            store.candidates()[0].addresses[0].address(),
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2))
        );
    }

    #[test]
    fn removal_deletes_candidate_and_reappearance_gets_a_new_id() {
        let now = Instant::now();
        let mut store = CandidateStore::new();
        let candidate = service("peer", Ipv4Addr::LOCALHOST, now);
        let key = candidate.service_instance.clone();
        store.apply(DiscoveryEvent::Resolved(candidate.clone()));
        let first_id = store.candidates()[0].id();

        store.apply(DiscoveryEvent::Removed {
            service_instance: key.to_ascii_uppercase(),
        });
        assert!(store.candidates().is_empty());

        store.apply(DiscoveryEvent::Resolved(candidate));
        assert_ne!(store.candidates()[0].id(), first_id);
    }

    #[test]
    fn full_store_evicts_the_oldest_candidate() {
        let now = Instant::now();
        let mut store = CandidateStore::new();

        for index in 0..MAX_CANDIDATES {
            store.apply(DiscoveryEvent::Resolved(service(
                &format!("peer-{index:02}"),
                Ipv4Addr::new(192, 0, 2, index as u8),
                now + Duration::from_secs(index as u64),
            )));
        }
        store.apply(DiscoveryEvent::Resolved(service(
            "new-peer",
            Ipv4Addr::new(198, 51, 100, 1),
            now + Duration::from_secs(MAX_CANDIDATES as u64),
        )));

        assert_eq!(store.candidates().len(), MAX_CANDIDATES);
        assert!(
            store
                .candidates()
                .iter()
                .all(|candidate| candidate.display_name() != "peer-00")
        );
        assert!(
            store
                .candidates()
                .iter()
                .any(|candidate| candidate.display_name() == "new-peer")
        );
    }
}
