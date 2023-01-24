#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::as_conversions)]

use std::{
    borrow::Cow,
    convert::Infallible,
    str::FromStr,
    sync::atomic::AtomicI64,
    time::{Duration, SystemTime},
};

use color_eyre::eyre::{bail, eyre, Context};
use metrics::boolean::{BooleanGauge, NumberBooleanGauge};
use pay2wash::{AuthenticatedSession, AuthenticatedSessionError};
use prometheus_client::{
    encoding::EncodeLabelSet,
    metrics::{family::Family, gauge::Gauge},
};
use sentry::{types::Dsn, SessionMode};
use serde::Deserialize;
use strict_types::{Email, Password};
use tokio::time::{interval, MissedTickBehavior};
use tracing::{info, warn, Level, trace, debug};
use tracing_error::ErrorLayer;
use tracing_subscriber::{prelude::*, util::SubscriberInitExt, EnvFilter};

use crate::pay2wash::Pay2WashClient;

mod metrics;
mod pay2wash;
mod strict_types;

#[derive(Debug, Deserialize)]
struct Environment {
    pay2wash_email: Email,
    pay2wash_password: Password,

    sentry_dsn: Option<String>,
}

fn main() -> color_eyre::Result<()> {
    // Load environment variables from .env file
    dotenvy::dotenv().ok();

    color_eyre::install()?;

    let environment: Environment = envy::from_env()
        .map_err(|err| match err {
            envy::Error::MissingValue(key) => eyre!("missing environment variable {key}"),
            envy::Error::Custom(message) => eyre!(message),
        })
        .wrap_err("failed to load environment")?;

    // TODO: Sentry
    let _sentry = sentry::init(sentry::ClientOptions {
        attach_stacktrace: true,
        dsn: environment
            .sentry_dsn
            .as_deref()
            .map(Dsn::from_str)
            .transpose()
            .wrap_err("provided DSN is invalid")?,
        default_integrations: true,
        release: Some(Cow::from(git_version::git_version!())),
        session_mode: SessionMode::Request,
        trim_backtraces: true,
        server_name: Some(Cow::from(env!("CARGO_PKG_NAME"))),
        ..Default::default()
    });

    tracing_subscriber::Registry::default()
        .with(tracing_subscriber::fmt::layer().pretty())
        .with(
            EnvFilter::builder()
                .with_default_directive(Level::INFO.into())
                .from_env()
                .wrap_err("failed to parse RUST_LOG")?,
        )
        .with(ErrorLayer::default())
        .init();

    if environment.sentry_dsn.is_none() {
        warn!("no sentry dsn provided, error reporting disabled");
    }

    // Since fly.io is a one core machine, we only need the current thread
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(async_main(environment))
}

async fn async_main(environment: Environment) -> color_eyre::Result<()> {
    info!(?environment);

    let metrics = Metrics::default();

    let mut registry = prometheus_client::registry::Registry::with_prefix("machine");

    registry.register(
        "updated",
        "the UNIX timestamp of when the provided machine_* data was updated per location",
        metrics.updated.clone(),
    );

    registry.register(
        "user_token",
        "the user id whose data is being scraped per location",
        metrics.user_token.clone(),
    );

    registry.register(
        "running",
        "boolean representing the running status of a specific machine",
        metrics.running.clone(),
    );

    registry.register(
        "remaining_time",
        "time remaining on the running program in seconds",
        metrics.remaining_time.clone(),
    );

    registry.register(
        "starter",
        "user id who started this machine",
        metrics.starter.clone(),
    );

    registry.register(
        "reserved",
        "boolean representing if the machine is reserved",
        metrics.reserved.clone(),
    );

    registry.register(
        "reserver",
        "user id who reserved this machine",
        metrics.reserver.clone(),
    );

    registry.register(
        "in_maintenance",
        "boolean representing if the machine is under maintenance",
        metrics.in_maintenance.clone(),
    );

    registry.register(
        "gateway_offline",
        "boolean representing if the machine's gateway is offline",
        metrics.gateway_offline.clone(),
    );

    registry.register(
        "remaining_time_is_from_machine",
        "boolean representing if the machine's remaining_time is provided from the machine itself",
        metrics.remaining_time_is_from_machine.clone(),
    );

    registry.register(
        "controller_logic",
        "unsure",
        metrics.controller_logic.clone(),
    );

    let client = Pay2WashClient::new(environment.pay2wash_email, environment.pay2wash_password);

    tokio::try_join!(metrics::metrics_server(registry), scraper(client, metrics))?;

    Ok(())
}

