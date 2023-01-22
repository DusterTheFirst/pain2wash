#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used)]

use std::{
    collections::HashMap,
    fmt::{self, Debug},
    ops::Deref,
    time::Duration,
};

use color_eyre::{
    eyre::{bail, eyre, Context, ContextCompat},
    Help, SectionExt,
};
use once_cell::sync::Lazy;
use reqwest::redirect;
use scraper::{ElementRef, Html, Selector};
use serde::{
    de::{self, Visitor},
    Deserialize, Serialize,
};
use tracing::{info, trace, Level};
use tracing_error::ErrorLayer;
use tracing_subscriber::{prelude::*, util::SubscriberInitExt, EnvFilter, Registry};

#[derive(Debug, Deserialize)]
struct Environment {
    pay2wash_email: Email,
    pay2wash_password: Password,
}

#[derive(Deserialize)]
#[serde(transparent)]
struct Password(String);

impl AsRef<str> for Password {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[hidden]")
    }
}

#[derive(Deserialize)]
#[serde(transparent)]
struct Email(String);

impl Deref for Email {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Debug for Email {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
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

    // let session = client
    //     .authenticate()
    //     .await
    //     .wrap_err("failed to authenticate")?;

    // Fake session to test dealing with unauthenticated session
    let session = AuthenticatedSession {
        csrf_token: "".to_string(),
        user_token: "".to_string(),
        location: "89".to_string(),
        machine_mappings: HashMap::from([
            ("476".to_string(), "W2".to_string()),
            ("475".to_string(), "W1".to_string()),
            ("478".to_string(), "W4".to_string()),
            ("479".to_string(), "W5".to_string()),
            ("481".to_string(), "D2".to_string()),
            ("482".to_string(), "D3".to_string()),
            ("477".to_string(), "W3".to_string()),
            ("480".to_string(), "D1".to_string()),
        ]),
    };

    let statuses = client
        .get_machine_statuses(&session)
        .await
        .wrap_err("failed to get machine statuses")?;

    dbg!(statuses);

    let session = client
        .authenticate()
        .await
        .wrap_err("failed to authenticate")?;

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
                .redirect(redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() == 5 {
                        // Do not redirect if chain is longer than 5 redirects
                        attempt.stop()
                    } else if attempt
                        .previous()
                        .last()
                        .expect("chain should have at least one url")
                        .path()
                        .starts_with("/machine_statuses/")
                    {
                        // Do not redirect away from "api" routes
                        attempt.stop()
                    } else {
                        attempt.follow()
                    }
                }))
                .build()
                .expect("reqwest client configuration should be valid"),
        }
    }

    #[tracing::instrument]
    async fn authenticate(&self) -> color_eyre::Result<AuthenticatedSession> {
        const LOGIN_PAGE: &str = "https://holland2stay.pay2wash.app/login";

        trace!(LOGIN_PAGE, "fetching login form for CSRF token");

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

        trace!("received login form");

        let html = Html::parse_document(&document);

        trace!("parsed login form html");

        let session = extract_session(html)
            .wrap_err("failed to extract session information from document")
            .note("the html returned by the server may have changed")?;

        trace!("extracted session information from login form");

        let session = match session {
            Pay2WashSession::Authenticated(session) => {
                info!("attempted to authenticate while in an authenticated session");

                return Ok(session);
            }
            Pay2WashSession::Unauthenticated(session) => session,
        };

        #[derive(Debug, Serialize)]
        struct LoginForm<'s> {
            _token: &'s str,
            email: &'s str,
            password: &'s str,
        }

        let login_form = LoginForm {
            _token: &session.csrf_token,
            email: &self.email,
            password: self.password.as_ref(),
        };

        trace!(?login_form, "submitting login form");

        let response = self
            .http_client
            .post(LOGIN_PAGE)
            .form(&login_form)
            .send()
            .await
            .wrap_err("failed to POST `/login` form")?
            .error_for_status()
            .wrap_err("server responded with non-success status code")?;

        trace!("login form submitted successfully");

        let document = response
            .text()
            .await
            .wrap_err("failed to receive response from server")?;

        trace!("received webpage html");

        let html = Html::parse_document(&document);

        trace!("parsed webpage html");

        let session = extract_session(html)
            .wrap_err("failed to extract session information from document")
            .note("the html returned by the server may have changed")?;

        trace!("extracted session information from login form");

        match session {
            Pay2WashSession::Authenticated(authenticated_session) => Ok(authenticated_session),
            _ => bail!("failed to achieve an authenticated sessions"),
        }
    }

    #[tracing::instrument]
    async fn get_machine_statuses<'session>(
        &self,
        session: &'session AuthenticatedSession,
    ) -> color_eyre::Result<HashMap<&'session str, MachineStatus>> {
        let response = self
            .http_client
            .get(format!(
                "https://holland2stay.pay2wash.app/machine_statuses/{}",
                session.location
            ))
            .send()
            .await
            .wrap_err("failed to GET `/machine_statuses/{ID}`")?
            .error_for_status()
            .wrap_err("server responded with non-success status code")?;

        if response.status().is_redirection() {
            // TODO: better handling of this case
            bail!("session is no longer authenticated");
        }

        let document = response
            .text()
            .await
            .wrap_err("failed to receive response from server")?;

        let statuses: HashMap<&str, MachineStatus> = serde_json::from_str(&document)
            .wrap_err("failed to deserialize json data from server")
            .with_section(|| document.clone().header("JSON"))?;

        statuses
            .into_iter()
            .map(|(key, value)| {
                session
                    .machine_mappings
                    .get(key)
                    .map(|new_key| (new_key.as_str(), value))
                    .wrap_err_with(|| format!("key {key} isl not in machine_mappings"))
            })
            .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct UserId(u32);

