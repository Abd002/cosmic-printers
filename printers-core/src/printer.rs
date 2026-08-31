//! Printer destination data shared across service boundaries.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::supplies::{
    SupplyLevel, SupplyWarning, SupplyWarningDirection, color_named_in, format_bound,
    join_supply_values, merged_colors, parse_supply_colors, supply_level_percent, supply_name,
    supply_warning,
};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum PrinterStatus {
    Ready,
    Offline,
    LowToner,
}

/// A configured or discovered CUPS destination.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct PrinterEntry {
    id: String,
    name: String,
    is_default: bool,
    options: HashMap<String, String>,
}

/// Identifies how a printer endpoint was obtained.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointSource {
    Uri,
    Connected,
}

impl PrinterEntry {
    /// Creates a destination from its identity and normalized options.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        is_default: bool,
        options: HashMap<String, String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            is_default,
            options,
        }
    }

    /// Returns the stable CUPS destination identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the display name reported by CUPS or discovery.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns whether a queue exists and the user may administer it.
    pub fn can_administer(&self) -> bool {
        self.option("can-administer") == Some("true")
    }

    /// Returns whether this destination is the one a job goes to by default.
    pub fn is_default(&self) -> bool {
        self.is_default
    }

    /// Records whether this is the effective user default.
    pub fn set_is_default(&mut self, is_default: bool) {
        self.is_default = is_default;
    }

    /// Returns a normalized option by its IPP/CUPS name.
    pub fn option(&self, name: &str) -> Option<&str> {
        self.options
            .get(name)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    /// Iterates normalized options for backend operations.
    #[doc(hidden)]
    pub fn options(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.options
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// Inserts or replaces a normalized option.
    pub fn set_option(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.options.insert(name.into(), value.into());
    }

    #[doc(hidden)]
    pub fn set_endpoint_source(&mut self, source: EndpointSource) {
        self.set_option(
            "endpoint-source",
            match source {
                EndpointSource::Uri => "uri",
                EndpointSource::Connected => "connected",
            },
        );
    }

    #[doc(hidden)]
    pub fn endpoint_source(&self) -> Option<EndpointSource> {
        match self.option("endpoint-source")? {
            "uri" => Some(EndpointSource::Uri),
            "connected" => Some(EndpointSource::Connected),
            _ => None,
        }
    }

    /// Merges normalized options into this destination.
    pub fn merge_options<I>(&mut self, options: I)
    where
        I: IntoIterator<Item = (String, String)>,
    {
        for (name, value) in options {
            if !value.is_empty() {
                self.set_option(name, value);
            }
        }
    }

    /// Merges a partial CUPS enumeration update while retaining an endpoint
    /// previously selected by a successful device connection.
    pub fn merge_enumeration_record(&mut self, incoming: Self) {
        const CONNECTED_ENDPOINT_OPTIONS: &[&str] = &[
            "endpoint-hostname",
            "endpoint-port",
            "endpoint-address",
            "endpoint-is-local",
            "endpoint-source",
            "dnssd-hostname",
            "dnssd-port",
        ];

        let preserve_connected_endpoint = self.endpoint_source() == Some(EndpointSource::Connected);
        if !incoming.name.is_empty() {
            self.name = incoming.name;
        }
        self.is_default = incoming.is_default;
        self.merge_options(incoming.options.into_iter().filter(|(name, _)| {
            !preserve_connected_endpoint || !CONNECTED_ENDPOINT_OPTIONS.contains(&name.as_str())
        }));
    }

    /// Returns the printer service URI reported by the destination.
    pub fn printer_uri(&self) -> Option<&str> {
        self.option("printer-uri-supported")
            .and_then(preferred_printer_uri)
    }

    /// Returns the destination device URI.
    pub fn device_uri(&self) -> Option<&str> {
        self.option("device-uri")
    }

    /// Returns the reachable web URL, falling back to `printer-more-info`.
    pub fn web_page(&self) -> Option<&str> {
        self.option("web-page")
            .or_else(|| self.option("printer-more-info"))
    }

    /// Sets the printer location option.
    pub fn set_location(&mut self, location: impl Into<String>) {
        self.set_option("printer-location", location);
    }

    /// Returns the printer location.
    pub fn location(&self) -> Option<&str> {
        self.option("printer-location")
    }

    /// Returns the printer make and model.
    pub fn model(&self) -> Option<&str> {
        self.option("printer-make-and-model")
    }

    /// Returns the driver version, when reported by the backend.
    pub fn driver_version(&self) -> Option<&str> {
        self.option("printer-driver-version")
    }

    /// Returns the endpoint hostname.
    pub fn hostname(&self) -> Option<&str> {
        self.option("dnssd-hostname")
            .or_else(|| self.option("endpoint-hostname"))
    }

    /// Returns the endpoint port.
    pub fn port(&self) -> Option<u16> {
        self.option("dnssd-port")
            .or_else(|| self.option("endpoint-port"))
            .and_then(|port| port.parse().ok())
    }

    /// Returns whether the endpoint is local without performing DNS resolution.
    pub fn endpoint_is_local(&self) -> bool {
        self.option("endpoint-is-local")
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| self.endpoint_host().is_some_and(crate::host_is_local))
    }

    /// Returns the endpoint, rewriting local hosts to `localhost` for PAPPL authorization.
    pub fn endpoint(&self) -> Option<(String, u16)> {
        let host = self.endpoint_host()?;
        let host = if self.endpoint_is_local() {
            "localhost"
        } else {
            host
        };

        Some((host.to_string(), self.port()?))
    }

    fn endpoint_host(&self) -> Option<&str> {
        self.hostname().or_else(|| self.endpoint_address())
    }

    /// Returns the current operational status.
    pub fn status(&self) -> PrinterStatus {
        if self
            .option_values("printer-state-reasons")
            .iter()
            .any(|reason| reason.contains("toner-low") || reason.contains("toner-empty"))
        {
            return PrinterStatus::LowToner;
        }

        match self.option("printer-state") {
            Some("5") => PrinterStatus::Offline,
            Some("3" | "4") => PrinterStatus::Ready,
            _ => PrinterStatus::Ready,
        }
    }

    /// Returns a status message suitable for a queue row.
    pub fn queue_status(&self) -> Option<&str> {
        self.option("queue-status")
            .or_else(|| self.option("printer-state-message"))
    }

    /// Returns supported media values.
    pub fn paper_sizes(&self) -> Vec<String> {
        self.option_values("media-supported")
    }

    /// Returns supported sides values.
    pub fn print_sides(&self) -> Vec<String> {
        self.option_values("sides-supported")
    }

    /// Returns the effective media value, preferring the user's libcups override.
    pub fn default_paper_size(&self) -> Option<&str> {
        self.option("media")
            .or_else(|| self.option("media-default"))
    }

    /// Sets the media value, as the user's own choice.
    pub fn set_default_paper_size(&mut self, paper_size: impl Into<String>) {
        self.set_option("media", paper_size);
    }

    /// Returns the sides value a job will use, the user's own choice first.
    pub fn default_print_sides(&self) -> Option<&str> {
        self.option("sides")
            .or_else(|| self.option("sides-default"))
    }

    /// Sets the sides value, as the user's own choice.
    pub fn set_default_print_sides(&mut self, print_sides: impl Into<String>) {
        self.set_option("sides", print_sides);
    }

    /// Returns the printer UUID, including aliases used by DNS-SD metadata.
    pub fn printer_uuid(&self) -> Option<&str> {
        self.option("printer-uuid")
            .or_else(|| self.option("uuid"))
            .or_else(|| self.option("UUID"))
    }

    /// Returns a separately reported physical device UUID.
    pub fn device_uuid(&self) -> Option<&str> {
        self.option("device-uuid")
    }

    /// Returns the resolved network address used by grouping.
    pub fn endpoint_address(&self) -> Option<&str> {
        self.option("endpoint-address")
    }

    /// Returns supplies aligned to the level array, leaving missing parallel values empty.
    pub fn supplies(&self) -> Vec<SupplyLevel> {
        let levels = self.aligned_values("marker-levels");
        let colors = self.aligned_values("marker-colors");
        let highs = self.aligned_values("marker-high-levels");
        let lows = self.aligned_values("marker-low-levels");
        let names = self.aligned_values("marker-names");
        // A mismatched name array cannot be aligned safely after comma splitting.
        let names = if names.len() == levels.len() {
            names
        } else {
            Vec::new()
        };

        levels
            .iter()
            .enumerate()
            .map(|(index, level)| {
                let number = |values: &[&str]| {
                    values
                        .get(index)
                        .and_then(|value| value.trim().parse::<i32>().ok())
                };
                let high = number(&highs);

                let name = names.get(index).unwrap_or(&"").trim().to_string();
                let reported = colors
                    .get(index)
                    .map(|value| parse_supply_colors(value))
                    .unwrap_or_default();
                // Infer colors from the name only when the printer reported none.
                let supply_colors = if reported.is_empty() {
                    color_named_in(&name).into_iter().collect()
                } else {
                    reported
                };

                SupplyLevel {
                    name,
                    level_percent: level
                        .trim()
                        .parse::<i32>()
                        .ok()
                        .and_then(|level| supply_level_percent(level, high)),
                    colors: supply_colors,
                    warning: supply_warning(high, number(&lows)),
                }
            })
            .collect()
    }

    /// Stores supplies in queue-style attributes because the option map cannot carry octet strings.
    pub fn set_supplies(&mut self, supplies: &[SupplyLevel]) {
        if supplies.is_empty() {
            return;
        }

        // Preserve reported colors before replacing the parallel attributes.
        let reported_names: Vec<String> = self
            .aligned_values("marker-names")
            .into_iter()
            .map(str::to_string)
            .collect();
        let reported_colors: Vec<String> = self
            .aligned_values("marker-colors")
            .into_iter()
            .map(str::to_string)
            .collect();

        let bounds = supplies.iter().map(|supply| match supply.warning {
            Some(SupplyWarning {
                level_percent,
                direction: SupplyWarningDirection::AtOrBelow,
            }) => (Some(100), Some(i32::from(level_percent))),
            Some(SupplyWarning {
                level_percent,
                direction: SupplyWarningDirection::AtOrAbove,
            }) => (Some(i32::from(level_percent)), Some(0)),
            None => (None, None),
        });
        let (highs, lows): (Vec<_>, Vec<_>) = bounds
            .map(|(high, low)| (format_bound(high), format_bound(low)))
            .unzip();

        self.set_option(
            "marker-levels",
            join_supply_values(supplies.iter().map(|supply| {
                supply
                    .level_percent
                    .map_or_else(|| "-1".to_string(), |level| level.to_string())
            })),
        );
        self.set_option(
            "marker-names",
            join_supply_values(supplies.iter().map(|supply| supply_name(&supply.name))),
        );
        self.set_option(
            "marker-colors",
            merged_colors(supplies, &reported_names, &reported_colors),
        );
        if highs.iter().any(|high| !high.is_empty()) {
            self.set_option("marker-high-levels", join_supply_values(highs));
            self.set_option("marker-low-levels", join_supply_values(lows));
        }
    }

    pub fn option_values(&self, name: &str) -> Vec<String> {
        self.option(name)
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Splits a multi-valued option, keeping empty values where they were.
    fn aligned_values(&self, name: &str) -> Vec<&str> {
        self.option(name)
            .map(|value| value.split(',').collect())
            .unwrap_or_default()
    }

    /// Merges a partial or resolved DNS-SD record into this discovered printer.
    pub fn merge_discovery_record(&mut self, incoming: Self) {
        if self.name.is_empty() {
            self.name = incoming.name;
        }

        self.merge_options(incoming.options);
    }
}

fn preferred_printer_uri(value: &str) -> Option<&str> {
    let mut uris = value
        .split(',')
        .map(str::trim)
        .filter(|uri| !uri.is_empty());
    let first = uris.next()?;

    Some(
        std::iter::once(first)
            .chain(uris)
            .find(|uri| {
                uri.get(..7)
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case("ipps://"))
            })
            .unwrap_or(first),
    )
}

