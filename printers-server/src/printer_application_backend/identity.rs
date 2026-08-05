//! Turning raw observations into Add Printer rows.
//!
//! Two collapses happen, in order, and they are not the same operation.
//!
//! First, within one Printer Application: an application can report the same
//! printer over USB, DNS-SD, and a raw socket, which is one candidate with three
//! endpoints, not three candidates. Only strong identity may collapse these —
//! matching model names are not enough, because two identical printers on one
//! desk look identical that way.
//!
//! Second, across applications: candidates from different applications that
//! describe the same hardware become one row listing each application. Nothing
//! crosses ownership here — each candidate keeps its own application's device
//! URIs and driver match.

use cosmic_settings_printers_core::{
    PhysicalDeviceEvidence, PhysicalDeviceObservation, PhysicalIdentityAggregate,
    group_by_physical_device,
};

use super::devices::{DeviceTransport, PaDeviceObservation};
use super::drivers::PaDriverMatch;

/// One way to reach a device, as reported by one Printer Application.
///
/// The device URI identifies an endpoint on its own — it is the value the
/// application gave and the value it will be handed back — so there is no
/// separate identifier to keep in step with it.
#[derive(Clone, Debug)]
pub(crate) struct PaDeviceEndpoint {
    /// The exact URI the owning application reported.
    pub(crate) device_uri: String,
    pub(crate) transport: DeviceTransport,
    /// Lower is preferred. Derived from the transport.
    pub(crate) preference: i32,
}

/// One Printer Application's offer to configure one physical printer.
#[derive(Clone, Debug)]
pub(crate) struct PaConfigurationCandidate {
    pub(crate) id: String,
    pub(crate) printer_application_id: String,
    /// Every way this application can reach the device, best first.
    pub(crate) endpoints: Vec<PaDeviceEndpoint>,
    pub(crate) identity: PhysicalDeviceEvidence,
    pub(crate) display_name: String,
    pub(crate) make_and_model: Option<String>,
    /// The device ID this application reported, replayed verbatim when matching
    /// drivers.
    pub(crate) device_id: Option<String>,
    pub(crate) driver_match: PaDriverMatch,
}

impl PaConfigurationCandidate {
    /// Returns the endpoint to try first.
    pub(crate) fn preferred_endpoint(&self) -> Option<&PaDeviceEndpoint> {
        self.endpoints.first()
    }

    /// Returns the endpoints to try, in order, if the first one fails.
    ///
    /// Every endpoint here belongs to this candidate, so a fallback stays within
    /// one application and one physical printer. Falling back to another
    /// application would silently configure through software the user did not
    /// choose.
    pub(crate) fn fallback_endpoints(&self) -> &[PaDeviceEndpoint] {
        self.endpoints.get(1..).unwrap_or_default()
    }
}

impl PhysicalDeviceObservation for PaConfigurationCandidate {
    fn physical_evidence(&self) -> &PhysicalDeviceEvidence {
        &self.identity
    }

    fn grouping_sort_key(&self) -> String {
        // Sorting on the candidate id keeps grouping independent of the order
        // applications answered in. The id starts with the application id, so
        // candidates from one application stay together.
        self.id.clone()
    }
}

/// Collapses one application's observations into candidates.
///
/// An observation joins an existing candidate only on strong identity. Weaker
/// agreement is deliberately not enough: within a single application, two
/// observations that share only a model name are more likely to be two printers
/// than two views of one.
pub(crate) fn collapse_observations(
    observations: Vec<PaDeviceObservation>,
) -> Vec<PaConfigurationCandidate> {
    let mut candidates: Vec<(PhysicalIdentityAggregate, PaConfigurationCandidate)> = Vec::new();

    for observation in observations {
        let incoming = PhysicalIdentityAggregate::from_evidence(&observation.identity);
        let existing = candidates
            .iter_mut()
            .find(|(identity, _)| identity.agrees_strongly_with(&incoming));

        match existing {
            Some((identity, candidate)) => {
                identity.absorb(&incoming);
                add_endpoint(candidate, observation);
            }
            None => candidates.push((incoming, new_candidate(observation))),
        }
    }

    let mut candidates = candidates
        .into_iter()
        .map(|(_, candidate)| candidate)
        .collect::<Vec<_>>();
    for candidate in &mut candidates {
        candidate.endpoints.sort_by(|left, right| {
            left.preference
                .cmp(&right.preference)
                .then_with(|| left.device_uri.cmp(&right.device_uri))
        });
    }
    candidates.sort_by(|left, right| left.id.cmp(&right.id));

    candidates
}

