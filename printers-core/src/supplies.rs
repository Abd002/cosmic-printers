//! Printer supply levels, colors, and warnings.

use serde::{Deserialize, Serialize};

/// One colour a supply holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct SupplyRgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

/// Which way a supply's level moves as it approaches needing attention.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum SupplyWarningDirection {
    /// Something that is used up: it starts full and needs attention as it empties.
    AtOrBelow,
    /// Something that fills up: it starts empty and needs attention as it fills.
    AtOrAbove,
}

/// The level at which a supply needs attention, and which side of it is bad.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct SupplyWarning {
    pub level_percent: u8,
    pub direction: SupplyWarningDirection,
}

impl SupplyWarning {
    /// Returns whether a level has reached the point of needing attention.
    pub fn is_reached_by(&self, level_percent: u8) -> bool {
        match self.direction {
            SupplyWarningDirection::AtOrBelow => level_percent <= self.level_percent,
            SupplyWarningDirection::AtOrAbove => level_percent >= self.level_percent,
        }
    }
}

/// One supply a printer reports, as the printer describes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct SupplyLevel {
    /// What the printer calls this supply, which is free-form marketing text and
    /// says nothing reliable about how the supply works.
    pub name: String,
    /// Absent when the printer reported no level it knows.
    pub level_percent: Option<u8>,
    /// The colours this supply holds, in the order reported. More than one means one
    /// cartridge holding several inks. Empty when it reports no colour.
    pub colors: Vec<SupplyRgb>,
    /// Absent when the printer did not say where this supply needs attention, which
    /// is the common case: most report no bounds at all.
    pub warning: Option<SupplyWarning>,
}

/// Parses one or more adjacent RGB hex triplets without guessing malformed values.
pub fn parse_supply_colors(value: &str) -> Vec<SupplyRgb> {
    let mut colors = Vec::new();
    let mut rest = value.trim();

    while let Some(digits) = rest.strip_prefix('#') {
        let Some(triplet) = digits.get(..6) else {
            break;
        };
        let channel = |at: usize| u8::from_str_radix(&triplet[at..at + 2], 16).ok();
        let (Some(red), Some(green), Some(blue)) = (channel(0), channel(2), channel(4)) else {
            break;
        };

        colors.push(SupplyRgb { red, green, blue });
        rest = &digits[6..];
    }

    colors
}

/// Converts a reported level to a percentage; negative values mean unknown.
pub fn supply_level_percent(level: i32, high: Option<i32>) -> Option<u8> {
    if level < 0 {
        return None;
    }

    match high {
        Some(high) if high > 100 => {
            Some((i64::from(level) * 100 / i64::from(high)).clamp(0, 100) as u8)
        }
        _ => Some(level.clamp(0, 100) as u8),
    }
}

/// Joins one value per supply, in supply order.
pub(crate) fn join_supply_values(values: impl IntoIterator<Item = String>) -> String {
    values.into_iter().collect::<Vec<_>>().join(",")
}