#[cfg(test)]
mod printer_entry_tests {
    use super::*;
    use crate::supplies::{SupplyRgb, parse_printer_supplies};

    fn named_printer(id: &str, name: &str, options: &[(&str, &str)]) -> PrinterEntry {
        PrinterEntry::new(
            id,
            name,
            false,
            options
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        )
    }

    #[test]
    fn discovery_merge_fills_name_and_refreshes_options() {
        let mut existing = named_printer("", "", &[("endpoint-address", "192.0.2.1")]);
        let incoming = named_printer(
            "",
            "Office Printer",
            &[
                ("endpoint-address", "192.0.2.2"),
                ("printer-location", "Office"),
            ],
        );

        existing.merge_discovery_record(incoming);

        assert_eq!(existing.name(), "Office Printer");
        assert_eq!(existing.endpoint_address(), Some("192.0.2.2"));
        assert_eq!(existing.location(), Some("Office"));
    }

    fn printer(options: &[(&str, &str)]) -> PrinterEntry {
        PrinterEntry::new(
            "printer",
            "Printer",
            false,
            options
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        )
    }

    fn rgb(red: u8, green: u8, blue: u8) -> SupplyRgb {
        SupplyRgb { red, green, blue }
    }

    #[test]
    fn reads_four_toners_and_a_waste_box() {
        let printer = printer(&[
            ("marker-colors", "#00FFFF,#FF00FF,#FFFF00,#000000,none"),
            ("marker-high-levels", "100,100,100,100,95"),
            ("marker-levels", "92,92,92,95,0"),
            ("marker-low-levels", "3,3,3,3,0"),
            (
                "marker-names",
                "Cyan TK-5490CS,Magenta TK-5490MS,Yellow TK-5490YS,Black TK-5490KS,Waste Toner Box",
            ),
            ("marker-types", "toner,toner,toner,toner,waste-toner"),
        ]);
        let supplies = printer.supplies();

        assert_eq!(supplies.len(), 5);
        assert_eq!(supplies[0].name, "Cyan TK-5490CS");
        assert_eq!(supplies[0].level_percent, Some(92));
        assert_eq!(
            supplies[0].colors,
            [SupplyRgb {
                red: 0x00,
                green: 0xFF,
                blue: 0xFF
            }]
        );
        assert_eq!(
            supplies[0].warning,
            Some(SupplyWarning {
                level_percent: 3,
                direction: SupplyWarningDirection::AtOrBelow,
            })
        );

        let waste = &supplies[4];
        assert_eq!(waste.name, "Waste Toner Box");
        assert_eq!(waste.level_percent, Some(0));
        assert!(waste.colors.is_empty());
        assert_eq!(
            waste.warning,
            Some(SupplyWarning {
                level_percent: 95,
                direction: SupplyWarningDirection::AtOrAbove,
            })
        );
        assert!(
            !waste
                .warning
                .is_some_and(|warning| warning.is_reached_by(0))
        );
    }

