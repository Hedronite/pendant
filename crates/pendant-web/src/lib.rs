use rust_embed::RustEmbed;

/// Static PWA files served by the loopback bridge.
#[derive(RustEmbed)]
#[folder = "static/"]
pub struct Assets;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_shell() {
        assert!(Assets::get("index.html").is_some());
        assert!(Assets::get("app.js").is_some());
        assert!(Assets::get("styles.css").is_some());
        assert!(Assets::get("manifest.webmanifest").is_some());
    }
}
