//! IEEE-1284 device ID parsing.
//!
//! A device ID is the string a printer reports about itself, for example
//! `MFG:Acme;MDL:Test Laser 9000;CMD:POSTSCRIPT,PJL;SN:ABC123;`. Printer
//! Applications hand it back verbatim from `PAPPL-Find-Devices`, and it is the
//! main source of physical-device identity evidence.
//!
//! Real devices produce malformed IDs, so parsing never fails: unusable input
//! yields a [`DeviceId`] with no fields and the original string preserved.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

/// Keys whose values are free text and may legitimately contain a semicolon.
///
/// A semicolon normally ends a field, so a fragment with no `KEY:` prefix is
/// ambiguous. Continuing only these keys recovers descriptions like
/// `DES:Acme Printer; the fast one;` without letting stray input corrupt an
/// identifying field such as `SN`.
const CONTINUABLE_KEYS: &[&str] = &["DES", "DESCRIPTION", "COMMENT"];

/// A parsed IEEE-1284 device ID.
///
/// Field lookup is case-insensitive and tolerates the common key spellings.
/// The original string is always retained, because a Printer Application needs
/// the exact value back when asked for matching drivers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceId {
    raw: String,
    fields: BTreeMap<String, Vec<String>>,
}

impl DeviceId {
    /// Parses a device ID string.
    ///
    /// Unparsable input is not an error; the result simply has no fields.
    pub fn parse(raw: &str) -> Self {
        let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut last_key: Option<String> = None;

        for token in raw.split(';') {
            if token.trim().is_empty() {
                last_key = None;
                continue;
            }

            match token.split_once(':') {
                Some((key, value)) => {
                    let key = normalize_key(key);
                    if key.is_empty() {
                        last_key = None;
                        continue;
                    }
                    fields
                        .entry(key.clone())
                        .or_default()
                        .push(value.trim().to_string());
                    last_key = Some(key);
                }
                // A fragment with no key: rejoin it to the previous value when
                // that value is free text, otherwise discard it. Discarding is
                // deliberate: a corrupted identifier must not be able to make
                // two different devices look alike.
                None => {
                    let Some(key) = last_key
                        .as_deref()
                        .filter(|key| CONTINUABLE_KEYS.contains(key))
                    else {
                        last_key = None;
                        continue;
                    };
                    if let Some(values) = fields.get_mut(key)
                        && let Some(value) = values.last_mut()
                    {
                        value.push(';');
                        value.push_str(token);
                    }
                }
            }
        }

        Self {
            raw: raw.to_string(),
            fields,
        }
    }

    /// Parses a device ID from bytes that may not be valid UTF-8.
    pub fn parse_bytes(raw: &[u8]) -> Self {
        Self::parse(&String::from_utf8_lossy(raw))
    }

    /// Returns the original device ID string, unchanged.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Returns true when nothing usable could be parsed.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Returns the first non-empty value for a key, trying each alias in order.
    fn first(&self, aliases: &[&str]) -> Option<&str> {
        aliases.iter().find_map(|alias| {
            self.fields
                .get(&normalize_key(alias))?
                .iter()
                .map(String::as_str)
                .find(|value| !value.is_empty())
        })
    }

    /// Returns a field by key, accepting any capitalization.
    pub fn field(&self, key: &str) -> Option<&str> {
        self.first(&[key])
    }

    /// Returns the reported manufacturer.
    pub fn manufacturer(&self) -> Option<&str> {
        self.first(&["MFG", "MANUFACTURER"])
    }

    /// Returns the reported model.
    pub fn model(&self) -> Option<&str> {
        self.first(&["MDL", "MODEL"])
    }

    /// Returns the reported serial number.
    pub fn serial_number(&self) -> Option<&str> {
        self.first(&["SN", "SERN", "SERIALNUMBER"])
    }

    /// Returns the reported device class.
    pub fn class(&self) -> Option<&str> {
        self.first(&["CLS", "CLASS"])
    }

    /// Returns the reported description.
    pub fn description(&self) -> Option<&str> {
        self.first(&["DES", "DESCRIPTION"])
    }

    /// Returns the supported command sets, normalized for comparison.
    ///
    /// Command sets are comma-separated within the field, and every reported
    /// spelling is folded to upper case so `postscript` and `POSTSCRIPT` compare
    /// equal.
    pub fn command_sets(&self) -> BTreeSet<String> {
        ["CMD", "COMMAND SET", "COMMANDSET"]
            .iter()
            .filter_map(|alias| self.fields.get(&normalize_key(alias)))
            .flatten()
            .flat_map(|value| value.split(','))
            .map(|value| value.trim().to_ascii_uppercase())
            .filter(|value| !value.is_empty())
            .collect()
    }
}