    #[test]
    fn supplies_survive_being_written_and_read_again() {
        let reported = printer(&[
            ("marker-colors", "#00FFFF#FF00FF#FFFF00,#000000,none,none"),
            ("marker-high-levels", "100,100,95,"),
            ("marker-levels", "92,50,10,-1"),
            ("marker-low-levels", "3,3,0,"),
            (
                "marker-names",
                "tri-color cartridge,black cartridge,Waste Toner Box,Printhead Wiper",
            ),
        ])
        .supplies();

        let mut written = printer(&[]);
        written.set_supplies(&reported);

        assert_eq!(written.supplies(), reported);
    }

    #[test]
    fn described_supplies_keep_the_colours_the_queue_reported() {
        let mut printer = printer(&[
            ("marker-colors", "none,#000000,#00FFFF,#FF00FF,#FFFF00"),
            (
                "marker-names",
                "Printhead Wiper,Black Cartridge,Cyan Cartridge,Magenta Cartridge,Yellow Cartridge",
            ),
        ]);
        let described = parse_printer_supplies(
            &[
                "type=cleaner-unit;level=60;",
                "type=ink-cartridge;level=96;",
                "type=ink-cartridge;level=48;",
                "type=ink-cartridge;level=74;",
                "type=ink-cartridge;level=47;",
            ],
            &[
                "Printhead Wiper",
                "Black Cartridge",
                "Cyan Cartridge",
                "Magenta Cartridge",
                "Yellow Cartridge",
            ],
        );

        printer.set_supplies(&described);
        let supplies = printer.supplies();

        assert!(supplies[0].colors.is_empty());
        assert_eq!(supplies[1].colors, [rgb(0x00, 0x00, 0x00)]);
        assert_eq!(supplies[2].colors, [rgb(0x00, 0xFF, 0xFF)]);
        assert_eq!(supplies[3].colors, [rgb(0xFF, 0x00, 0xFF)]);
        assert_eq!(supplies[4].colors, [rgb(0xFF, 0xFF, 0x00)]);
        assert_eq!(supplies[3].level_percent, Some(74));
    }