/// Writes a supply's name so it survives being joined with the others.
pub(crate) fn supply_name(name: &str) -> String {
    name.replace(',', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Writes a supply's colours the way `marker-colors` carries them.
fn format_colors(colors: &[SupplyRgb]) -> String {
    if colors.is_empty() {
        return "none".to_string();
    }

    colors
        .iter()
        .map(|color| format!("#{:02X}{:02X}{:02X}", color.red, color.green, color.blue))
        .collect()
}

/// Preserves queue colors by name, or by position only when array lengths match.
pub(crate) fn merged_colors(
    supplies: &[SupplyLevel],
    names: &[String],
    colors: &[String],
) -> String {
    // Mismatched arrays cannot be aligned safely after comma splitting.
    let names = if names.len() == colors.len() {
        names
    } else {
        &[]
    };
    let position_is_meaningful = colors.len() == supplies.len();

    join_supply_values(supplies.iter().enumerate().map(|(index, supply)| {
        if !supply.colors.is_empty() {
            return format_colors(&supply.colors);
        }

        let wanted = supply.name.trim();
        let at = names
            .iter()
            .position(|reported| !wanted.is_empty() && reported.trim().eq_ignore_ascii_case(wanted))
            .or_else(|| position_is_meaningful.then_some(index));

        match at.and_then(|at| colors.get(at)) {
            Some(reported) if !parse_supply_colors(reported).is_empty() => {
                reported.trim().to_string()
            }
            _ => format_colors(&supply.colors),
        }
    }))
}

/// Writes a bound, or nothing at all where a supply reported none.
pub(crate) fn format_bound(bound: Option<i32>) -> String {
    bound.map(|bound| bound.to_string()).unwrap_or_default()
}

/// Reads where a supply needs attention from the bounds it reports.
pub fn supply_warning(high: Option<i32>, low: Option<i32>) -> Option<SupplyWarning> {
    let (high, low) = (high?, low?);

    if high == 100 && low > 0 && low != 100 {
        return Some(SupplyWarning {
            level_percent: low as u8,
            direction: SupplyWarningDirection::AtOrBelow,
        });
    }

    if low == 0 && high > 0 && high < 100 {
        return Some(SupplyWarning {
            level_percent: high as u8,
            direction: SupplyWarningDirection::AtOrAbove,
        });
    }

    None
}

/// Parses extensible `printer-supply` values, omitting entries without a level.
pub fn parse_printer_supplies(supplies: &[&str], descriptions: &[&str]) -> Vec<SupplyLevel> {
    supplies
        .iter()
        .enumerate()
        .filter_map(|(index, supply)| {
            let mut level = None;
            // Capacity scales the level; it is not a warning threshold.
            let mut capacity = None;
            let mut high = None;
            let mut low = None;
            let mut colorant = None;
            let mut consumed = None;

            for pair in supply.split(';') {
                let Some((key, value)) = pair.split_once('=') else {
                    continue;
                };
                let value = value.trim();

                match key.trim().to_ascii_lowercase().as_str() {
                    "level" => level = value.parse::<i32>().ok(),
                    "maxcapacity" => capacity = value.parse::<i32>().ok(),
                    "highlevel" => high = value.parse::<i32>().ok(),
                    "lowlevel" => low = value.parse::<i32>().ok(),
                    "colorantname" => colorant = Some(value.to_string()),
                    // An explicit class outranks inference from bounds.
                    "class" => {
                        consumed = match value.to_ascii_lowercase().as_str() {
                            "supplythatisconsumed" => Some(true),
                            "receptaclethatisfilled" => Some(false),
                            _ => None,
                        }
                    }
                    _ => {}
                }
            }

            let warning = match consumed {
                Some(true) => low.filter(|low| *low > 0).map(|low| SupplyWarning {
                    level_percent: low.clamp(0, 100) as u8,
                    direction: SupplyWarningDirection::AtOrBelow,
                }),
                Some(false) => high.filter(|high| *high > 0).map(|high| SupplyWarning {
                    level_percent: high.clamp(0, 100) as u8,
                    direction: SupplyWarningDirection::AtOrAbove,
                }),
                None => supply_warning(high, low),
            };

            Some(SupplyLevel {
                name: descriptions
                    .get(index)
                    .map(|description| description.trim().to_string())
                    .filter(|description| !description.is_empty())
                    .or_else(|| colorant.clone())
                    .unwrap_or_default(),
                level_percent: supply_level_percent(level?, capacity),
                colors: colorant
                    .as_deref()
                    .and_then(colorant_color)
                    .into_iter()
                    .collect(),
                warning,
            })
        })
        .collect()
}

/// Infers one unambiguous colour from a free-form supply name.
/// This is a last resort when neither the printer nor its queue reported a colorant.
pub(crate) fn color_named_in(name: &str) -> Option<SupplyRgb> {
    let mut found: Option<SupplyRgb> = None;

    for word in name.split(|character: char| !character.is_ascii_alphanumeric()) {
        let Some(color) = colorant_color(word) else {
            continue;
        };

        match found {
            Some(existing) if existing == color => {}
            Some(_) => return None,
            None => found = Some(color),
        }
    }

    found
}

/// Maps standard CUPS colorant names to their RGB values.
fn colorant_color(name: &str) -> Option<SupplyRgb> {
    let rgb = |red, green, blue| Some(SupplyRgb { red, green, blue });

    match name.trim().to_ascii_lowercase().as_str() {
        "black" | "photoblack" | "matteblack" => rgb(0x00, 0x00, 0x00),
        "cyan" | "process-cyan" => rgb(0x00, 0xFF, 0xFF),
        "magenta" | "process-magenta" => rgb(0xFF, 0x00, 0xFF),
        "yellow" | "process-yellow" => rgb(0xFF, 0xFF, 0x00),
        "lightcyan" | "photocyan" => rgb(0xE0, 0xFF, 0xFF),
        "lightmagenta" | "photomagenta" => rgb(0xFF, 0xE0, 0xFF),
        "lightblack" | "gray" | "grey" | "lightgray" | "lightgrey" => rgb(0x80, 0x80, 0x80),
        "red" => rgb(0xFF, 0x00, 0x00),
        "green" => rgb(0x00, 0xFF, 0x00),
        "blue" => rgb(0x00, 0x00, 0xFF),
        "orange" => rgb(0xFF, 0xA5, 0x00),
        "violet" => rgb(0xEE, 0x82, 0xEE),
        "white" => rgb(0xFF, 0xFF, 0xFF),
        _ => None,
    }
}
