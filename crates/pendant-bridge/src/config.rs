use std::net::SocketAddr;
use std::path::PathBuf;

use pendant_herdr::{expand_path, DEFAULT_SOCKET_PATH};

use crate::bind::{parse_bind, BindError, DEFAULT_BIND};
use crate::error::BridgeError;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub herdr_sock: PathBuf,
    /// Optional bearer token required on `/v1/*` (`PENDANT_TOKEN`).
    /// Unset = Tailscale mesh is the auth boundary.
    pub token: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, BridgeError> {
        let bind_raw = std::env::var("PENDANT_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_string());
        let sock_raw =
            std::env::var("PENDANT_HERDR_SOCK").unwrap_or_else(|_| DEFAULT_SOCKET_PATH.to_string());
        let mut cfg = Self::from_raw(&bind_raw, &sock_raw)?;
        cfg.token = std::env::var("PENDANT_TOKEN")
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        Ok(cfg)
    }

    pub fn from_raw(bind: &str, herdr_sock: &str) -> Result<Self, BridgeError> {
        Ok(Self {
            bind: parse_bind(bind)?,
            herdr_sock: expand_path(herdr_sock),
            token: None,
        })
    }
}

impl From<BindError> for BridgeError {
    fn from(value: BindError) -> Self {
        match value {
            BindError::Invalid { input, message } => BridgeError::InvalidBind { input, message },
            BindError::Unspecified { addr } => BridgeError::UnspecifiedBind(addr),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pendant_herdr::home_dir;

    #[test]
    fn defaults_expand_tilde_and_loopback() {
        let cfg = Config::from_raw(DEFAULT_BIND, DEFAULT_SOCKET_PATH).unwrap();
        assert_eq!(cfg.bind.to_string(), "127.0.0.1:8780");
        assert_eq!(cfg.herdr_sock, home_dir().join(".config/herdr/herdr.sock"));
    }

    #[test]
    fn env_shaped_custom_sock() {
        let cfg = Config::from_raw("127.0.0.1:9000", "/tmp/custom.sock").unwrap();
        assert_eq!(cfg.bind.port(), 9000);
        assert_eq!(cfg.herdr_sock, PathBuf::from("/tmp/custom.sock"));
    }
}