    #[test]
    fn a_reported_colour_is_matched_by_name_not_by_position() {
        let mut printer = printer(&[
            ("marker-colors", "#FFFF00,#00FFFF"),
            ("marker-names", "Yellow Cartridge,Cyan Cartridge"),
        ]);
        let described = parse_printer_supplies(
            &[
                "type=ink-cartridge;level=20;",
                "type=ink-cartridge;level=30;",
            ],
            &["Cyan Cartridge", "Yellow Cartridge"],
        );

        printer.set_supplies(&described);
        let supplies = printer.supplies();

        assert_eq!(supplies[0].name, "Cyan Cartridge");
        assert_eq!(supplies[0].colors, [rgb(0x00, 0xFF, 0xFF)]);
        assert_eq!(supplies[1].name, "Yellow Cartridge");
        assert_eq!(supplies[1].colors, [rgb(0xFF, 0xFF, 0x00)]);
    }

    #[test]
    fn a_colour_array_of_another_length_is_not_used_by_position() {
        let mut printer = printer(&[("marker-colors", "#00FFFF,#FF00FF,#FFFF00")]);
        let described = parse_printer_supplies(
            &[
                "type=ink-cartridge;level=20;",
                "type=ink-cartridge;level=30;",
            ],
            &["Left Cartridge", "Right Cartridge"],
        );

        printer.set_supplies(&described);

        assert_eq!(printer.option("marker-colors"), Some("none,none"));
    }

