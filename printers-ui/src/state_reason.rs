//! Localized printer-state reasons.

use cosmic_settings_printers_core::PrinterEntry;

/// Severity encoded by an RFC 8011 reason suffix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational condition.
    Report,
    /// Warning condition.
    Warning,
    /// Error condition.
    Error,
}

/// A localized printer-state reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reason {
    /// Localized text.
    pub text: String,
    /// Reason severity.
    pub severity: Severity,
}

/// Returns the most severe recognized reason.
pub fn worst(printer: &PrinterEntry) -> Option<Reason> {
    printer
        .option("printer-state-reasons")
        .unwrap_or_default()
        .split(',')
        .filter_map(describe)
        .max_by_key(|reason| reason.severity)
}

fn describe(reason: &str) -> Option<Reason> {
    let (keyword, severity) = split_severity(reason.trim());

    Some(Reason {
        text: wording(&keyword.to_ascii_lowercase())?,
        severity,
    })
}

fn split_severity(reason: &str) -> (&str, Severity) {
    for (suffix, severity) in [
        ("-error", Severity::Error),
        ("-warning", Severity::Warning),
        ("-report", Severity::Report),
    ] {
        if let Some(keyword) = reason.strip_suffix(suffix) {
            return (keyword, severity);
        }
    }

    (reason, Severity::Report)
}

// `none`, `other`, and unknown keywords provide no useful localized message.
fn wording(keyword: &str) -> Option<String> {
    let wording = match keyword {
        // Accept both legacy and current IPP spellings.
        "media-empty" | "media-needed" => fl!("printer-reason-out-of-paper"),
        "media-low" => fl!("printer-reason-paper-low"),
        "media-jam" => fl!("printer-reason-paper-jam"),
        "cover-open" => fl!("printer-reason-cover-open"),
        "door-open" => fl!("printer-reason-door-open"),
        "input-tray-missing" => fl!("printer-reason-tray-missing"),
        "output-area-almost-full" => fl!("printer-reason-output-nearly-full"),
        "output-area-full" => fl!("printer-reason-output-full"),
        "spool-area-full" => fl!("printer-reason-spool-full"),
        "toner-low" => fl!("printer-reason-toner-low"),
        "toner-empty" => fl!("printer-reason-toner-empty"),
        "marker-supply-low" => fl!("printer-reason-supply-low"),
        "marker-supply-empty" => fl!("printer-reason-supply-empty"),
        "marker-waste-almost-full" => fl!("printer-reason-waste-nearly-full"),
        "marker-waste-full" => fl!("printer-reason-waste-full"),
        "offline" => fl!("printer-offline"),
        "paused" => fl!("printer-reason-paused"),
        "shutdown" => fl!("printer-reason-turned-off"),
        "timed-out" => fl!("printer-reason-no-answer"),
        "connecting-to-device" => fl!("printer-reason-connecting"),
        _ => return None,
    };

    Some(wording)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn printer(reasons: &str) -> PrinterEntry {
        PrinterEntry::new(
            "printer",
            "Printer",
            false,
            HashMap::from([("printer-state-reasons".to_string(), reasons.to_string())]),
        )
    }

    #[test]
    fn a_suffix_says_how_much_a_reason_matters() {
        assert_eq!(split_severity("media-jam-error").1, Severity::Error);
        assert_eq!(split_severity("media-low-warning").1, Severity::Warning);
        assert_eq!(split_severity("media-empty-report").1, Severity::Report);
        assert_eq!(split_severity("media-empty").1, Severity::Report);
        assert_eq!(split_severity("media-empty-report").0, "media-empty");
    }

    #[test]
    fn the_worst_reason_is_the_one_shown() {
        let reason = worst(&printer(
            "media-low-report,media-jam-error,toner-low-warning",
        ))
        .unwrap();

        assert_eq!(reason.severity, Severity::Error);
        assert_eq!(reason.text, fl!("printer-reason-paper-jam"));
    }

    #[test]
    fn the_reasons_this_hardware_reports_are_put_into_words() {
        assert_eq!(
            worst(&printer("media-empty-report")).map(|reason| reason.text),
            Some(fl!("printer-reason-out-of-paper"))
        );
        assert_eq!(
            worst(&printer("spool-area-full-report")).map(|reason| reason.text),
            Some(fl!("printer-reason-spool-full"))
        );
    }

    #[test]
    fn a_reason_that_says_nothing_shows_nothing() {
        assert_eq!(worst(&printer("none")), None);
        assert_eq!(worst(&printer("other")), None);
        assert_eq!(worst(&printer("")), None);
    }

    #[test]
    fn an_unknown_keyword_is_not_read_out() {
        assert_eq!(worst(&printer("vendor-thing-broke-error")), None);
    }

    #[test]
    fn a_known_reason_still_shows_beside_an_unknown_one() {
        let reason = worst(&printer("vendor-thing-broke-error,media-jam-error")).unwrap();

        assert_eq!(reason.text, fl!("printer-reason-paper-jam"));
    }
}
