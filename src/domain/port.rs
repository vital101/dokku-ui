//! Port-mapping validation for the `ports:*` command family.
//!
//! The mappings accepted here are exactly what dokku 0.38.4's ports plugin
//! parses (`plugins/ports/functions.go::parsePortMapString`): either a bare
//! host port (an internal mapping) or `scheme:host-port:container-port`.
//! Values travel as SSH argv and are re-split by the host's `xargs` wrapper,
//! so anything containing a single quote or whitespace is rejected.

/// Maximum mappings accepted in one form/plan. Keeps the SSH argv bounded.
pub const MAX_PORT_MAPPINGS: usize = 20;
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PortMapError {
    #[error("no port mappings given")]
    Empty,
    #[error("too many port mappings; at most {MAX_PORT_MAPPINGS} are allowed")]
    TooMany,
    #[error("port mapping `{0}` must not contain quotes or whitespace")]
    Unsafe(String),
    #[error("port mapping `{0}` must be a port number or scheme:host:container")]
    Shape(String),
    #[error("port mapping `{0}` has an invalid scheme")]
    Scheme(String),
    #[error("port mapping `{0}` has an invalid port number")]
    Port(String),
}

/// Validates one mapping string as printed by `ports:report` (`http:80:5000`,
/// `__internal__:8080:0`) or typed by a user (`8080`).
pub fn validate_port_mapping(raw: &str) -> Result<(), PortMapError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(PortMapError::Empty);
    }
    if trimmed
        .chars()
        .any(|c| c == '\'' || c.is_whitespace() || c.is_control())
    {
        return Err(PortMapError::Unsafe(raw.to_owned()));
    }
    if !trimmed.contains(':') {
        parse_port(trimmed).map_err(|_| PortMapError::Port(raw.to_owned()))?;
        return Ok(());
    }
    let parts: Vec<&str> = trimmed.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(PortMapError::Shape(raw.to_owned()));
    }
    let scheme = parts[0];
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(PortMapError::Scheme(raw.to_owned()));
    }
    parse_port(parts[1]).map_err(|_| PortMapError::Port(raw.to_owned()))?;
    parse_port(parts[2]).map_err(|_| PortMapError::Port(raw.to_owned()))?;
    Ok(())
}

fn parse_port(raw: &str) -> Result<u16, ()> {
    raw.parse::<u16>().map_err(|_| ())
}

/// Parses a whitespace/comma-separated mapping list (the form input) into
/// validated, individual argv tokens. Empty input and overlength lists are
/// rejected; so is every token that would not survive the SSH re-split.
pub fn parse_port_mappings(input: &str) -> Result<Vec<String>, PortMapError> {
    let mappings: Vec<String> = input
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect();
    if mappings.is_empty() {
        return Err(PortMapError::Empty);
    }
    if mappings.len() > MAX_PORT_MAPPINGS {
        return Err(PortMapError::TooMany);
    }
    for mapping in &mappings {
        validate_port_mapping(mapping)?;
    }
    Ok(mappings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_user_and_report_shapes() {
        for raw in [
            "8080",
            "http:80:5000",
            "https:443:5000",
            "tcp:5432:5432",
            "grpc:50051:50051",
            "__internal__:8080:0",
        ] {
            assert!(validate_port_mapping(raw).is_ok(), "{raw}");
        }
    }

    #[test]
    fn rejects_unsafe_and_malformed() {
        for raw in [
            "",
            "http:'80':5000",
            "http:80:5000 extra",
            "nope",
            "http:80",
            "http:80:5000:1",
            "http::5000",
            "http:abc:5000",
            "http:80:70000",
            ":80:5000",
            "HTTP:80:5000",
        ] {
            assert!(validate_port_mapping(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn parses_whitespace_and_comma_separated_lists() {
        assert_eq!(
            parse_port_mappings(" http:80:5000, tcp:5432:5432 ").expect("parse"),
            vec!["http:80:5000".to_owned(), "tcp:5432:5432".to_owned()]
        );
        assert_eq!(parse_port_mappings("").unwrap_err(), PortMapError::Empty);
        assert_eq!(
            parse_port_mappings("http:80:5000,").expect("trailing separator"),
            vec!["http:80:5000".to_owned()]
        );
    }

    #[test]
    fn caps_the_mapping_count() {
        let many = vec!["80"; MAX_PORT_MAPPINGS + 1].join(" ");
        assert_eq!(
            parse_port_mappings(&many).unwrap_err(),
            PortMapError::TooMany
        );
    }

    #[test]
    fn maps_zero_is_accepted_like_dokku() {
        assert!(validate_port_mapping("0").is_ok());
        assert!(validate_port_mapping("http:0:0").is_ok());
    }
}