    #[test]
    fn a_name_holding_a_comma_keeps_the_rest_of_the_names() {
        let mut written = printer(&[]);
        written.set_supplies(&[
            SupplyLevel {
                name: "Black, high yield".to_string(),
                level_percent: Some(40),
                colors: Vec::new(),
                warning: None,
            },
            SupplyLevel {
                name: "Cyan".to_string(),
                level_percent: Some(60),
                colors: Vec::new(),
                warning: None,
            },
        ]);
        let read = written.supplies();

        assert_eq!(read.len(), 2);
        assert_eq!(read[0].name, "Black high yield");
        assert_eq!(read[1].name, "Cyan");
    }

    #[test]
    fn writing_no_supplies_leaves_what_was_there() {
        let mut printer = printer(&[("marker-levels", "50"), ("marker-names", "Black")]);
        printer.set_supplies(&[]);

        assert_eq!(printer.option("marker-levels"), Some("50"));
    }

    #[test]
    fn a_cartridge_holding_several_inks_reports_each_of_them() {
        let printer = printer(&[
            ("marker-colors", "#00FFFF#FF00FF#FFFF00,#000000"),
            ("marker-high-levels", "100,100"),
            ("marker-levels", "100,50"),
            ("marker-low-levels", "2,2"),
            ("marker-names", "tri-color cartridge,black cartridge"),
        ]);
        let supplies = printer.supplies();

        assert_eq!(supplies.len(), 2);
        assert_eq!(supplies[0].colors.len(), 3);
        assert_eq!(supplies[1].colors.len(), 1);
    }

