//! Reading what the user typed as a printer address.

use cosmic_settings_printers_core::Error;
use url::Url;

/// An address as typed: a bare host, `host:port`, or a URI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Target {
    /// Host name or IP literal, without brackets.
    pub(super) host: String,
    /// The port, when the input named one or the scheme implies one.
    pub(super) port: Option<u16>,
    /// The resource path, when the input had one beyond `/`.
    pub(super) path: Option<String>,
    /// The scheme, when the input had one (`ipp`, `ipps`, `http`, `https`, `socket`, `lpd`).
    pub(super) scheme: Option<String>,
}

/// Reads `192.168.1.5`, `192.168.1.5:631`, `[fe80::1]:631`, `printer.local`, or a URI such as
/// `ipps://10.0.0.8:443/ipp/print` or `socket://10.0.0.8:9100`.
pub(super) fn parse_target(input: &str) -> Result<Target, Error> {
    let input = input.trim();
    let bad = |why: &str| Error::InvalidPrinterAddress {
        why: format!("cannot read '{input}' as a printer address: {why}"),
    };

    if input.is_empty() {
        return Err(bad("it is empty"));
    }

    let (scheme, url) = if let Some((scheme, rest)) = input.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(
            scheme.as_str(),
            "ipp" | "ipps" | "http" | "https" | "socket" | "lpd"
        ) {
            return Err(bad(
                "the scheme must be ipp, ipps, http, https, socket or lpd",
            ));
        }
        // `Url` treats the port of unknown schemes as given, so normalize to a known one.
        (
            Some(scheme),
            Url::parse(&format!("http://{rest}")).map_err(|error| bad(&error.to_string()))?,
        )
    } else {
        (
            None,
            Url::parse(&format!("http://{input}")).map_err(|error| bad(&error.to_string()))?,
        )
    };

    let host = url
        .host_str()
        .map(|host| host.trim_matches(|c| c == '[' || c == ']').to_string())
        .filter(|host| !host.is_empty())
        .ok_or_else(|| bad("it has no host"))?;

    // `http://` leaves 80 implicit, so only an explicit port counts as typed.
    let port = url
        .port()
        .or_else(|| scheme.as_deref().and_then(default_port));
    let path = Some(url.path().to_string()).filter(|path| path != "/" && !path.is_empty());

    Ok(Target {
        host,
        port,
        path,
        scheme,
    })
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "ipp" | "ipps" => Some(631),
        "http" => Some(80),
        "https" => Some(443),
        "socket" => Some(9100),
        "lpd" => Some(515),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_host_has_no_port() {
        let target = parse_target("192.168.1.5").expect("a host");

        assert_eq!(target.host, "192.168.1.5");
        assert_eq!(target.port, None);
        assert_eq!(target.scheme, None);
    }

    #[test]
    fn a_host_and_port_are_split() {
        let target = parse_target("10.0.0.8:8801").expect("a host and port");

        assert_eq!(
            (target.host.as_str(), target.port),
            ("10.0.0.8", Some(8801))
        );
    }

    #[test]
    fn an_ipv6_literal_loses_its_brackets() {
        let target = parse_target("[fe80::1]:631").expect("an ipv6 literal");

        assert_eq!((target.host.as_str(), target.port), ("fe80::1", Some(631)));
    }

    #[test]
    fn a_uri_keeps_scheme_port_and_path() {
        let target = parse_target("ipps://printer.local:443/ipp/print").expect("a uri");

        assert_eq!(target.scheme.as_deref(), Some("ipps"));
        assert_eq!(target.port, Some(443));
        assert_eq!(target.path.as_deref(), Some("/ipp/print"));
    }

    #[test]
    fn socket_and_lpd_schemes_imply_their_ports() {
        assert_eq!(
            parse_target("socket://10.0.0.8")
                .expect("a socket uri")
                .port,
            Some(9100)
        );
        assert_eq!(
            parse_target("lpd://10.0.0.8/queue")
                .expect("an lpd uri")
                .port,
            Some(515)
        );
    }

    #[test]
    fn rubbish_is_refused() {
        assert!(matches!(
            parse_target(""),
            Err(Error::InvalidPrinterAddress { .. })
        ));
        assert!(parse_target("ftp://host").is_err());
        assert!(parse_target("exa mple:99999").is_err());
    }
}
