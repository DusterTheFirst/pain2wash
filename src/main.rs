#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used)]

use std::{
    fmt::{self, Debug},
    ops::Deref,
};

use color_eyre::eyre::{bail, eyre, Context, ContextCompat};
use reqwest::redirect;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use tracing::{info, Level};
use tracing_error::ErrorLayer;
use tracing_subscriber::{prelude::*, util::SubscriberInitExt, EnvFilter, Registry};
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
struct Environment {
    #[validate]
    pay2wash_email: Email,
    #[validate]
    pay2wash_password: Password,
}

#[derive(Deserialize, Validate)]
#[serde(transparent)]
struct Password {
    #[validate(non_control_character)]
    inner: String,
}

impl AsRef<str> for Password {
    fn as_ref(&self) -> &str {
        &self.inner
    }
}

impl Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[hidden]")
    }
}

#[derive(Deserialize, Validate)]
#[serde(transparent)]
struct Email {
    #[validate(email)]
    inner: String,
}

impl Deref for Email {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Debug for Email {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
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

    environment.validate().wrap_err("environment is invalid")?;

    // Since fly.io is a one core machine, we only need the current thread
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(async_main(environment))
}

async fn async_main(environment: Environment) -> color_eyre::Result<()> {
    info!(?environment, "Main entered");

    let client = Pay2WashClient::new(environment.pay2wash_email, environment.pay2wash_password);

    client.authenticate().await?;

    Ok(())
}

#[derive(Debug)]
struct Pay2WashClient {
    email: Email,
    password: Password,
    http_client: reqwest::Client,
}

impl Pay2WashClient {
    fn new(email: Email, password: Password) -> Self {
        Self {
            email,
            password,
            http_client: reqwest::Client::builder()
                .cookie_store(true)
                .redirect(redirect::Policy::limited(5))
                .build()
                .expect("reqwest client configuration should be valid"),
        }
    }

    // Is logged in == meta[name=user-token] has content?

    #[tracing::instrument]
    async fn authenticate(&self) -> color_eyre::Result<()> {
        const LOGIN_PAGE: &str = "https://holland2stay.pay2wash.app/login";

        let response = self
            .http_client
            .get(LOGIN_PAGE)
            .send()
            .await
            .wrap_err("failed to GET `/login` form")?
            .error_for_status()
            .wrap_err("server responded with non-success status code")?;

        let document = response
            .text()
            .await
            .wrap_err("failed to receive response from server")?;

        let html = Html::parse_document(&document);

        let session = extract_session(&html)
            .wrap_err("failed to extract session information from document")?;

        dbg!(&session);

        #[derive(Debug, Serialize)]
        struct LoginForm<'s> {
            _token: &'s str,
            email: &'s str,
            password: &'s str,
        }

        let response = self
            .http_client
            .post(LOGIN_PAGE)
            .form(&LoginForm {
                _token: session.csrf_token(),
                email: &self.email,
                password: self.password.as_ref(),
            })
            .send()
            .await
            .wrap_err("failed to POST `/login` form")?
            .error_for_status()
            .wrap_err("server responded with non-success status code")?;

        let document = response
            .text()
            .await
            .wrap_err("failed to receive response from server")?;

        let html = Html::parse_document(&document);

        let session = extract_session(&html)
            .wrap_err("failed to extract session information from document")?;

        dbg!(&session);

        let Pay2WashSession::Authenticated { location, .. } = session else {
            bail!("session should be authenticated by now");
        };

        let response = self
            .http_client
            .get(format!(
                "https://holland2stay.pay2wash.app/machine_statuses/{location}"
            ))
            .send()
            .await
            .wrap_err("failed to GET `/machine_statuses/{ID}`")?
            .error_for_status()
            .wrap_err("server responded with non-success status code")?;

        let document = response
            .text()
            .await
            .wrap_err("failed to receive response from server")?;

        println!("{document}");

        Ok(())
    }
}

#[derive(Debug)]
enum Pay2WashSession<'s> {
    Unauthenticated {
        csrf_token: &'s str,
    },
    Authenticated {
        csrf_token: &'s str,
        user_token: &'s str,
        location: &'s str,
    },
}

impl<'s> Pay2WashSession<'s> {
    pub fn csrf_token(&self) -> &str {
        match self {
            Pay2WashSession::Unauthenticated { csrf_token } => csrf_token,
            Pay2WashSession::Authenticated { csrf_token, .. } => csrf_token,
        }
    }
}

#[tracing::instrument(skip_all)]
// TODO: make better, once_cell, better error handling
fn extract_session<'html>(html: &'html Html) -> color_eyre::Result<Pay2WashSession<'html>> {
    let csrf_token = html
        .select(&Selector::parse("meta[name=csrf-token]").expect("css selector should be valid"))
        .next()
        .wrap_err("selector failed to select any element")?
        .value()
        .attr("content")
        .wrap_err("meta[name=csrf-token] did not have value attribute")?;

    let user_token = html
        .select(&Selector::parse("meta[name=user-token]").expect("css selector should be valid"))
        .next()
        .wrap_err("selector failed to select any element")?
        .value()
        .attr("content")
        .wrap_err("meta[name=csrf-token] did not have value attribute")?;

    if user_token.is_empty() {
        Ok(Pay2WashSession::Unauthenticated { csrf_token })
    } else {
        let location = html
            .select(&Selector::parse("#location").expect("css selector should be valid"))
            .next()
            .wrap_err("selector failed to select any element")?
            .value()
            .attr("value")
            .wrap_err("#location did not have value attribute")?;

        Ok(Pay2WashSession::Authenticated {
            csrf_token,
            user_token,
            location,
        })
    }
}