    #[test]
    fn a_short_array_does_not_shift_the_supplies_after_it() {
        let printer = printer(&[
            ("marker-colors", "#00FFFF,#FF00FF"),
            ("marker-levels", "10,20,30"),
            ("marker-names", "Cyan,Magenta,Yellow"),
        ]);
        let supplies = printer.supplies();

        assert_eq!(supplies.len(), 3);
        assert_eq!(supplies[2].name, "Yellow");
        assert_eq!(supplies[2].level_percent, Some(30));
        assert_eq!(supplies[2].colors, [rgb(0xFF, 0xFF, 0x00)]);
    }

    #[test]
    fn a_supply_with_no_reported_colour_takes_the_one_its_name_names() {
        let printer = printer(&[
            ("marker-colors", "none,none,none,none,none"),
            ("marker-levels", "60,96,48,74,47"),
            (
                "marker-names",
                "Printhead Wiper cleaner-unit,Black Cartridge ink-cartridge S/N:922941333,Cyan Cartridge ink-cartridge S/N:689344159,Magenta Cartridge ink-cartridge S/N:724830724,Yellow Cartridge ink-cartridge S/N:768769241",
            ),
        ]);
        let supplies = printer.supplies();

        assert!(supplies[0].colors.is_empty());
        assert_eq!(supplies[1].colors, [rgb(0x00, 0x00, 0x00)]);
        assert_eq!(supplies[2].colors, [rgb(0x00, 0xFF, 0xFF)]);
        assert_eq!(supplies[3].colors, [rgb(0xFF, 0x00, 0xFF)]);
        assert_eq!(supplies[4].colors, [rgb(0xFF, 0xFF, 0x00)]);
    }

    #[test]
    fn a_name_naming_two_colours_names_none() {
        let printer = printer(&[
            ("marker-colors", "none,none"),
            ("marker-levels", "50,50"),
            ("marker-names", "Cyan and Magenta cartridge,Black cartridge"),
        ]);
        let supplies = printer.supplies();

        assert!(supplies[0].colors.is_empty());
        assert_eq!(supplies[1].colors, [rgb(0x00, 0x00, 0x00)]);
    }

    #[test]
    fn a_reported_colour_is_not_overruled_by_the_name() {
        let printer = printer(&[
            ("marker-colors", "#0000FF"),
            ("marker-levels", "50"),
            ("marker-names", "Cyan cartridge"),
        ]);

        assert_eq!(printer.supplies()[0].colors, [rgb(0x00, 0x00, 0xFF)]);
    }

    #[test]
    fn names_that_do_not_match_the_supply_count_are_left_out() {
        let printer = printer(&[
            ("marker-levels", "10,20"),
            ("marker-names", "Toner Cartridge, Black,Waste Box"),
        ]);
        let supplies = printer.supplies();

        assert_eq!(supplies.len(), 2);
        assert!(supplies.iter().all(|supply| supply.name.is_empty()));
        assert_eq!(supplies[0].level_percent, Some(10));
        assert_eq!(supplies[1].level_percent, Some(20));
    }