#[derive(Debug, Default)]
struct Metrics {
    updated: Family<LocationMetricKey, Gauge<i64, AtomicI64>>,
    user_token: Family<LocationMetricKey, Gauge<i64, AtomicI64>>,

    running: Family<WashingMachineMetricKey, BooleanGauge>,
    starter: Family<WashingMachineMetricKey, Gauge<i64, AtomicI64>>,
    remaining_time: Family<WashingMachineMetricKey, Gauge<i64, AtomicI64>>,

    reserved: Family<WashingMachineMetricKey, BooleanGauge>,
    reserver: Family<WashingMachineMetricKey, Gauge<i64, AtomicI64>>,

    in_maintenance: Family<WashingMachineMetricKey, NumberBooleanGauge>,
    gateway_offline: Family<WashingMachineMetricKey, NumberBooleanGauge>,
    remaining_time_is_from_machine: Family<WashingMachineMetricKey, NumberBooleanGauge>,
    controller_logic: Family<WashingMachineMetricKey, Gauge<i64, AtomicI64>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
pub struct LocationMetricKey {
    pub location: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
pub struct WashingMachineMetricKey {
    pub location: String,
    pub name: String,
}

async fn scraper(client: Pay2WashClient, metrics: Metrics) -> color_eyre::Result<Infallible> {
    let mut session: Option<AuthenticatedSession> = None;

    let mut interval = interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        let authenticated_session = if let Some(authenticated_session) = session.as_ref() {
            authenticated_session
        } else {
            let authenticated_session = client
                .authenticate()
                .await
                .wrap_err("failed to authenticate")?;

            &*session.insert(authenticated_session)
        };

        let statuses = match client.get_machine_statuses(authenticated_session).await {
            Ok(statuses) => statuses,
            Err(AuthenticatedSessionError::BadSession) => {
                warn!("authentication session was bad");

                session.take();

                continue;
            }
            Err(AuthenticatedSessionError::Other(error)) => {
                bail!(error);
            }
        };

        let location_key = LocationMetricKey {
            location: authenticated_session.location.clone(),
        };

        metrics.updated.get_or_create(&location_key).set(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("time should only move forwards")
                .as_secs()
                .try_into()
                .expect("unix timestamp should not overflow an i64"),
        );

        metrics
            .user_token
            .get_or_create(&location_key)
            .set(i64::from(u32::from(authenticated_session.user_token)));

        for (name, status) in statuses {
            let metric_key = WashingMachineMetricKey {
                location: authenticated_session.location.clone(),
                name: String::from(name),
            };

            macro_rules! metric {
                ($name:ident) => {
                    metrics
                        .$name
                        .get_or_create(&metric_key)
                        .set(status.raw.$name)
                };
                ($name:ident as i64) => {
                    metrics
                        .$name
                        .get_or_create(&metric_key)
                        .set(i64::from(status.raw.$name))
                };
                ($name:ident as u32 => i64) => {
                    metrics
                        .$name
                        .get_or_create(&metric_key)
                        .set(i64::from(u32::from(status.raw.$name)))
                };
            }

            metric!(running);
            metric!(starter as u32 => i64);

            metrics.remaining_time.get_or_create(&metric_key).set(
                status
                    .raw
                    .remaining_time
                    .into_inner()
                    .as_secs()
                    .try_into()
                    .expect("remaining time should not overflow an i64"),
            );

            metric!(reserved);
            metric!(reserver as u32 => i64);

            metric!(in_maintenance);
            metric!(gateway_offline);
            metric!(remaining_time_is_from_machine);
            metric!(controller_logic as i64);
        }

        debug!(period = ?interval.period(), "waiting for next update");
    }
}
