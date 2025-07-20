#![doc = include_str!("../README.md")]
#![warn(clippy::pedantic, clippy::nursery, missing_docs, clippy::cargo)]
#![allow(clippy::multiple_crate_versions)]

mod commands;
mod env;
mod events;
mod interactions;
mod models;
mod oauth;
mod util;

use crate::models::{system, trust::Trusted, user};
use std::{
    process::ExitCode,
    str::FromStr,
    sync::{Arc, LazyLock},
};

use axum::{extract::MatchedPath, http::Request};
use commands::process_command_event;
use error_stack::{ResultExt, report};
use events::process_push_event;
use interactions::process_interaction_event;
use oauth::oauth_handler;
use slack_morphism::prelude::*;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use tower_http::trace::TraceLayer;
use tracing::{debug, info, info_span, level_filters::LevelFilter};
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

/// The slack app token. Used for socket mode if we ever decide to use it.
pub static APP_TOKEN: LazyLock<SlackApiToken> =
    LazyLock::new(|| SlackApiToken::new(env::slack_app_token().into()));

/// The slack bot token. Used for most interactions
pub static BOT_TOKEN: LazyLock<SlackApiToken> =
    LazyLock::new(|| SlackApiToken::new(env::slack_bot_token().into()));

#[derive(thiserror::Error, displaydoc::Display, Debug)]
enum Error {
    /// Error initializing environment variables
    Env,
    /// Error during slack client initialization
    Initialization,
}

#[dotenvy::load]
#[tokio::main]
#[tracing::instrument]
async fn main() -> error_stack::Result<ExitCode, Error> {
    let console_subscriber = tracing_subscriber::fmt::layer().pretty();
    let error_subscriber = tracing_error::ErrorLayer::default();
    let env_subscriber = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    tracing_subscriber::registry()
        .with(console_subscriber.with_filter(env_subscriber))
        .with(error_subscriber)
        .with(tracing_journald::layer().ok())
        .init();

    if env::any_set() {
        env::assert_env_vars();
    } else {
        return Err(report!(Error::Env)
            .attach_printable("No environment variables are set. See help message below:")
            .attach_printable(env::gen_help()));
    }

    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| report!(Error::Initialization))
        .attach_printable("Error installing default ring crypto provider")?;

    let mut options = SqliteConnectOptions::from_str(&env::database_url())
        .unwrap()
        .optimize_on_close(true, None)
        .create_if_missing(true);

    if let Some(key) = env::encryption_key() {
        options = options.pragma("key", key);
    }

    let pool = SqlitePool::connect_with(options)
        .await
        .attach_printable("Error connecting to database")
        .change_context(Error::Initialization)?;

    sqlx::migrate!()
        .run(&pool)
        .await
        .attach_printable("Error running database migrations")
        .change_context(Error::Initialization)?;

    // Test query to make sure stuff works before we start the bot
    debug!("Testing database connection");
    sqlx::query!(
        r#"
        SELECT
            id as "id: system::Id<Trusted>"
        FROM
            systems
    "#
    )
    .fetch_all(&pool)
    .await
    .attach_printable("Error fetching systems from database")
    .change_context(Error::Initialization)?;

    let client = Arc::new(SlackClient::new(
        SlackClientHyperConnector::new()
            .attach_printable("Error creating Slack hyper connector")
            .change_context(Error::Initialization)?,
    ));

    let state = user::State { db: pool.clone() };

    let listener_environment: Arc<SlackHyperListenerEnvironment> = Arc::new(
        SlackClientEventsListenerEnvironment::new(client.clone()).with_user_state(state.clone()),
    );

    let signing_secret: SlackSigningSecret = env::slack_signing_secret().into();

    let listener: SlackEventsAxumListener<SlackHyperHttpsConnector> =
        SlackEventsAxumListener::new(listener_environment.clone());

    let app = axum::routing::Router::new()
        // Note: I do not use the slack-morphism oauth thing because it's a bit too much for me
        .route("/auth", axum::routing::get(oauth_handler))
        .with_state(state.clone())
        .route(
            "/push",
            axum::routing::post(process_push_event).layer(
                listener
                    .events_layer(&signing_secret)
                    .with_event_extractor(SlackEventsExtractors::push_event()),
            ),
        )
        .route(
            "/command",
            axum::routing::post(process_command_event).layer(
                listener
                    .events_layer(&signing_secret)
                    .with_event_extractor(SlackEventsExtractors::command_event()),
            ),
        )
        .route(
            "/interaction",
            axum::routing::post(process_interaction_event).layer(
                listener
                    .events_layer(&signing_secret)
                    .with_event_extractor(SlackEventsExtractors::interaction_event()),
            ),
        )
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                // Log the matched route's path (with placeholders not filled in).
                // Use request.uri() or OriginalUri if you want the real path.
                let matched_path = request
                    .extensions()
                    .get::<MatchedPath>()
                    .map(MatchedPath::as_str);

                info_span!(
                    "slack_system_bot::http_request",
                    method = ?request.method(),
                    matched_path,
                )
            }),
        );

    info!("Slack bot is running");

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
        .await
        .attach_printable("Failed to bind to address")
        .change_context(Error::Initialization)?;

    axum::serve(listener, app)
        .await
        .attach_printable("Failed to start server")
        .change_context(Error::Initialization)?;

    Ok(ExitCode::SUCCESS)
}