    #[test]
    fn an_unreported_level_is_absent_rather_than_empty() {
        let supplies = printer(&[("marker-levels", "-1,-2,0")]).supplies();

        assert_eq!(supplies[0].level_percent, None);
        assert_eq!(supplies[1].level_percent, None);
        assert_eq!(supplies[2].level_percent, Some(0));
    }

    #[test]
    fn a_counted_level_is_read_against_the_top_reported() {
        let printer = printer(&[("marker-high-levels", "512"), ("marker-levels", "256")]);

        assert_eq!(printer.supplies()[0].level_percent, Some(50));
    }

    #[test]
    fn bounds_that_describe_neither_kind_of_supply_mark_nothing() {
        let consumable = supply_warning(Some(100), Some(3));
        assert_eq!(
            consumable.map(|warning| warning.direction),
            Some(SupplyWarningDirection::AtOrBelow)
        );
        assert_eq!(
            supply_warning(Some(95), Some(0)).map(|warning| warning.direction),
            Some(SupplyWarningDirection::AtOrAbove)
        );

        assert_eq!(supply_warning(Some(100), Some(0)), None);
        assert_eq!(supply_warning(Some(100), Some(100)), None);
        assert_eq!(supply_warning(Some(0), Some(0)), None);
        assert_eq!(supply_warning(Some(3), Some(100)), None);
        assert_eq!(supply_warning(None, None), None);
        assert_eq!(supply_warning(Some(100), None), None);
    }

    #[test]
    fn a_colour_it_cannot_read_names_no_colour() {
        assert_eq!(parse_supply_colors("none"), []);
        assert_eq!(parse_supply_colors(""), []);
        assert_eq!(parse_supply_colors("#12345"), []);
        assert_eq!(
            parse_supply_colors("#00ffff"),
            [SupplyRgb {
                red: 0,
                green: 255,
                blue: 255
            }]
        );
        assert_eq!(parse_supply_colors("#00FFFF junk").len(), 1);
    }

    #[test]
    fn reads_the_supplies_a_printer_reports_for_itself() {
        let supplies = parse_printer_supplies(
            &[
                "index=1;class=supplyThatIsConsumed;type=toner;unit=percent;maxcapacity=100;level=92;lowlevel=3;colorantname=cyan;",
                "index=2;class=receptacleThatIsFilled;type=wasteToner;unit=percent;maxcapacity=100;level=0;highlevel=95;colorantname=unknown;",
            ],
            &["Cyan TK-5490CS"],
        );

        assert_eq!(supplies.len(), 2);
        assert_eq!(supplies[0].name, "Cyan TK-5490CS");
        assert_eq!(supplies[0].level_percent, Some(92));
        assert_eq!(
            supplies[0].colors,
            [SupplyRgb {
                red: 0x00,
                green: 0xFF,
                blue: 0xFF
            }]
        );
        assert_eq!(
            supplies[0].warning,
            Some(SupplyWarning {
                level_percent: 3,
                direction: SupplyWarningDirection::AtOrBelow,
            })
        );

        assert_eq!(supplies[1].name, "unknown");
        assert!(supplies[1].colors.is_empty());
        assert_eq!(
            supplies[1].warning,
            Some(SupplyWarning {
                level_percent: 95,
                direction: SupplyWarningDirection::AtOrAbove,
            })
        );
    }

    #[test]
    fn a_capacity_is_not_a_point_of_attention() {
        let supplies = parse_printer_supplies(
            &[
                "index=1;class=receptacleThatIsFilled;type=wasteToner;unit=percent;maxcapacity=100;level=25;colorantname=unknown;",
                "index=2;class=supplyThatIsConsumed;type=toner;unit=percent;maxcapacity=100;level=75;colorantname=black;",
            ],
            &["Toner Waste Tank", "Black Toner"],
        );

        assert_eq!(supplies.len(), 2);
        assert_eq!(supplies[0].name, "Toner Waste Tank");
        assert_eq!(supplies[0].level_percent, Some(25));
        assert_eq!(supplies[0].warning, None);

        assert_eq!(supplies[1].name, "Black Toner");
        assert_eq!(supplies[1].level_percent, Some(75));
        assert_eq!(supplies[1].warning, None);
        assert_eq!(
            supplies[1].colors,
            [SupplyRgb {
                red: 0,
                green: 0,
                blue: 0
            }]
        );
    }

