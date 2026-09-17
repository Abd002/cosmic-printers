//! Standalone printers application.

mod app;

use std::sync::OnceLock;

use cosmic::app::Settings;
use cosmic_printers_ui::{Backend, list};

use app::App;

// Keep one backend per process to avoid duplicate embedded discovery state.
static BACKEND: OnceLock<Backend> = OnceLock::new();

fn backend() -> Backend {
    BACKEND.get().cloned().unwrap_or_default()
}

fn printer_events() -> impl cosmic::iced::futures::Stream<Item = list::Message<app::Message>> {
    list::printer_events_subscription(backend())
}

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    cosmic_printers_ui::init();

    let _ = BACKEND.set(Backend::detect_blocking());
    tracing::info!(backend = ?backend(), "serving printers");

    cosmic::app::run::<App>(
        Settings::default().size_limits(
            cosmic::iced::Limits::NONE
                .min_width(450.0)
                .min_height(300.0),
        ),
        (),
    )
}
