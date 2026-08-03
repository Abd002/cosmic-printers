//! Supply-graph drawing helpers for printer-reported colors.

use cosmic::Element;
use cosmic::iced::border::Radius;
use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{container, space::horizontal as horizontal_space};

/// Draws a rounded solid fill.
pub fn fill_container(
    color: Color,
    radius: impl Into<Radius>,
) -> cosmic::theme::Container<'static> {
    let radius = radius.into();
    cosmic::theme::Container::custom(move |_theme| cosmic::widget::container::Style {
        background: Some(Background::Color(color)),
        border: Border {
            radius,
            ..Default::default()
        },
        ..Default::default()
    })
}

/// Draws a rounded fill with a contrasting edge.
pub fn bordered_fill_container(
    background: Color,
    border_color: Color,
    radius: impl Into<Radius>,
) -> cosmic::theme::Container<'static> {
    let radius = radius.into();
    cosmic::theme::Container::custom(move |_theme| cosmic::widget::container::Style {
        background: Some(Background::Color(background)),
        border: Border {
            color: border_color,
            radius,
            width: 1.0,
        },
        ..Default::default()
    })
}

/// Draws a supply-color marker.
pub fn dot<Message: 'static>(color: Color, size: f32) -> Element<'static, Message> {
    container(horizontal_space())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .class(fill_container(color, size / 2.0))
        .into()
}
