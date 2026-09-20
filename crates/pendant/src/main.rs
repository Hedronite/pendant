use anyhow::{Context, Result};
use pendant_bridge::{serve, Config};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env().context("invalid Pendant config")?;
    tracing::info!(
        bind = %config.bind,
        sock = %config.herdr_sock.display(),
        "starting Pendant (loopback bridge + PWA)"
    );
    serve(config).await.context("pendant bridge failed")?;
    Ok(())
}
