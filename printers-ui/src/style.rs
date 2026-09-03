//! Printer-page colors and dimensions.

use cosmic::iced::Color;

/// A printer that is ready to take work.
pub fn status_ready() -> Color {
    cosmic::theme::active().cosmic().palette.bright_green.into()
}

/// A printer that is working.
pub fn status_printing() -> Color {
    cosmic::theme::active().cosmic().accent_color().into()
}

/// A printer that has stopped and wants attention.
pub fn status_stopped() -> Color {
    cosmic::theme::active()
        .cosmic()
        .palette
        .bright_orange
        .into()
}

/// Something that has gone wrong, such as a job that failed.
pub fn error() -> Color {
    cosmic::theme::active().cosmic().palette.bright_red.into()
}

/// The unfilled part of a supply bar.
pub fn supply_track() -> Color {
    cosmic::theme::active().cosmic().palette.neutral_5.into()
}

/// What a supply is drawn in when it reported no colour of its own.
pub fn supply_neutral() -> Color {
    grey(if cosmic::theme::active().cosmic().is_dark {
        MIN_CHANNEL_ON_DARK
    } else {
        MAX_CHANNEL_ON_LIGHT
    })
}

/// Returns a translucent selection color that preserves job-state colors.
pub fn selection() -> Color {
    let mut color: Color = cosmic::theme::active().cosmic().palette.neutral_5.into();
    color.a = 0.3;
    color
}

/// A hairline, for the edge of a supply that is nearly the colour of its track.
pub fn hairline() -> Color {
    let mut color: Color = cosmic::theme::active().cosmic().on_bg_color().into();
    color.a = 0.2;
    color
}

/// Adjusts supply brightness for contrast while preserving hue where possible.
pub fn visible_on_card(color: Color) -> Color {
    visible_on(color, cosmic::theme::active().cosmic().is_dark)
}

// Accept the theme explicitly because global theme state is not test-isolated.
fn visible_on(color: Color, on_dark: bool) -> Color {
    if on_dark {
        let peak = color.r.max(color.g).max(color.b);
        if peak >= MIN_CHANNEL_ON_DARK {
            return color;
        }
        if peak <= f32::EPSILON {
            return grey(MIN_CHANNEL_ON_DARK);
        }

        return scaled(color, MIN_CHANNEL_ON_DARK / peak);
    }

    let dimmest = color.r.min(color.g).min(color.b);
    if dimmest <= MAX_CHANNEL_ON_LIGHT {
        return color;
    }
    if dimmest >= 1.0 - f32::EPSILON {
        return grey(MAX_CHANNEL_ON_LIGHT);
    }

    scaled(color, MAX_CHANNEL_ON_LIGHT / dimmest)
}

fn grey(channel: f32) -> Color {
    Color::from_rgb(channel, channel, channel)
}

fn scaled(color: Color, scale: f32) -> Color {
    Color {
        r: (color.r * scale).clamp(0.0, 1.0),
        g: (color.g * scale).clamp(0.0, 1.0),
        b: (color.b * scale).clamp(0.0, 1.0),
        ..color
    }
}

/// How bright a supply's strongest channel has to be to be seen on a dark card.
const MIN_CHANNEL_ON_DARK: f32 = 0x9A as f32 / 255.0;
/// How dark a supply's weakest channel has to be to be seen on a light card.
const MAX_CHANNEL_ON_LIGHT: f32 = 0xD0 as f32 / 255.0;

/// How close to the track a colour may be before it needs an edge to be told apart.
pub const SUPPLY_OUTLINE_TOLERANCE: f32 = 0.15;

// The graph stacks a 21-pixel label over a 20-pixel bar row.
pub const SUPPLY_GRAPH_HEIGHT: f32 = 41.0;
pub const SUPPLY_LABEL_HEIGHT: f32 = 21.0;
pub const SUPPLY_BAR_HEIGHT: f32 = 20.0;
pub const SUPPLY_TRACK_HEIGHT: f32 = 12.0;
pub const SUPPLY_PERCENTAGE_WIDTH: f32 = 48.0;
pub const SUPPLY_DOT_SIZE: f32 = 8.0;
pub const INLINE_EDIT_HEIGHT: f32 = 32.0;
#[allow(dead_code)]
pub const SUPPLY_MARK_WIDTH: f32 = 2.0;
#[allow(dead_code)]
pub const SUPPLY_MARK_HEIGHT: f32 = 16.0;
/// A supply bar is a pill, whatever its height.
pub const RADIUS_SUPPLY_BAR: f32 = 40.0;

pub const ICON_SIZE: u16 = 16;

#[cfg(test)]
mod tests {
    use super::{MAX_CHANNEL_ON_LIGHT, visible_on};
    use cosmic::iced::Color;

    fn channels(color: Color) -> [u8; 3] {
        [
            (color.r * 255.0).round() as u8,
            (color.g * 255.0).round() as u8,
            (color.b * 255.0).round() as u8,
        ]
    }

    #[test]
    fn a_dark_supply_is_lifted_on_a_dark_card() {
        assert_eq!(channels(visible_on(Color::BLACK, true)), [0x9A, 0x9A, 0x9A]);
        assert_eq!(
            channels(visible_on(Color::from_rgba8(0x00, 0x00, 0x80, 1.0), true)),
            [0x00, 0x00, 0x9A]
        );

        let cyan = Color::from_rgba8(0x00, 0xFF, 0xFF, 1.0);
        assert_eq!(channels(visible_on(cyan, true)), channels(cyan));
    }

    #[test]
    fn a_pale_supply_is_deepened_on_a_light_card() {
        assert_eq!(
            channels(visible_on(Color::WHITE, false)),
            [0xD0, 0xD0, 0xD0]
        );

        let pale_yellow = Color::from_rgba8(0xFF, 0xFF, 0xE0, 1.0);
        let deepened = visible_on(pale_yellow, false);
        assert!(deepened.b <= MAX_CHANNEL_ON_LIGHT);
        assert!(deepened.r > deepened.b && deepened.g > deepened.b);

        let navy = Color::from_rgba8(0x00, 0x00, 0x80, 1.0);
        assert_eq!(channels(visible_on(navy, false)), channels(navy));
    }

    #[test]
    fn the_two_themes_move_a_colour_opposite_ways() {
        let mid_grey = Color::from_rgba8(0x55, 0x55, 0x55, 1.0);

        assert!(visible_on(mid_grey, true).r > mid_grey.r);
        assert_eq!(visible_on(mid_grey, false).r, mid_grey.r);

        let pale = Color::from_rgba8(0xF0, 0xF0, 0xF0, 1.0);
        assert_eq!(visible_on(pale, true).r, pale.r);
        assert!(visible_on(pale, false).r < pale.r);
    }
}
