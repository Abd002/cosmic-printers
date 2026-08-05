//! `Create-Printer`: asking one Printer Application to create a printer.
//!
//! This module sends the request and works out what happened. It does not create
//! a destination. The Printer Application creates a printer and advertises it,
//! the ordinary CUPS destination pipeline discovers it, and [`super::reconcile`]
//! ties the two together. A receipt from here is not a destination and must never
//! be turned into one directly.
//!
//! The hardest case is an ambiguous outcome: the request went out, the reply did
//! not come back. The printer may well exist. Retrying would create a second one,
//! so the printer is looked for instead.

use cups_rs::{IppOperation, IppTag, IppValueTag};

use super::client::{OperationCost, PaError, PaRequest, bounded, check_status};
use super::configured_printers::{self, ConfiguredPrinter};

/// The only value PAPPL accepts for `printer-service-type`.
const PRINTER_SERVICE_TYPE: &str = "print";

/// The attribute naming the device to configure.
pub(crate) const DEVICE_URI: &str = "smi55357-device-uri";

/// The attribute naming the driver to use.
pub(crate) const DRIVER: &str = "smi55357-driver";

/// The attribute carrying the device's IEEE-1284 identification.
const DEVICE_ID: &str = "printer-device-id";

/// The attribute carrying a printer's readable description.
const PRINTER_INFO: &str = "printer-info";

/// PAPPL rejects a `printer-name` longer than this.
const MAX_PRINTER_NAME_LENGTH: usize = 127;

/// How many suffixed names to try before giving up on finding a free one.
const MAX_NAME_ATTEMPTS: u32 = 50;

/// What a Printer Application was asked to create.
#[derive(Clone, Debug)]
pub(crate) struct CreatePrinterRequest {
    pub(crate) printer_name: String,
    /// The description shown to a user, kept separate from the queue name.
    pub(crate) printer_info: Option<String>,
    /// The exact URI the owning application reported for this device.
    pub(crate) device_uri: String,
    /// The driver name to use, or `auto` when the application may choose.
    pub(crate) driver: String,
    /// The device ID, passed through so the application can validate the match.
    pub(crate) device_id: Option<String>,
    /// The optional attributes this application said it accepts at creation.
    ///
    /// Anything outside the list is left out. An application that is sent one
    /// refuses the request *and still creates the printer* — verified against
    /// ps-printer-app 20240504-11, which does not list `printer-info` and answered
    /// "Unsupported printer-info textWithoutLanguage value" for a request whose
    /// printer then existed. An empty list carries no information, so the optional
    /// attributes are sent as before.
    pub(crate) accepted_attributes: Vec<String>,
}

impl CreatePrinterRequest {
    fn accepts(&self, attribute: &str) -> bool {
        self.accepted_attributes.is_empty()
            || self
                .accepted_attributes
                .iter()
                .any(|accepted| accepted == attribute)
    }
}

/// How a create attempt ended.
#[derive(Debug)]
pub(crate) enum CreateOutcome {
    /// The printer was created.
    Created {
        printer_name: String,
        printer_uri: Option<String>,
        printer_uuid: Option<String>,
    },
    /// A printer for this device already existed.
    AlreadyConfigured { printer_name: String },
    /// The application refused, and named attributes it could not accept.
    Rejected {
        status: String,
        why: String,
        unsupported_attributes: Vec<String>,
    },
    /// The request was sent and the outcome could not be established.
    ///
    /// Deliberately not retried: the printer may exist.
    UnknownOutcome { printer_name: String, why: String },
}

impl CreateOutcome {
    /// Returns whether the application refused this route to the device rather
    /// than the request itself.
    ///
    /// Only then is another route to the same printer worth trying. A refused
    /// driver, an existing printer, or an outcome that could not be established
    /// would each answer the same way again, or worse.
    pub(crate) fn device_route_was_refused(&self) -> bool {
        match self {
            Self::Rejected {
                unsupported_attributes,
                ..
            } => unsupported_attributes
                .iter()
                .any(|attribute| attribute == DEVICE_URI),
            _ => false,
        }
    }
}

