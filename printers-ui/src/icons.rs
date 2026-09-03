//! Portable icon-name selection.

use cosmic::widget::icon;
use std::sync::LazyLock;

pub(crate) fn web_page() -> &'static str {
    "web-browser-symbolic"
}

pub(crate) fn printer_queue() -> &'static str {
    *PRINTER_QUEUE
}

static PRINTER_QUEUE: LazyLock<&'static str> =
    LazyLock::new(|| resolve("printer-queue-symbolic", "printer-printing-symbolic"));

// Resolve at the same 16-pixel size used by the UI because themes may be size-specific.
fn resolve(preferred: &'static str, fallback: &'static str) -> &'static str {
    if icon::from_name(preferred).size(16).path().is_some() {
        preferred
    } else {
        tracing::debug!(
            preferred,
            fallback,
            "icon not in this theme, using the fallback"
        );
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::{icon, resolve};

    const PORTABLE: &[&str] = &[
        "checkbox-checked-symbolic",
        "go-next-symbolic",
        "go-previous-symbolic",
        "media-playback-pause-symbolic",
        "media-playback-start-symbolic",
        "object-select-symbolic",
        "view-refresh-symbolic",
        "web-browser-symbolic",
        "window-close-symbolic",
    ];

    const FALLBACKS: &[(&str, &str)] = &[("printer-queue-symbolic", "printer-printing-symbolic")];

    fn found(name: &str) -> bool {
        icon::from_name(name).size(16).path().is_some()
    }

    fn no_theme_here() -> bool {
        if PORTABLE.iter().copied().any(found) {
            return false;
        }

        eprintln!("no icon theme on this machine, so there is nothing to resolve against");
        true
    }

    #[test]
    fn every_name_the_screens_ask_for_resolves() {
        if no_theme_here() {
            return;
        }

        for name in PORTABLE {
            assert!(
                found(name),
                "{name} is asked for directly and this theme does not carry it"
            );
        }

        for (preferred, fallback) in FALLBACKS {
            assert!(
                found(resolve(preferred, fallback)),
                "neither {preferred} nor {fallback} resolves"
            );
        }
    }

    #[test]
    fn every_fallback_is_a_real_icon_name() {
        let roots = icon_directories();
        if roots.is_empty() {
            eprintln!("no icon directories on this machine, so there is nothing to look through");
            return;
        }

        for (preferred, fallback) in FALLBACKS {
            assert!(
                roots.iter().any(|root| contains_icon(root, fallback)),
                "{preferred} falls back to {fallback}, which is not an icon in any theme installed here"
            );
        }
    }

    #[test]
    fn adwaita_alone_can_draw_every_screen() {
        let Some(adwaita) = icon_directories()
            .into_iter()
            .map(|root| root.join("Adwaita"))
            .find(|theme| theme.is_dir())
        else {
            eprintln!(
                "Adwaita is not installed here, so the desktops it stands in for cannot be checked"
            );
            return;
        };

        for name in PORTABLE {
            assert!(
                contains_icon(&adwaita, name),
                "{name} is asked for directly and Adwaita does not carry it"
            );
        }

        for (preferred, fallback) in FALLBACKS {
            assert!(
                contains_icon(&adwaita, fallback),
                "with no COSMIC icons a desktop needs {fallback} in place of {preferred}, and Adwaita does not carry it"
            );
        }
    }

    fn icon_directories() -> Vec<std::path::PathBuf> {
        let mut roots = Vec::new();

        if let Some(home) = std::env::var_os("HOME") {
            roots.push(std::path::PathBuf::from(home).join(".local/share/icons"));
        }

        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .unwrap_or_else(|_| String::from("/usr/local/share:/usr/share"));
        roots.extend(
            data_dirs
                .split(":")
                .map(|dir| std::path::Path::new(dir).join("icons")),
        );

        roots.retain(|root| root.is_dir());
        roots
    }

    fn contains_icon(root: &std::path::Path, name: &str) -> bool {
        let wanted = format!("{name}.svg");
        let mut pending = vec![root.to_path_buf()];

        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.file_name().is_some_and(|file| file == wanted.as_str()) {
                    return true;
                }
            }
        }

        false
    }
}
