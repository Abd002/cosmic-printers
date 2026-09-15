//! Icons the screens draw.

use cosmic::widget::icon;

const WEB_PAGE_ICON: &[u8] = include_bytes!("../resources/icons/web-browser-symbolic.svg");
const PRINTER_QUEUE_ICON: &[u8] = include_bytes!("../resources/icons/printer-queue-symbolic.svg");

pub(crate) fn web_page() -> icon::Handle {
    embedded(WEB_PAGE_ICON)
}

pub(crate) fn printer_queue() -> icon::Handle {
    embedded(PRINTER_QUEUE_ICON)
}

fn embedded(bytes: &'static [u8]) -> icon::Handle {
    let mut handle = icon::from_svg_bytes(bytes);
    handle.symbolic = true;
    handle
}

#[cfg(test)]
mod tests {
    use super::{PRINTER_QUEUE_ICON, WEB_PAGE_ICON, icon, printer_queue, web_page};

    const PORTABLE: &[&str] = &[
        "checkbox-checked-symbolic",
        "go-next-symbolic",
        "go-previous-symbolic",
        "media-playback-pause-symbolic",
        "media-playback-start-symbolic",
        "object-select-symbolic",
        "view-refresh-symbolic",
        "window-close-symbolic",
    ];

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
    }

    #[test]
    fn the_embedded_icons_are_symbolic_so_the_theme_colours_them() {
        for handle in [web_page(), printer_queue()] {
            assert!(handle.symbolic);
        }
    }

    #[test]
    fn the_embedded_icons_are_svgs() {
        for bytes in [WEB_PAGE_ICON, PRINTER_QUEUE_ICON] {
            assert!(
                std::str::from_utf8(bytes).is_ok_and(|svg| svg.contains("<svg")),
                "an embedded icon is not an SVG"
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