#[derive(Debug)]
enum NumberBool {
    False,
    True,
    Unknown(u8),
}

impl From<u8> for NumberBool {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::False,
            1 => Self::True,
            _ => Self::Unknown(value),
        }
    }
}

impl TryFrom<NumberBool> for bool {
    type Error = u8;

    fn try_from(value: NumberBool) -> Result<Self, Self::Error> {
        match value {
            NumberBool::False => Ok(false),
            NumberBool::True => Ok(true),
            NumberBool::Unknown(unknown) => Err(unknown),
        }
    }
}

impl<'de> Deserialize<'de> for NumberBool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct NumberBoolVisitor;

        impl<'v> Visitor<'v> for NumberBoolVisitor {
            type Value = u8;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("boolean represented as an integer")
            }

            fn visit_u8<E>(self, v: u8) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(v)
            }

            fn visit_u16<E>(self, v: u16) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                v.try_into().map_err(E::custom)
            }

            fn visit_u32<E>(self, v: u32) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                v.try_into().map_err(E::custom)
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                v.try_into().map_err(E::custom)
            }

            fn visit_u128<E>(self, v: u128) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                v.try_into().map_err(E::custom)
            }
        }

        deserializer
            .deserialize_u8(NumberBoolVisitor)
            .map(NumberBool::from)
    }
}

#[derive(Debug)]
struct RemainingTime(Duration);

impl<'de> Deserialize<'de> for RemainingTime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RemainingTimeVisitor;

        impl<'v> Visitor<'v> for RemainingTimeVisitor {
            type Value = Duration;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a duration formatted as HH:MM")
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&v)
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let (hours, minutes) = v
                    .split_once(':')
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Str(v), &self))?;

                let hours = (hours.len() <= 2).then_some(hours).ok_or_else(|| {
                    E::invalid_value(de::Unexpected::Str(v), &"too many digits in hours place")
                })?;
                let minutes = (minutes.len() <= 2).then_some(minutes).ok_or_else(|| {
                    E::invalid_value(de::Unexpected::Str(v), &"too many digits in minutes place")
                })?;

                let hours: u8 = hours.parse().map_err(E::custom)?;
                let minutes: u8 = minutes.parse().map_err(E::custom)?;

                Ok(Duration::from_secs(
                    60 * 60 * u64::from(hours) + 60 * u64::from(minutes),
                ))
            }
        }

        deserializer.deserialize_str(RemainingTimeVisitor).map(Self)
    }
}

#[derive(Debug, Deserialize)]
struct MachineStatus {
    running: bool,
    starter: UserId,
    reserved: bool,
    reserver: UserId,
    in_maintenance: NumberBool,
    remaining_time: RemainingTime,
    gateway_offline: NumberBool,
    remaining_time_is_from_machine: NumberBool,
    // Unknown
    controller_logic: u32,
}

