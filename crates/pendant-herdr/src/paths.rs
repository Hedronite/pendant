use std::path::PathBuf;

/// Documented default for herdr 0.9.x.
pub const DEFAULT_SOCKET_PATH: &str = "~/.config/herdr/herdr.sock";

/// Expand a leading `~` or `~/` using `HOME` (Unix) or `USERPROFILE` (Windows).
///
/// Absolute paths and other relative paths are returned unchanged. `$HOME` in
/// the middle of a string is not expanded.
pub fn expand_path(input: &str) -> PathBuf {
    let trimmed = input.trim();
    if trimmed == "~" {
        return home_dir();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("~\\") {
        return home_dir().join(rest);
    }
    PathBuf::from(trimmed)
}

pub fn default_socket_path() -> PathBuf {
    expand_path(DEFAULT_SOCKET_PATH)
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_joins_home() {
        let home = home_dir();
        assert_eq!(expand_path("~"), home);
        assert_eq!(
            expand_path("~/.config/herdr/herdr.sock"),
            home.join(".config/herdr/herdr.sock")
        );
    }

    #[test]
    fn expand_absolute_unchanged() {
        assert_eq!(
            expand_path("/tmp/herdr.sock"),
            PathBuf::from("/tmp/herdr.sock")
        );
    }

    #[test]
    fn expand_trims_whitespace() {
        assert_eq!(expand_path("  /abs/sock  "), PathBuf::from("/abs/sock"));
    }

    #[test]
    fn default_socket_ends_with_herdr_sock() {
        let path = default_socket_path();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("herdr.sock")
        );
        assert!(path.is_absolute());
    }
}