    #[test]
    fn a_named_colorant_gives_the_bar_its_colour() {
        let supplies = parse_printer_supplies(
            &[
                "index=1;class=supplyThatIsConsumed;type=ink;unit=percent;maxcapacity=100;level=50;colorantname=cyan;",
                "index=2;class=supplyThatIsConsumed;type=ink;unit=percent;maxcapacity=100;level=33;colorantname=magenta;",
                "index=3;class=supplyThatIsConsumed;type=ink;unit=percent;maxcapacity=100;level=67;colorantname=yellow;",
                "index=4;class=supplyThatIsConsumed;type=ink;unit=percent;maxcapacity=100;level=10;colorantname=fuchsia;",
            ],
            &[],
        );

        let colors = supplies
            .iter()
            .map(|supply| supply.colors.first().copied())
            .collect::<Vec<_>>();

        assert_eq!(
            colors,
            [
                Some(SupplyRgb {
                    red: 0x00,
                    green: 0xFF,
                    blue: 0xFF
                }),
                Some(SupplyRgb {
                    red: 0xFF,
                    green: 0x00,
                    blue: 0xFF
                }),
                Some(SupplyRgb {
                    red: 0xFF,
                    green: 0xFF,
                    blue: 0x00
                }),
                None,
            ]
        );
    }

    #[test]
    fn a_supply_reporting_no_level_is_left_out() {
        let supplies = parse_printer_supplies(&["index=1;type=toner;maxcapacity=100;"], &[]);

        assert!(supplies.is_empty());
    }

    #[test]
    fn normalizes_printer_uuid_aliases() {
        assert_eq!(
            printer(&[("printer-uuid", "urn:uuid:standard")]).printer_uuid(),
            Some("urn:uuid:standard")
        );
        assert_eq!(printer(&[("uuid", "lower")]).printer_uuid(), Some("lower"));
        assert_eq!(printer(&[("UUID", "upper")]).printer_uuid(), Some("upper"));
    }

    #[test]
    fn returns_printer_more_info_as_web_page() {
        assert_eq!(
            printer(&[("printer-more-info", "https://printer.local/")]).web_page(),
            Some("https://printer.local/")
        );
    }

    #[test]
    fn optional_metadata_can_be_absent() {
        let printer = printer(&[("device-uri", "ipps://printer._ipps._tcp.local/")]);

        assert_eq!(printer.printer_uuid(), None);
        assert_eq!(printer.web_page(), None);
    }

    #[test]
    fn secure_printer_uri_is_preferred_from_supported_values() {
        let printer = printer(&[(
            "printer-uri-supported",
            "ipp://host:8889/ipp/print,ipps://host:8889/ipp/print",
        )]);

        assert_eq!(printer.printer_uri(), Some("ipps://host:8889/ipp/print"));
    }

    #[test]
    fn enumeration_preserves_connected_endpoint() {
        let mut existing = printer(&[
            ("endpoint-hostname", "printer.local"),
            ("endpoint-port", "8000"),
            ("endpoint-is-local", "true"),
            ("endpoint-source", "connected"),
        ]);
        let incoming = printer(&[
            ("endpoint-hostname", "printer._ipps._tcp.local"),
            ("endpoint-port", "631"),
            ("printer-location", "Office"),
        ]);

        existing.merge_enumeration_record(incoming);

        assert_eq!(existing.hostname(), Some("printer.local"));
        assert_eq!(existing.port(), Some(8000));
        assert_eq!(existing.endpoint_address(), None);
        assert_eq!(existing.endpoint_source(), Some(EndpointSource::Connected));
        assert_eq!(existing.location(), Some("Office"));
    }
}
