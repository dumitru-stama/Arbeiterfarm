mod echo_tool;

use af_re_tools::plugin::RePlugin;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let exe_dir = std::env::current_exe()?;
    let exe_dir = exe_dir.parent().unwrap_or_else(|| std::path::Path::new("."));

    let cli = af_cli::parse_cli();

    let re_plugin = RePlugin::from_env(exe_dir);

    // Distribution binary: only serve the compiled RE plugin by default.
    // Pass --plugin <name> to additionally load specific TOML plugins.
    let plugin_filter = Some(cli.plugins.clone());

    let config = af_cli::bootstrap::bootstrap(
        &[&re_plugin],
        vec![af_cli::bootstrap::ExtraTool {
            spec: echo_tool::echo_tool_spec(),
            executor: Box::new(echo_tool::EchoToolExecutor),
        }],
        plugin_filter.as_deref(),
    )
    .await?;

    // Apply --oaie flag to core config
    let mut config = config;
    config.core_config.use_oaie = cli.oaie;

    // Start plugin-owned async services (VT gateway, sandbox gateway)
    let _vt_handle = re_plugin.start_vt_gateway(config.pool.as_ref()).await;
    let _sandbox_handle = re_plugin.start_sandbox_gateway().await;

    af_cli::run_with_cli(config, cli).await
}
