use std::net::SocketAddr;

use thiserror::Error;

pub const DEFAULT_BIND: &str = "127.0.0.1:8787";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BindError {
    #[error("invalid PENDANT_BIND {input:?}: {message}")]
    Invalid { input: String, message: String },
    #[error("refusing unspecified bind address {addr} (no 0.0.0.0 / ::)")]
    Unspecified { addr: SocketAddr },
}

/// Parse `host:port` and reject unspecified IPs (`0.0.0.0`, `::`).
pub fn parse_bind(input: &str) -> Result<SocketAddr, BindError> {
    let trimmed = input.trim();
    let addr: SocketAddr = trimmed
        .parse::<SocketAddr>()
        .map_err(|err| BindError::Invalid {
            input: trimmed.to_string(),
            message: err.to_string(),
        })?;
    if addr.ip().is_unspecified() {
        return Err(BindError::Unspecified { addr });
    }
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn default_loopback_ok() {
        let addr = parse_bind(DEFAULT_BIND).unwrap();
        assert_eq!(addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(addr.port(), 8787);
    }

    #[test]
    fn ipv6_loopback_ok() {
        let addr = parse_bind("[::1]:8787").unwrap();
        assert_eq!(addr.ip(), IpAddr::V6(Ipv6Addr::LOCALHOST));
    }

    #[test]
    fn reject_unspecified_v4() {
        let err = parse_bind("0.0.0.0:8787").unwrap_err();
        assert!(matches!(err, BindError::Unspecified { .. }));
    }

    #[test]
    fn reject_unspecified_v6() {
        let err = parse_bind("[::]:8787").unwrap_err();
        assert!(matches!(err, BindError::Unspecified { .. }));
    }

    #[test]
    fn reject_garbage() {
        assert!(matches!(
            parse_bind("not-an-addr"),
            Err(BindError::Invalid { .. })
        ));
    }
}