fn new_candidate(observation: PaDeviceObservation) -> PaConfigurationCandidate {
    let make_and_model = observation.device_id.as_ref().and_then(|device_id| {
        let name = [device_id.manufacturer(), device_id.model()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        (!name.trim().is_empty()).then_some(name)
    });

    PaConfigurationCandidate {
        id: observation.id.clone(),
        printer_application_id: observation.printer_application_id.clone(),
        endpoints: vec![endpoint(&observation)],
        identity: observation.identity.clone(),
        display_name: observation.display_name.clone(),
        make_and_model,
        device_id: observation.raw_device_id().map(ToString::to_string),
        driver_match: PaDriverMatch::Unchecked,
    }
}

/// Adds another route to an existing candidate, filling in anything it lacked.
fn add_endpoint(candidate: &mut PaConfigurationCandidate, observation: PaDeviceObservation) {
    if !candidate
        .endpoints
        .iter()
        .any(|existing| existing.device_uri == observation.device_uri)
    {
        candidate.endpoints.push(endpoint(&observation));
    }

    if candidate.device_id.is_none() {
        candidate.device_id = observation.raw_device_id().map(ToString::to_string);
    }
    if candidate.identity.serial_number.is_none() {
        candidate.identity.serial_number = observation.identity.serial_number.clone();
    }
    if candidate.identity.device_uuid.is_none() {
        candidate.identity.device_uuid = observation.identity.device_uuid.clone();
    }
}

fn endpoint(observation: &PaDeviceObservation) -> PaDeviceEndpoint {
    PaDeviceEndpoint {
        device_uri: observation.device_uri.clone(),
        transport: observation.transport,
        preference: observation.transport.preference(),
    }
}

/// One physical printer and every application that offered to configure it.
#[derive(Debug)]
pub(crate) struct PhysicalPrinter {
    /// Stable within a discovery generation, so adding another application's
    /// candidate does not renumber the row.
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) make_and_model: Option<String>,
    pub(crate) identity: PhysicalIdentityAggregate,
    pub(crate) candidates: Vec<PaConfigurationCandidate>,
}

/// Groups candidates from every application by physical printer.
pub(crate) fn group_candidates(candidates: Vec<PaConfigurationCandidate>) -> Vec<PhysicalPrinter> {
    let mut printers = group_by_physical_device(candidates)
        .into_iter()
        .map(|group| {
            let identity = group.identity;
            let candidates = group.members;

            PhysicalPrinter {
                id: physical_printer_id(&identity, &candidates),
                display_name: printer_display_name(&candidates),
                make_and_model: candidates
                    .iter()
                    .find_map(|candidate| candidate.make_and_model.clone()),
                identity,
                candidates,
            }
        })
        .collect::<Vec<_>>();

    printers.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then_with(|| left.id.cmp(&right.id))
    });

    printers
}

/// Builds the row identifier.
///
/// Strong evidence gives an id that survives another application reporting the
/// same printer later in the same generation. With nothing strong to key on, the
/// first candidate's id is used, which is stable because grouping is
/// order-independent.
fn physical_printer_id(
    identity: &PhysicalIdentityAggregate,
    candidates: &[PaConfigurationCandidate],
) -> String {
    identity
        .stable_key()
        .map(|key| format!("physical:{key}"))
        .unwrap_or_else(|| {
            candidates
                .first()
                .map(|candidate| format!("physical:candidate:{}", candidate.id))
                .unwrap_or_else(|| "physical:unknown".to_string())
        })
}

