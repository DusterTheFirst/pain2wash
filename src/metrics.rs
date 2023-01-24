use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::Arc,
};

use axum::{extract::State, response::Redirect, routing::get, Router, Server};
use color_eyre::{eyre::Context, Report};
use prometheus_client::registry::Registry;
use reqwest::StatusCode;
use sentry_tower::{SentryHttpLayer, SentryLayer};
use tower_http::{catch_panic::CatchPanicLayer, trace::TraceLayer};
use tracing::{error, info};

pub mod boolean;
pub mod gauge_info;

pub async fn metrics_server(registry: Registry) -> Result<(), Report> {
    let router = Router::new()
        .route("/metrics", get(metrics).with_state(Arc::new(registry)))
        .fallback(|| async { Redirect::to("/metrics") })
        .layer(
            tower::ServiceBuilder::new()
                .layer(SentryLayer::new_from_top())
                .layer(SentryHttpLayer::with_transaction())
                .layer(TraceLayer::new_for_http())
                .layer(CatchPanicLayer::new()),
        );

    info!("Starting metrics server on http://localhost:9091");

    let listen = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 9091);
    Server::bind(&listen.into())
        .serve(router.into_make_service())
        .await
        .wrap_err("axum server ran into a problem")
}

#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn metrics(State(registry): State<Arc<Registry>>) -> Result<String, StatusCode> {
    let mut buffer = String::new();

    // TODO: "application/openmetrics-text; version=1.0.0; charset=utf-8"
    match prometheus_client::encoding::text::encode(&mut buffer, &registry) {
        Ok(()) => Ok(buffer),
        Err(error) => {
            error!(?error, "failed to encode prometheus data");

            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
