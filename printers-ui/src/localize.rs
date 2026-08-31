//! Printer UI localization.
//! A crate-local catalogue lets `i18n_embed_fl` validate keys at compile time.

use i18n_embed::{
    DefaultLocalizer, LanguageLoader, Localizer,
    fluent::{FluentLanguageLoader, fluent_language_loader},
};
use rust_embed::RustEmbed;
use std::sync::LazyLock;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

pub(crate) static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader: FluentLanguageLoader = fluent_language_loader!();

    loader
        .load_fallback_language(&Localizations)
        .expect("Error while loading fallback language");

    #[cfg(test)]
    loader.set_use_isolating(false);

    loader
});

#[allow(unused_macros)]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::localize::LANGUAGE_LOADER, $message_id)
    }};

    ($message_id:literal, $($args:expr),*) => {{
        i18n_embed_fl::fl!($crate::localize::LANGUAGE_LOADER, $message_id, $($args), *)
    }};
}

#[allow(unused_macros)]
macro_rules! slab {
    ( $descriptions:ident { $( $txt_id:ident = $txt_expr:expr; )+ } ) => {
        let mut $descriptions = slab::Slab::new();

        $(
            let $txt_id = $descriptions.insert($txt_expr);
        )+
    }
}

/// Selects the user's preferred languages.
pub fn select_languages() {
    let localizer = localizer();
    let requested = i18n_embed::DesktopLanguageRequester::requested_languages();

    if let Err(why) = localizer.select(&requested) {
        tracing::warn!(%why, "could not select a language for the printer screens");
    }
}

/// Returns the printer UI localizer.
#[must_use]
pub fn localizer() -> Box<dyn Localizer> {
    Box::from(DefaultLocalizer::new(&*LANGUAGE_LOADER, &Localizations))
}
