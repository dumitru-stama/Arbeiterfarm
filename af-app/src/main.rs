//! Generic Arbeiterfarm binary — loads TOML plugins only, no compiled domain plugins.
//!
//! Usage:
//!   af chat --agent assistant --project p1            # all TOML plugins
//!   af --plugin personal-assistant chat ...           # specific plugin
//!   af --plugin pa --plugin fuzzer serve --bind 0.0.0.0:9090

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = af_cli::parse_cli();

    let plugin_filter = if cli.plugins.is_empty() {
        None
    } else {
        Some(cli.plugins.clone())
    };

    let config = af_cli::bootstrap::bootstrap(
        &[],
        vec![],
        plugin_filter.as_deref(),
    )
    .await?;

    af_cli::run_with_cli(config, cli).await
}