/// Sends `Create-Printer` and establishes what happened.
pub(crate) fn create_printer(
    system_uri: &str,
    request: &CreatePrinterRequest,
) -> Result<CreateOutcome, PaError> {
    // Every operation attribute goes in before the first printer attribute. A
    // request must name each group once, in order; returning to a group already
    // closed writes a second delimiter for it, and a Printer Application reading
    // that answered nothing at all and restarted.
    //
    // The two vendor attributes belong in the operation group: PAPPL checks the
    // group as well as the value tag, and answers "Unsupported
    // smi55357-device-uri uri value" for one sent among the printer attributes —
    // verified against ps-printer-app 20240504-11 by sending the same URI and
    // driver both ways.
    let mut ipp = PaRequest::new(IppOperation::CreatePrinter, system_uri)?
        .string(
            IppTag::Operation,
            IppValueTag::Keyword,
            "printer-service-type",
            PRINTER_SERVICE_TYPE,
        )?
        .string(
            IppTag::Operation,
            IppValueTag::Uri,
            DEVICE_URI,
            &request.device_uri,
        )?
        .string(
            IppTag::Operation,
            IppValueTag::Keyword,
            DRIVER,
            &request.driver,
        )?
        .string(
            IppTag::Printer,
            IppValueTag::Name,
            "printer-name",
            &request.printer_name,
        )?;

    if let Some(device_id) = &request
        .device_id
        .as_deref()
        .filter(|_| request.accepts(DEVICE_ID))
    {
        ipp = ipp.string(IppTag::Printer, IppValueTag::Text, DEVICE_ID, device_id)?;
    }
    if let Some(info) = &request
        .printer_info
        .as_deref()
        .filter(|_| request.accepts(PRINTER_INFO))
    {
        ipp = ipp.string(IppTag::Printer, IppValueTag::Text, PRINTER_INFO, info)?;
    }

    // Sent allowing failure because a rejection names the attribute it could not
    // accept, which is what tells the difference between a driver the application
    // refused and a device URI it could not use.
    let response = match ipp.send_allowing_failure(system_uri, OperationCost::Create) {
        Ok(response) => response,
        Err(PaError::Unreachable { why }) => {
            // The request may have been carried out. Look before retrying.
            return Ok(recover_unknown_outcome(system_uri, request, why));
        }
        Err(error) => return Err(error),
    };

    if let Err(error) = check_status(&response) {
        return match error {
            // A rejection does not mean nothing happened: PAPPL names an
            // unsupported attribute *after* creating the printer, so a refusal can
            // leave a working queue behind. Looking is the only way to know, and it
            // has to happen before any other route is tried or a second printer
            // would appear.
            PaError::Rejected { status, why } => Ok(created_or_rejected(
                system_uri,
                request,
                CreateOutcome::Rejected {
                    status,
                    why,
                    unsupported_attributes: unsupported_attributes(&response),
                },
            )),
            other => Err(other),
        };
    }

    Ok(CreateOutcome::Created {
        printer_name: response
            .find_attribute("printer-name", None)
            .and_then(|attribute| attribute.get_string(0))
            .map(bounded)
            .unwrap_or_else(|| request.printer_name.clone()),
        printer_uri: response
            .find_attribute("printer-uri-supported", None)
            .and_then(|attribute| attribute.get_string(0))
            .map(bounded),
        printer_uuid: response
            .find_attribute("printer-uuid", None)
            .and_then(|attribute| attribute.get_string(0))
            .map(bounded),
    })
}

/// Works out whether a printer was created after the reply was lost.
///
/// Looks by the exact device URI first, because that identifies the device within
/// this application regardless of what the printer ended up being called, then by
/// the requested name. Finding neither means the outcome is genuinely unknown,
/// which is reported as such rather than guessed at.
fn recover_unknown_outcome(
    system_uri: &str,
    request: &CreatePrinterRequest,
    why: String,
) -> CreateOutcome {
    created_or_rejected(
        system_uri,
        request,
        CreateOutcome::UnknownOutcome {
            printer_name: request.printer_name.clone(),
            why,
        },
    )
}