/// Normalizes a key for lookup: trimmed, upper case, internal runs of
/// whitespace collapsed to one space so `COMMAND  SET` matches `COMMAND SET`.
fn normalize_key(key: &str) -> String {
    key.split_whitespace()
        .map(|word| word.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_complete_device_id() {
        let id = DeviceId::parse("MFG:Acme;MDL:Test Laser 9000;CMD:POSTSCRIPT,PJL,PCL;SN:ABC123;");

        assert_eq!(id.manufacturer(), Some("Acme"));
        assert_eq!(id.model(), Some("Test Laser 9000"));
        assert_eq!(id.serial_number(), Some("ABC123"));
        assert_eq!(
            id.command_sets(),
            BTreeSet::from([
                "PCL".to_string(),
                "PJL".to_string(),
                "POSTSCRIPT".to_string()
            ])
        );
    }

    #[test]
    fn preserves_the_original_string_exactly() {
        let raw = "  mfg:Acme ; MDL:X ";
        assert_eq!(DeviceId::parse(raw).raw(), raw);
    }

    #[test]
    fn recognizes_keys_case_insensitively_and_by_alias() {
        let id = DeviceId::parse(
            "manufacturer:Acme;model:Test Laser;serialnumber:S1;command set:postscript;",
        );

        assert_eq!(id.manufacturer(), Some("Acme"));
        assert_eq!(id.model(), Some("Test Laser"));
        assert_eq!(id.serial_number(), Some("S1"));
        assert_eq!(
            id.command_sets(),
            BTreeSet::from(["POSTSCRIPT".to_string()])
        );
    }

    #[test]
    fn collapses_whitespace_inside_keys() {
        let id = DeviceId::parse("COMMAND   SET:PCL;");
        assert_eq!(id.command_sets(), BTreeSet::from(["PCL".to_string()]));
    }

    #[test]
    fn tolerates_a_missing_final_semicolon_and_reordering() {
        let id = DeviceId::parse("SN:S1;MDL:Test Laser;MFG:Acme");

        assert_eq!(id.manufacturer(), Some("Acme"));
        assert_eq!(id.model(), Some("Test Laser"));
        assert_eq!(id.serial_number(), Some("S1"));
    }

    #[test]
    fn rejoins_semicolons_inside_a_description() {
        let id = DeviceId::parse("DES:Acme Printer; the fast one;MDL:Test Laser;");

        assert_eq!(id.description(), Some("Acme Printer; the fast one"));
        assert_eq!(id.model(), Some("Test Laser"));
    }

    #[test]
    fn discards_stray_fragments_after_an_identifying_field() {
        let id = DeviceId::parse("SN:S1;garbage;MDL:Test Laser;");

        assert_eq!(id.serial_number(), Some("S1"));
        assert_eq!(id.model(), Some("Test Laser"));
    }

    #[test]
    fn keeps_the_first_usable_value_of_a_duplicated_key() {
        let id = DeviceId::parse("MDL:;MDL:Test Laser;MDL:Other;");
        assert_eq!(id.model(), Some("Test Laser"));
    }

    #[test]
    fn ignores_empty_and_keyless_input() {
        assert!(DeviceId::parse("").is_empty());
        assert!(DeviceId::parse(";;;").is_empty());
        assert!(DeviceId::parse("no-colon-here").is_empty());
        assert!(DeviceId::parse(":value;").is_empty());
    }

    #[test]
    fn accepts_non_ascii_model_names() {
        let id = DeviceId::parse("MFG:Acmé;MDL:Tëst Läser 9000;");

        assert_eq!(id.manufacturer(), Some("Acmé"));
        assert_eq!(id.model(), Some("Tëst Läser 9000"));
    }

    #[test]
    fn recovers_from_invalid_utf8_without_panicking() {
        let id = DeviceId::parse_bytes(b"MFG:Ac\xffme;MDL:Test;");

        assert_eq!(id.model(), Some("Test"));
        assert!(id.manufacturer().is_some());
    }

    #[test]
    fn treats_whitespace_only_values_as_absent() {
        let id = DeviceId::parse("MFG:   ;MDL:Test;");

        assert_eq!(id.manufacturer(), None);
        assert_eq!(id.model(), Some("Test"));
    }

    #[test]
    fn keeps_colons_inside_values() {
        let id = DeviceId::parse("DES:host:port;MDL:Test;");

        assert_eq!(id.description(), Some("host:port"));
        assert_eq!(id.model(), Some("Test"));
    }
}
