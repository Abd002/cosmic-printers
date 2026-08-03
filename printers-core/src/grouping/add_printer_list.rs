//! Groups Printer Application candidates into one Add Printer row per physical printer.

use crate::grouping::evidence::{PhysicalDeviceEvidence, PhysicalIdentityAggregate};

/// A candidate that supplies printer identity fields and a deterministic sorting key.
pub trait PhysicalDeviceObservation {
    /// Returns its device UUID, serial number, MAC address, DNS-SD service, normalized device URI,
    /// network host and port, manufacturer, model, and USB vendor and product IDs.
    fn physical_evidence(&self) -> &PhysicalDeviceEvidence;

    /// Returns the key used to sort candidates before each candidate is tested against groups in
    /// order. The candidate joins the first group for which `can_merge` returns `true`.
    fn grouping_sort_key(&self) -> String;
}

/// Candidates merged into one physical-printer row.
#[derive(Clone, Debug)]
pub struct GroupedDevice<T> {
    /// Every UUID, serial number, MAC address, service, URI, endpoint, manufacturer, model, and
    /// USB ID collected from the candidates.
    pub identity: PhysicalIdentityAggregate,
    /// The candidates, ordered by [`PhysicalDeviceObservation::grouping_sort_key`].
    pub members: Vec<T>,
}

/// Sorts candidates by `grouping_sort_key`, then tests each candidate against existing groups in
/// order. A group is rejected when its UUID, serial-number, or MAC-address set and the candidate's
/// corresponding set are both non-empty with no shared value. Otherwise, a shared UUID, serial
/// number, MAC address, DNS-SD service, or device URI merges the candidate. With none of those
/// matches, the manufacturer sets and model sets must each be empty on one side or contain an equal
/// or substring-related pair. The final requirement is equal host and port, or equal host plus
/// non-empty model sets containing an equal or substring-related pair. The candidate joins the
/// first accepted group; if no group accepts it, a new group is created.pub fn group_by_physical_device<T: PhysicalDeviceObservation>(
pub fn group_by_physical_device<T: PhysicalDeviceObservation>(
    observations: Vec<T>,
) -> Vec<GroupedDevice<T>> {
    let mut sorted = observations;
    sorted.sort_by_key(|observation| observation.grouping_sort_key());

    let mut groups: Vec<GroupedDevice<T>> = Vec::new();

    for observation in sorted {
        let candidate = PhysicalIdentityAggregate::from_evidence(observation.physical_evidence());
        match groups
            .iter_mut()
            .find(|group| group.identity.can_merge(&candidate))
        {
            Some(group) => {
                group.identity.absorb(&candidate);
                group.members.push(observation);
            }
            None => groups.push(GroupedDevice {
                identity: candidate,
                members: vec![observation],
            }),
        }
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grouping::evidence::{normalize_name, normalize_serial};

    struct Observation {
        id: &'static str,
        evidence: PhysicalDeviceEvidence,
    }

    impl PhysicalDeviceObservation for Observation {
        fn physical_evidence(&self) -> &PhysicalDeviceEvidence {
            &self.evidence
        }

        fn grouping_sort_key(&self) -> String {
            self.id.to_string()
        }
    }

    fn observation(id: &'static str, evidence: PhysicalDeviceEvidence) -> Observation {
        Observation { id, evidence }
    }

    fn with_serial(serial: &str) -> PhysicalDeviceEvidence {
        PhysicalDeviceEvidence {
            serial_number: Some(normalize_serial(serial)),
            ..PhysicalDeviceEvidence::default()
        }
    }

    fn with_model(model: &str) -> PhysicalDeviceEvidence {
        PhysicalDeviceEvidence {
            model: Some(normalize_name(model)),
            ..PhysicalDeviceEvidence::default()
        }
    }

    #[test]
    fn transitive_conflict_does_not_over_merge() {
        let mut b = with_serial("1234");
        b.set_network_endpoint("192.0.2.50", Some(9100));
        let mut c = with_serial("9999");
        c.set_network_endpoint("192.0.2.50", Some(9100));

        let groups = group_by_physical_device(vec![
            observation("a", with_serial("1234")),
            observation("b", b),
            observation("c", c),
        ]);

        assert_eq!(groups.len(), 2);
        let mut sizes = groups
            .iter()
            .map(|group| group.members.len())
            .collect::<Vec<_>>();
        sizes.sort_unstable();
        assert_eq!(sizes, vec![1, 2]);
    }

    #[test]
    fn grouping_is_independent_of_observation_order() {
        let build = || {
            let mut b = with_serial("1234");
            b.set_network_endpoint("192.0.2.50", Some(9100));
            let mut c = with_serial("9999");
            c.set_network_endpoint("192.0.2.50", Some(9100));
            vec![
                observation("a", with_serial("1234")),
                observation("b", b),
                observation("c", c),
            ]
        };

        let forward = group_by_physical_device(build());
        let mut reversed_input = build();
        reversed_input.reverse();
        let reversed = group_by_physical_device(reversed_input);

        let keys = |groups: &[GroupedDevice<Observation>]| {
            groups
                .iter()
                .map(|group| {
                    let mut ids = group
                        .members
                        .iter()
                        .map(|member| member.id)
                        .collect::<Vec<_>>();
                    ids.sort_unstable();
                    ids
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(keys(&forward), keys(&reversed));
    }

    #[test]
    fn two_identical_models_without_serials_stay_separate() {
        let groups = group_by_physical_device(vec![
            observation("a", with_model("Test Laser 9000")),
            observation("b", with_model("Test Laser 9000")),
        ]);

        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn removing_an_observation_cannot_create_a_new_merge() {
        let mut bridge = with_serial("1234");
        bridge.set_network_endpoint("192.0.2.50", Some(9100));

        let all = || {
            vec![
                observation("a", with_serial("1234")),
                observation("b", bridge.clone()),
                observation("c", with_serial("9999")),
            ]
        };

        let full = group_by_physical_device(all());
        let without_bridge = group_by_physical_device(
            all()
                .into_iter()
                .filter(|observation| observation.id != "b")
                .collect(),
        );

        assert_eq!(full.len(), 2);
        assert_eq!(without_bridge.len(), 2);
    }
}