/// Returns the printer this request produced, or `otherwise` if it produced none.
fn created_or_rejected(
    system_uri: &str,
    request: &CreatePrinterRequest,
    otherwise: CreateOutcome,
) -> CreateOutcome {
    let Ok(printers) = configured_printers::get_printers(system_uri) else {
        return otherwise;
    };
    let Some(existing) = configured_printers::find_by_device_uri(&printers, &request.device_uri)
        .or_else(|| configured_printers::find_by_name(&printers, &request.printer_name))
    else {
        return otherwise;
    };

    CreateOutcome::Created {
        printer_name: existing.name.clone(),
        printer_uri: existing.printer_uri.clone(),
        printer_uuid: existing.printer_uuid.clone(),
    }
}

/// Returns the attributes a Printer Application reported it could not accept.
///
/// PAPPL puts the offending attribute in the unsupported group — a refused driver
/// comes back as `smi55357-driver` — which is what distinguishes "wrong driver"
/// from "cannot use this device at all".
fn unsupported_attributes(response: &cups_rs::IppResponse) -> Vec<String> {
    let mut names = response
        .attributes()
        .into_iter()
        .filter(|attribute| attribute.group_tag() == Some(IppTag::UnsupportedGroup))
        .filter_map(|attribute| attribute.name())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();

    names
}

/// Builds a queue name a Printer Application will accept.
///
/// PAPPL requires a name that starts with a letter or underscore, excludes
/// special characters, and stays within 127 characters. The display name is
/// normalized rather than rejected, so a printer called `HP LaserJet 4050`
/// becomes `HP_LaserJet_4050`, and the human-readable form is carried separately
/// in `printer-info`.
pub(crate) fn printer_name_from_display_name(display_name: &str) -> String {
    let mut name = String::new();

    for character in display_name.chars() {
        let mapped = if character.is_ascii_alphanumeric() {
            character
        } else if matches!(character, '-' | '.' | '_') || character.is_whitespace() {
            '_'
        } else {
            // Anything else — a slash, a hash, a control character, a non-ASCII
            // letter — is dropped rather than transliterated, because the goal is
            // a name the application accepts, not a faithful rendering.
            continue;
        };

        // Collapse runs of separators so `Acme   Laser` does not become
        // `Acme___Laser`.
        if mapped == '_' && name.ends_with('_') {
            continue;
        }
        name.push(mapped);
    }

    let name = name.trim_matches('_');
    let mut name = if name.is_empty() {
        "printer".to_string()
    } else {
        name.to_string()
    };

    // Must start with a letter or underscore, so a leading digit gets a prefix
    // rather than being dropped.
    if name.starts_with(|character: char| character.is_ascii_digit()) {
        name.insert(0, '_');
    }

    truncate_name(name)
}

/// Returns a name not already used in this Printer Application.
///
/// A name that is taken by a printer for the *same* device is not a conflict —
/// that is the already-configured case, handled before this is called. Here the
/// name belongs to something else, so a suffix is added.
pub(crate) fn unique_printer_name(base: &str, printers: &[ConfiguredPrinter]) -> Option<String> {
    if configured_printers::find_by_name(printers, base).is_none() {
        return Some(base.to_string());
    }

    (2..=MAX_NAME_ATTEMPTS).find_map(|suffix| {
        let candidate = truncate_name_with_suffix(base, suffix);
        configured_printers::find_by_name(printers, &candidate)
            .is_none()
            .then_some(candidate)
    })
}

fn truncate_name(mut name: String) -> String {
    if name.len() > MAX_PRINTER_NAME_LENGTH {
        name.truncate(MAX_PRINTER_NAME_LENGTH);
    }

    name.trim_end_matches('_').to_string()
}

