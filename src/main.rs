#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::as_conversions)]

use color_eyre::eyre::{eyre, Context};
use influxdb::InfluxDbWriteable;
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Deserialize;
use tracing::{info, Level};
use tracing_error::ErrorLayer;
use tracing_subscriber::{prelude::*, util::SubscriberInitExt, EnvFilter, Registry};

use crate::pay2wash::model::influx::InfluxMachineStatus;

mod pay2wash;
mod strict_types;

#[derive(Debug, Deserialize)]
struct Environment {
    pay2wash_email: strict_types::Email,
    pay2wash_password: strict_types::Password,

    influx_api_token: strict_types::Password,
}

fn main() -> color_eyre::Result<()> {
    // Load environment variables from .env file
    dotenvy::dotenv().ok();

    color_eyre::install()?;

    // TODO: Sentry

    Registry::default()
        .with(tracing_subscriber::fmt::layer().pretty())
        .with(
            EnvFilter::builder()
                .with_default_directive(Level::INFO.into())
                .from_env()
                .wrap_err("failed to parse RUST_LOG")?,
        )
        .with(ErrorLayer::default())
        .init();

    let environment: Environment = envy::from_env()
        .map_err(|err| match err {
            envy::Error::MissingValue(key) => eyre!("missing environment variable {key}"),
            envy::Error::Custom(message) => eyre!(message),
        })
        .wrap_err("failed to load environment")?;

    // Since fly.io is a one core machine, we only need the current thread
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(async_main(environment))
}

async fn async_main(environment: Environment) -> color_eyre::Result<()> {
    info!(?environment);

    let db = influxdb::Client::new(
        "https://pain2wash-influx.fly.dev",
        if cfg!(debug_assertions) {
            "pain2wash-development"
        } else {
            "pain2wash-production"
        },
    )
    .with_http_client(
        reqwest::Client::builder()
            .default_headers(HeaderMap::from_iter([(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Token {}", environment.influx_api_token.as_ref()))
                    .expect("Authorization header value should be well formed"),
            )]))
            .build()
            .expect("reqwest client configuration should be valid"),
    );

    let (db_type, db_version) = db
        .ping()
        .await
        .wrap_err("failed to connect to influxdb instance")?;

    info!(
        db_type,
        db_version,
        db_name = db.database_name(),
        db_url = db.database_url(),
        "connected to influxdb database"
    );

    let client =
        pay2wash::Pay2WashClient::new(environment.pay2wash_email, environment.pay2wash_password);

    let session = client
        .authenticate()
        .await
        .wrap_err("failed to authenticate")?;

    let statuses = client
        .get_machine_statuses(&session)
        .await
        .wrap_err("failed to get machine statuses")?;

    // for (name, MachineStatus { state, .. }) in statuses {
    //     dbg!((name, state));
    // }

    dbg!(
        db.query(
            statuses
                .iter()
                .map(|(name, status)| {
                    InfluxMachineStatus::new(status.raw, name, &session.location)
                        .into_query("washers")
                })
                .collect::<Vec<_>>(),
        )
        .await
    );

    // let session = client
    //     .authenticate()
    //     .await
    //     .wrap_err("failed to authenticate")?;

    Ok(())
}
