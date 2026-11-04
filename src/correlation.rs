use std::collections::HashSet;

use sha2::{Digest, Sha256};

use crate::model::{AccessChange, EndpointRole, HumanInputObservation};

#[derive(Debug)]
pub struct CorrelationBuffer {
    correlation_window_ns: i64,
    pending: Vec<AccessChange>,
}

impl CorrelationBuffer {
    pub fn new(correlation_window_ns: i64) -> Self {
        Self {
            correlation_window_ns,
            pending: Vec::new(),
        }
    }

    pub fn ready(
        &mut self,
        changes: impl IntoIterator<Item = AccessChange>,
        observed_monotonic_ns: i64,
    ) -> Vec<HumanInputObservation> {
        self.pending.extend(changes);
        let cutoff = observed_monotonic_ns - self.correlation_window_ns;
        let clients: Vec<usize> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, change)| change.endpoint.role == EndpointRole::Client)
            .map(|(index, _)| index)
            .collect();
        let panes: Vec<usize> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, change)| change.endpoint.role == EndpointRole::Pane)
            .map(|(index, _)| index)
            .collect();
        let mut matches = Vec::new();
        for &client in &clients {
            let Some(pane) = self.unique_nearest(client, &panes) else {
                continue;
            };
            if self.unique_nearest(pane, &clients) != Some(client) {
                continue;
            }
            if self.pending[client]
                .observed_monotonic_ns
                .max(self.pending[pane].observed_monotonic_ns)
                > cutoff
            {
                continue;
            }
            matches.push((client, pane));
        }
        matches.sort_by_key(|(client, pane)| {
            self.pending[*client]
                .observed_monotonic_ns
                .max(self.pending[*pane].observed_monotonic_ns)
        });
        let consumed: HashSet<usize> = matches
            .iter()
            .flat_map(|(client, pane)| [*client, *pane])
            .collect();
        let observations = matches
            .iter()
            .filter_map(|(client, pane)| observation(&self.pending[*client], &self.pending[*pane]))
            .collect();
        let expiry = observed_monotonic_ns - 2 * self.correlation_window_ns;
        self.pending = self
            .pending
            .drain(..)
            .enumerate()
            .filter(|(index, change)| {
                !consumed.contains(index) && change.observed_monotonic_ns > expiry
            })
            .map(|(_, change)| change)
            .collect();
        observations
    }

    pub fn retain(&mut self, endpoint_ids: &HashSet<String>) {
        self.pending
            .retain(|change| endpoint_ids.contains(&change.endpoint.endpoint_id));
    }

    fn unique_nearest(&self, source: usize, candidates: &[usize]) -> Option<usize> {
        let mut nearest = None;
        let mut nearest_delta = i64::MAX;
        let mut tied = false;
        for &candidate in candidates {
            let delta = (self.pending[source].access_time_ns
                - self.pending[candidate].access_time_ns)
                .abs();
            if delta > self.correlation_window_ns {
                continue;
            }
            if delta < nearest_delta {
                nearest = Some(candidate);
                nearest_delta = delta;
                tied = false;
            } else if delta == nearest_delta {
                tied = true;
            }
        }
        if tied { None } else { nearest }
    }
}

fn observation(client: &AccessChange, pane: &AccessChange) -> Option<HumanInputObservation> {
    let workspace_id = pane.endpoint.workspace_id.clone()?;
    let pane_id = pane.endpoint.pane_id.clone()?;
    let identity = format!(
        "{}:{}:{}:{}",
        client.endpoint.endpoint_id,
        client.access_time_ns,
        pane.endpoint.endpoint_id,
        pane.access_time_ns
    );
    let observation_id = hex::encode(Sha256::digest(identity.as_bytes()));
    Some(HumanInputObservation {
        observation_id,
        observed_monotonic_ns: client.observed_monotonic_ns.max(pane.observed_monotonic_ns),
        input_time_ns: client.access_time_ns.max(pane.access_time_ns),
        correlation_delta_ns: (client.access_time_ns - pane.access_time_ns).abs(),
        client_endpoint_id: client.endpoint.endpoint_id.clone(),
        pane_endpoint_id: pane.endpoint.endpoint_id.clone(),
        workspace_id,
        pane_id,
        cwd: pane.endpoint.cwd.clone(),
        provenance: "local-pty-atime-correlation-v1".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TtyEndpoint;
    use std::path::PathBuf;

    fn change(role: EndpointRole, id: &str, atime: i64, observed: i64) -> AccessChange {
        AccessChange {
            endpoint: TtyEndpoint {
                endpoint_id: id.into(),
                role,
                pid: 1,
                device: PathBuf::from("/dev/null"),
                client_id: None,
                workspace_id: (role == EndpointRole::Pane).then(|| "w1".into()),
                pane_id: (role == EndpointRole::Pane).then(|| "p1".into()),
                cwd: None,
            },
            observed_monotonic_ns: observed,
            previous_access_time_ns: atime - 1,
            access_time_ns: atime,
        }
    }

    #[test]
    fn matches_across_poll_boundaries() {
        let mut buffer = CorrelationBuffer::new(150);
        assert!(
            buffer
                .ready([change(EndpointRole::Client, "c", 1000, 10)], 100)
                .is_empty()
        );
        assert_eq!(
            buffer
                .ready([change(EndpointRole::Pane, "p", 1010, 110)], 300)
                .len(),
            1
        );
    }

    #[test]
    fn discards_equal_distance_ambiguity() {
        let mut buffer = CorrelationBuffer::new(150);
        let values = [
            change(EndpointRole::Client, "c", 1000, 10),
            change(EndpointRole::Pane, "p1", 990, 10),
            change(EndpointRole::Pane, "p2", 1010, 10),
        ];
        assert!(buffer.ready(values, 300).is_empty());
    }
}