/// Appends a suffix, trimming the base so the whole name still fits.
fn truncate_name_with_suffix(base: &str, suffix: u32) -> String {
    let suffix = format!("_{suffix}");
    let room = MAX_PRINTER_NAME_LENGTH.saturating_sub(suffix.len());
    let mut name = base.to_string();
    if name.len() > room {
        name.truncate(room);
    }
    name.push_str(&suffix);

    name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(name: &str) -> ConfiguredPrinter {
        ConfiguredPrinter {
            name: name.to_string(),
            device_uri: None,
            printer_uri: None,
            printer_uuid: None,
            web_interface_uri: None,
        }
    }

    #[test]
    fn spaces_become_underscores() {
        assert_eq!(
            printer_name_from_display_name("HP LaserJet 4050"),
            "HP_LaserJet_4050"
        );
    }

    #[test]
    fn characters_a_printer_application_rejects_are_removed() {
        assert_eq!(
            printer_name_from_display_name("Acme/Laser #2 (front)"),
            "AcmeLaser_2_front"
        );
        assert_eq!(
            printer_name_from_display_name("Acme\u{7}Laser"),
            "AcmeLaser"
        );
    }

    #[test]
    fn runs_of_separators_collapse() {
        assert_eq!(
            printer_name_from_display_name("Acme   Test    Laser"),
            "Acme_Test_Laser"
        );
        assert_eq!(printer_name_from_display_name("__Acme__"), "Acme");
    }

    #[test]
    fn a_name_always_starts_with_a_letter_or_underscore() {
        assert_eq!(printer_name_from_display_name("4050 Laser"), "_4050_Laser");
    }

    #[test]
    fn an_unusable_display_name_still_yields_a_name() {
        assert_eq!(printer_name_from_display_name(""), "printer");
        assert_eq!(printer_name_from_display_name("///"), "printer");
        assert_eq!(printer_name_from_display_name("测试打印机"), "printer");
    }

    #[test]
    fn names_stay_within_the_length_a_printer_application_accepts() {
        let name = printer_name_from_display_name(&"a".repeat(300));

        assert_eq!(name.len(), MAX_PRINTER_NAME_LENGTH);
    }

    #[test]
    fn normalization_is_stable() {
        let once = printer_name_from_display_name("HP LaserJet 4050");
        let twice = printer_name_from_display_name(&once);

        assert_eq!(once, twice);
    }

    #[test]
    fn a_free_name_is_used_unchanged() {
        assert_eq!(
            unique_printer_name("HP_LaserJet_4050", &[]).as_deref(),
            Some("HP_LaserJet_4050")
        );
    }

    #[test]
    fn a_name_used_by_another_device_gets_a_suffix() {
        let printers = vec![configured("HP_LaserJet_4050")];

        assert_eq!(
            unique_printer_name("HP_LaserJet_4050", &printers).as_deref(),
            Some("HP_LaserJet_4050_2")
        );

        let printers = vec![
            configured("HP_LaserJet_4050"),
            configured("HP_LaserJet_4050_2"),
        ];
        assert_eq!(
            unique_printer_name("HP_LaserJet_4050", &printers).as_deref(),
            Some("HP_LaserJet_4050_3")
        );
    }

    #[test]
    fn suffixing_keeps_the_name_within_the_length_limit() {
        let base = "a".repeat(MAX_PRINTER_NAME_LENGTH);
        let printers = vec![configured(&base)];
        let name = unique_printer_name(&base, &printers).expect("a unique name");

        assert!(name.len() <= MAX_PRINTER_NAME_LENGTH);
        assert!(name.ends_with("_2"));
    }

    #[test]
    fn giving_up_is_reported_rather_than_looping() {
        let mut printers = vec![configured("printer")];
        printers
            .extend((2..=MAX_NAME_ATTEMPTS).map(|suffix| configured(&format!("printer_{suffix}"))));

        assert_eq!(unique_printer_name("printer", &printers), None);
    }
}