/// Chooses the name for a row shared by several applications.
///
/// A name derived from the device's own make and model is preferred over one an
/// application invented, so the row reads the same however many applications
/// found it.
fn printer_display_name(candidates: &[PaConfigurationCandidate]) -> String {
    candidates
        .iter()
        .find_map(|candidate| candidate.make_and_model.clone())
        .or_else(|| {
            candidates
                .iter()
                .map(|candidate| candidate.display_name.clone())
                .min()
        })
        .unwrap_or_else(|| "Unknown printer".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_settings_printers_core::DeviceId;

    fn observation(
        application: &str,
        index: usize,
        device_uri: &str,
        device_id: Option<&str>,
    ) -> PaDeviceObservation {
        let device_id = device_id.map(DeviceId::parse);
        // The same derivation the scan uses, so a change to what a device URI
        // says about identity is exercised here rather than only in production.
        let identity = super::super::devices::evidence(device_id.as_ref(), device_uri);

        PaDeviceObservation {
            id: format!("{application}:1:{index}"),
            printer_application_id: application.to_string(),
            device_uri: device_uri.to_string(),
            device_id,
            display_name: device_uri.to_string(),
            identity,
            transport: super::super::devices::DeviceTransport::from_uri(device_uri),
        }
    }

    #[test]
    fn one_printer_reached_two_ways_is_one_candidate_with_two_endpoints() {
        let device_id = "MFG:Acme;MDL:Test Laser 9000;SN:ABC123;";
        let candidates = collapse_observations(vec![
            observation("pa-a", 0, "socket://192.0.2.10:9100", Some(device_id)),
            observation("pa-a", 1, "usb://Acme/Test%20Laser%209000", Some(device_id)),
        ]);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].endpoints.len(), 2);
        // USB is the more direct route to a printer that is physically here.
        assert_eq!(
            candidates[0]
                .preferred_endpoint()
                .map(|endpoint| endpoint.transport),
            Some(DeviceTransport::Usb)
        );
        assert_eq!(candidates[0].fallback_endpoints().len(), 1);
    }

    #[test]
    fn the_same_model_with_different_serials_stays_two_candidates() {
        let candidates = collapse_observations(vec![
            observation(
                "pa-a",
                0,
                "socket://192.0.2.10:9100",
                Some("MFG:Acme;MDL:Test Laser;SN:FIRST;"),
            ),
            observation(
                "pa-a",
                1,
                "socket://192.0.2.11:9100",
                Some("MFG:Acme;MDL:Test Laser;SN:SECOND;"),
            ),
        ]);

        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn the_same_model_without_a_serial_stays_two_candidates() {
        let candidates = collapse_observations(vec![
            observation(
                "pa-a",
                0,
                "socket://192.0.2.10:9100",
                Some("MFG:Acme;MDL:Test Laser;"),
            ),
            observation(
                "pa-a",
                1,
                "socket://192.0.2.11:9100",
                Some("MFG:Acme;MDL:Test Laser;"),
            ),
        ]);

        assert_eq!(candidates.len(), 2);
    }

    /// What Gutenprint and the PostScript application really report for one
    /// DNS-SD printer: the same service under two device schemes, spelled with
    /// percent escapes in one and DNS-SD escapes in the other, and carrying the
    /// printer's UUID on only one of them.
    #[test]
    fn one_service_reported_under_two_schemes_is_one_candidate() {
        let device_id = "MFG:4BARCODE;MDL:4B-2054A;CMD:CEZD;";
        let candidates = collapse_observations(vec![
            observation(
                "pa-a",
                0,
                "cups:dnssd://Fake%20Arkscan%202054A%20%40%20pop-os._pdl-datastream._tcp.local/?uuid=6f42764b-8be5-4b23-842e-f36be28fa103",
                Some(device_id),
            ),
            observation(
                "pa-a",
                1,
                "dnssd://Fake%5C032Arkscan%5C0322054A%5C032%5C064%5C032pop-os._pdl-datastream._tcp.local/",
                Some(device_id),
            ),
        ]);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].endpoints.len(), 2);
        // The CUPS backend route is tried first; both URIs stay exactly as the
        // application wrote them.
        assert_eq!(
            candidates[0]
                .preferred_endpoint()
                .map(|endpoint| endpoint.device_uri.as_str()),
            Some(
                "cups:dnssd://Fake%20Arkscan%202054A%20%40%20pop-os._pdl-datastream._tcp.local/?uuid=6f42764b-8be5-4b23-842e-f36be28fa103"
            )
        );
    }

    #[test]
    fn a_repeated_observation_does_not_add_a_second_endpoint() {
        let device_id = "MFG:Acme;MDL:Test Laser;SN:ABC123;";
        let candidates = collapse_observations(vec![
            observation("pa-a", 0, "socket://192.0.2.10:9100", Some(device_id)),
            observation("pa-a", 1, "socket://192.0.2.10:9100", Some(device_id)),
        ]);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].endpoints.len(), 1);
    }

    #[test]
    fn two_applications_finding_one_printer_produce_one_row() {
        let device_id = "MFG:Acme;MDL:Test Laser 9000;SN:ABC123;";
        let candidates = [
            collapse_observations(vec![observation(
                "pa-a",
                0,
                "socket://192.0.2.10:9100",
                Some(device_id),
            )]),
            collapse_observations(vec![observation(
                "pa-b",
                0,
                "hp:/net/Test_Laser?ip=192.0.2.10",
                Some(device_id),
            )]),
        ]
        .concat();

        let printers = group_candidates(candidates);

        assert_eq!(printers.len(), 1);
        assert_eq!(printers[0].candidates.len(), 2);
        assert_eq!(
            printers[0].make_and_model.as_deref(),
            Some("Acme Test Laser 9000")
        );
        // Each application keeps its own URI; nothing is rewritten across the
        // ownership boundary.
        let uris = printers[0]
            .candidates
            .iter()
            .filter_map(|candidate| candidate.preferred_endpoint())
            .map(|endpoint| endpoint.device_uri.as_str())
            .collect::<Vec<_>>();
        assert!(uris.contains(&"socket://192.0.2.10:9100"));
        assert!(uris.contains(&"hp:/net/Test_Laser?ip=192.0.2.10"));
    }

    #[test]
    fn row_identity_is_stable_when_another_application_joins() {
        let device_id = "MFG:Acme;MDL:Test Laser;SN:ABC123;";
        let first = collapse_observations(vec![observation(
            "pa-a",
            0,
            "socket://192.0.2.10:9100",
            Some(device_id),
        )]);
        let alone = group_candidates(first.clone());

        let joined = group_candidates(
            [
                first,
                collapse_observations(vec![observation(
                    "pa-b",
                    0,
                    "hp:/net/Test_Laser",
                    Some(device_id),
                )]),
            ]
            .concat(),
        );

        assert_eq!(alone[0].id, joined[0].id);
    }

    #[test]
    fn grouping_is_independent_of_application_order() {
        let device_id = "MFG:Acme;MDL:Test Laser;SN:ABC123;";
        let build = || {
            [
                collapse_observations(vec![observation(
                    "pa-a",
                    0,
                    "socket://192.0.2.10:9100",
                    Some(device_id),
                )]),
                collapse_observations(vec![observation(
                    "pa-b",
                    0,
                    "hp:/net/Test_Laser",
                    Some(device_id),
                )]),
            ]
            .concat()
        };

        let forward = group_candidates(build());
        let mut input = build();
        input.reverse();
        let reversed = group_candidates(input);

        assert_eq!(forward.len(), reversed.len());
        assert_eq!(forward[0].id, reversed[0].id);
        assert_eq!(forward[0].candidates.len(), reversed[0].candidates.len());
    }
}
