use agent::config::{EnvironmentConfig, GlobalAgentConfig};
use agent::{app, decision_log, telemetry};
use config::{Config, Environment};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const IS_DEVELOPMENT: bool = cfg!(debug_assertions);

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    // Donka: secret settings given as files (`X_FILE`), read before anything else.
    if let Err(error) = agent::secret_files::load() {
        eprintln!("Invalid environment variables: {error}");
        std::process::exit(2);
    }

    let settings = Config::builder()
        .add_source(Environment::default().separator("__").try_parsing(true))
        .set_default("provider.type", "Zip")
        .expect("Failed to set provider.type as Filesystem")
        .build()
        .expect("Failed to build settings");

    let cfg: EnvironmentConfig = settings
        .try_deserialize()
        .expect("Invalid environment variables");

    telemetry::setup(cfg.otel_enabled).ok();

    let rustls_config = match &cfg.http_ssl {
        None => None,
        Some(s) => {
            rustls::crypto::aws_lc_rs::default_provider()
                .install_default()
                .expect("Failed to install rustls crypto provider");
            Some(s.to_rustls_config().await.expect("Valid SSL Config"))
        }
    };

    // Artifact signature keys (DNK-87): an invalid value stops the start, like
    // any invalid setting; without keys the Runtime says so and serves as
    // before, so existing installations upgrade in order.
    let artifact_keys = match agent::artifact_signature::ArtifactKeys::parse(
        "ARTIFACT__PUBLIC_KEY",
        cfg.artifact.public_key.as_deref(),
        cfg.artifact.public_key_previous.as_deref(),
    ) {
        Ok(keys) => keys,
        Err(err) => {
            eprintln!("Invalid environment variables: {err}");
            std::process::exit(2);
        }
    };
    if artifact_keys.enforced() {
        tracing::info!("release artifacts are verified against the installation's signing keys");
    } else {
        tracing::warn!(
            "no artifact signing key configured (ARTIFACT__PUBLIC_KEY): release artifacts are served unverified"
        );
    }
    let global_config = Arc::new(GlobalAgentConfig {
        release_zip_password: cfg.release_zip_password.clone(),
        artifact_keys,
    });

    let agent = app::create_agent(cfg.clone(), global_config).await;
    let app = app::create_app(agent, cfg).await;

    let listener_address_str = IS_DEVELOPMENT
        .then_some("127.0.0.1:3000")
        .unwrap_or("0.0.0.0:8080");
    let listener_address =
        SocketAddr::from_str(listener_address_str).expect("Valid socket address");

    // Donka: on SIGTERM or Ctrl-C, finish the requests in flight, then send the
    // decision records still queued.
    let handle = axum_server::Handle::new();
    tokio::spawn(stop_on_signal(handle.clone()));

    let server_result = match rustls_config {
        None => {
            tracing::info!("🚀 Listening on http://{listener_address}");
            axum_server::bind(listener_address)
                .handle(handle)
                .serve(app.into_make_service())
                .await
        }
        Some(rustls_config) => {
            tracing::info!("🚀 Listening on https://{listener_address}");
            axum_server::bind_rustls(listener_address, rustls_config)
                .handle(handle)
                .serve(app.into_make_service())
                .await
        }
    };

    if let Err(error) = server_result {
        tracing::error!("Server exited with an error: {error:?}");
    }
    decision_log::shutdown().await;
}

async fn stop_on_signal(handle: axum_server::Handle<SocketAddr>) {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!("Cannot listen for SIGTERM: {error}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("Stopping: finishing requests in flight");
    handle.graceful_shutdown(Some(Duration::from_secs(10)));
}
