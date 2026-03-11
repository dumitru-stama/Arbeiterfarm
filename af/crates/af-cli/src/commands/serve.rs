use crate::app::ServeCommand;
use crate::CliConfig;
use af_core::LlmRoute;
use tower_service::Service;

pub async fn handle(config: &CliConfig, cmd: ServeCommand) -> anyhow::Result<()> {
    let pool = if let Some(p) = &config.pool {
        p.clone()
    } else {
        let database_url = std::env::var("AF_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://af:af@localhost/af".to_string());
        let pool_size: u32 = std::env::var("AF_DB_POOL_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);
        af_db::init_db_with_pool_size(&database_url, pool_size).await?
    };

    let router = config
        .router
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no LLM backends configured — cannot start API server"))?
        .clone();

    let upload_max_bytes: usize = std::env::var("AF_UPLOAD_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100 * 1024 * 1024); // 100 MB

    let api_rate_limit: u32 = std::env::var("AF_API_RATE_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    let rate_limiter = Some(std::sync::Arc::new(
        af_api::rate_limit::ApiRateLimiter::new_postgres(pool.clone(), api_rate_limit),
    ));

    let cors_origin = std::env::var("AF_CORS_ORIGIN").ok();
    // Validate CORS origin eagerly so a misconfiguration fails at startup, not on first request
    if let Some(ref origin) = cors_origin {
        if origin != "*" {
            origin.parse::<axum::http::HeaderValue>().map_err(|e| {
                anyhow::anyhow!("invalid AF_CORS_ORIGIN value \"{origin}\": {e}")
            })?;
        }
    }

    // Resolve summarization backend for compaction
    let summarization_backend = config
        .compaction
        .summarization_route
        .as_ref()
        .and_then(|route_str| {
            let route = LlmRoute::from_str(route_str);
            match router.resolve(&route) {
                Ok(backend) => {
                    eprintln!("[af] Compaction summarization route: {route_str}");
                    Some(backend)
                }
                Err(e) => {
                    eprintln!(
                        "[af] WARNING: summarization_route '{route_str}' not found ({e}), using agent's backend"
                    );
                    None
                }
            }
        });

    let cleanup_pool = pool.clone();

    let state = af_api::AppState {
        pool,
        specs: config.specs.clone(),
        executors: config.executors.clone(),
        evidence_resolvers: config.evidence_resolvers.clone(),
        post_tool_hook: config.post_tool_hook.clone(),
        core_config: config.core_config.clone(),
        agent_configs: config.agent_configs.clone(),
        router,
        upload_max_bytes,
        rate_limiter,
        cors_origin,
        ghidra_cache_dir: config.ghidra_cache_dir.clone(),
        source_map: config.source_map.clone(),
        compaction_threshold: config.compaction.threshold,
        summarization_backend,
        security_config: af_api::SecurityConfig {
            sandbox_available: af_jobs::oop_executor::bwrap_available().await,
            sandbox_enforced: std::env::var("AF_ALLOW_UNSANDBOXED").is_err(),
            tls_enabled: cmd.tls_cert.is_some(),
        },
        stream_tracker: af_api::ActiveStreamTracker::new(
            std::env::var("AF_MAX_CONCURRENT_STREAMS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
        ),
    };

    let shared_state = std::sync::Arc::new(state);
    let app = af_api::build_router(shared_state.clone());

    // Spawn background task to clean up stale rate-limit windows every 60s
    {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                af_api::rate_limit::cleanup_stale_windows(&cleanup_pool).await;
            }
        });
    }

    // Spawn PgListener for near-real-time notification delivery.
    // _shutdown_tx is kept alive until handle() returns (when the server exits),
    // at which point the watch channel closes and the listener task shuts down.
    let _shutdown_tx;
    {
        let listener_pool = shared_state.pool.clone();
        let listener_storage = config.core_config.storage_root.clone();
        let (tx, shutdown_rx) = tokio::sync::watch::channel(false);
        _shutdown_tx = tx;
        tokio::spawn(async move {
            af_notify::listener::run_notification_listener(
                listener_pool,
                listener_storage,
                shutdown_rx,
            )
            .await;
        });
    }

    // Resolve TLS config: CLI flags take priority, then env vars
    let tls_cert = cmd.tls_cert.clone().or_else(|| std::env::var("AF_TLS_CERT").ok());
    let tls_key = cmd.tls_key.clone().or_else(|| std::env::var("AF_TLS_KEY").ok());

    let bind = std::env::var("AF_BIND_ADDR").unwrap_or_else(|_| cmd.bind.clone());

    // Non-localhost TLS safety check
    if tls_cert.is_none() && !is_localhost_bind(&bind) {
        if std::env::var("AF_ALLOW_INSECURE").is_err() && !cmd.allow_insecure {
            anyhow::bail!(
                "Refusing to serve HTTP without TLS on {bind}. \
                 Use --tls-cert/--tls-key, or --allow-insecure / AF_ALLOW_INSECURE=1 to override."
            );
        }
        eprintln!("[af] WARNING: serving HTTP without TLS on {bind}");
    }

    let listener = tokio::net::TcpListener::bind(&bind).await?;

    match (tls_cert, tls_key) {
        (Some(cert), Some(key)) => {
            println!("Arbeiterfarm API server listening on https://{bind}");
            serve_tls(listener, app, &cert, &key).await?;
        }
        (None, None) => {
            println!("Arbeiterfarm API server listening on http://{bind}");
            axum::serve(listener, app).await?;
        }
        _ => {
            anyhow::bail!("both --tls-cert and --tls-key must be provided (or neither)");
        }
    }

    Ok(())
}

fn is_localhost_bind(bind: &str) -> bool {
    let host = bind.rsplit_once(':').map(|(h, _)| h).unwrap_or(bind);
    matches!(host, "127.0.0.1" | "::1" | "localhost" | "[::1]")
}

async fn serve_tls(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    cert_path: &str,
    key_path: &str,
) -> anyhow::Result<()> {
    use std::io::BufReader;
    use std::sync::Arc;

    let cert_file = std::fs::File::open(cert_path)
        .map_err(|e| anyhow::anyhow!("failed to open TLS cert {cert_path}: {e}"))?;
    let key_file = std::fs::File::open(key_path)
        .map_err(|e| anyhow::anyhow!("failed to open TLS key {key_path}: {e}"))?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(cert_file))
        .collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        anyhow::bail!("no certificates found in {cert_path}");
    }

    let key = rustls_pemfile::private_key(&mut BufReader::new(key_file))?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {key_path}"))?;

    let tls_config = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(certs, key)?;

    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));

    loop {
        let (stream, addr) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let app = app.clone();

        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(stream).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!("TLS handshake failed from {addr}: {e}");
                    return;
                }
            };

            let stream = hyper_util::rt::TokioIo::new(tls_stream);

            let hyper_service = hyper::service::service_fn(
                move |req: hyper::Request<hyper::body::Incoming>| {
                    let mut app = app.clone();
                    async move {
                        let req = req.map(axum::body::Body::new);
                        Ok::<_, std::convert::Infallible>(
                            app.call(req).await.unwrap_or_else(|e| match e {}),
                        )
                    }
                },
            );

            let builder = hyper_util::server::conn::auto::Builder::new(
                hyper_util::rt::TokioExecutor::new(),
            );

            if let Err(e) = builder
                .serve_connection_with_upgrades(stream, hyper_service)
                .await
            {
                tracing::debug!("connection error from {addr}: {e}");
            }
        });
    }
}