#[derive(Debug)]
enum Pay2WashSession {
    Unauthenticated(UnauthenticatedSession),
    Authenticated(AuthenticatedSession),
}

#[derive(Debug)]
struct UnauthenticatedSession {
    csrf_token: String,
}

#[derive(Debug)]
struct AuthenticatedSession {
    csrf_token: String,
    user_token: String,
    location: String,
    machine_mappings: HashMap<String, String>,
}

impl Pay2WashSession {
    pub fn csrf_token(&self) -> &str {
        match self {
            Pay2WashSession::Unauthenticated(UnauthenticatedSession { csrf_token }) => csrf_token,
            Pay2WashSession::Authenticated(AuthenticatedSession { csrf_token, .. }) => csrf_token,
        }
    }
}

#[tracing::instrument(skip_all)]
// TODO: make better, once_cell, better error handling
fn extract_session(html: Html) -> color_eyre::Result<Pay2WashSession> {
    static CSRF_SELECTOR: Lazy<Selector> = Lazy::new(|| {
        Selector::parse("meta[name=csrf-token]").expect("css selector should be valid")
    });
    static USER_TOKEN_SELECTOR: Lazy<Selector> = Lazy::new(|| {
        Selector::parse("meta[name=user-token]").expect("css selector should be valid")
    });
    static LOCATION_SELECTOR: Lazy<Selector> =
        Lazy::new(|| Selector::parse("#location").expect("css selector should be valid"));
    static MACHINE_ID_SELECTOR: Lazy<Selector> =
        Lazy::new(|| Selector::parse("input.machine_pk").expect("css selector should be valid"));
    static MACHINE_NAME_SELECTOR: Lazy<Selector> =
        Lazy::new(|| Selector::parse("span.js-reservation").expect("css selector should be valid"));

    let csrf_token = html
        .select(&CSRF_SELECTOR)
        .next()
        .wrap_err("csrf selector failed to select any element")?
        .value()
        .attr("content")
        .wrap_err("csrf meta tag did not have `value` attribute")
        .warning("this should never happen as all meta tags should have a content attribute")?
        .to_owned();

    let user_token = html
        .select(&USER_TOKEN_SELECTOR)
        .next()
        .wrap_err("user token selector failed to select any element")?
        .value()
        .attr("content")
        .wrap_err("user token meta tag did not have `value` attribute")
        .warning("this should never happen as all meta tags should have a content attribute")?;

    if user_token.is_empty() {
        Ok(Pay2WashSession::Unauthenticated(UnauthenticatedSession {
            csrf_token,
        }))
    } else {
        let location = html
            .select(&LOCATION_SELECTOR)
            .next()
            .wrap_err("location selector failed to select any element")?
            .value()
            .attr("value")
            .wrap_err("#location did not have value attribute")?;

        let machine_mappings = html
            .select(&MACHINE_ID_SELECTOR)
            .map(|element| {
                Ok((
                    element
                        .value()
                        .attr("value")
                        .wrap_err("machine id element does not have any value attribute")
                        .with_section(|| format!("{:?}", element.value()).header("Element:"))?
                        .to_owned(),
                    {
                        let parent = element
                            .parent()
                            .and_then(ElementRef::wrap)
                            .wrap_err("element does not have have parent")
                            .with_section(|| format!("{:?}", element.value()).header("Element:"))?;

                        parent
                            .select(&MACHINE_NAME_SELECTOR)
                            .next()
                            .wrap_err("machine name selector failed to select any element")
                            .with_section(|| format!("{:?}", parent.value()).header("Element:"))?
                            .text()
                            .next()
                            .wrap_err("element does not have any text nodes")
                            .with_section(|| format!("{:?}", parent.value()).header("Element:"))?
                            .trim()
                            .to_owned()
                    },
                ))
            })
            .collect::<color_eyre::Result<HashMap<String, String>>>()?;

        Ok(Pay2WashSession::Authenticated(AuthenticatedSession {
            csrf_token,
            user_token: user_token.to_owned(),
            location: location.to_owned(),
            machine_mappings,
        }))
    }
}
