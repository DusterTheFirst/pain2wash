use color_eyre::{
    eyre::{bail, eyre, Context},
    Help, SectionExt,
};
use once_cell::sync::Lazy;
use reqwest::redirect;
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use thiserror::Error;
use tracing::{info, trace};

use std::{
    collections::HashMap,
    fmt::{self, Debug},
};

use crate::strict_types::{Email, Password, PasswordRef};

use self::model::{JsonMachineStatus, MachineState, MachineStatus, UserId};

pub mod model;

pub struct Pay2WashClient {
    email: Email,
    password: Password,
    http_client: reqwest::Client,
}

impl Debug for Pay2WashClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pay2WashClient")
            .field("email", &self.email)
            .field("password", &self.password)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub enum AuthenticatedSessionError {
    #[error("session is no longer authenticated")]
    BadSession,
    #[error(transparent)]
    Other(#[from] color_eyre::Report),
}

const LOGIN_PAGE: &str = "https://holland2stay.pay2wash.app/login";

impl Pay2WashClient {
    pub fn new(email: Email, password: Password) -> Self {
        Self {
            email,
            password,
            http_client: reqwest::Client::builder()
                .cookie_store(true)
                .redirect(redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() == 5
                        || attempt
                            .previous()
                            .last()
                            .expect("chain should have at least one url")
                            .path()
                            .starts_with("/machine_statuses/")
                    {
                        // Do not redirect if chain is longer than 5 redirects
                        // or request is to "api" routes
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
    pub async fn authenticate(&self) -> color_eyre::Result<AuthenticatedSession> {
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

        let unauthenticated_session = match session {
            Pay2WashSession::Authenticated(session) => {
                info!("attempted to authenticate while in an authenticated session");

                return Ok(session);
            }
            Pay2WashSession::Unauthenticated(session) => session,
        };

        self.authenticate_from_unauthenticated_session(unauthenticated_session)
            .await
    }

    pub async fn authenticate_from_unauthenticated_session(
        &self,
        session: UnauthenticatedSession,
    ) -> color_eyre::Result<AuthenticatedSession> {
        #[derive(Serialize, Debug)]
        struct LoginForm<'s> {
            _token: &'s str,
            email: &'s str,
            password: PasswordRef<'s>
        }

        let login_form = LoginForm {
            _token: &session.csrf_token,
            email: &self.email,
            password: self.password.as_ref(),
        };

        trace!(?login_form, LOGIN_PAGE, "submitting login form");

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
    pub async fn get_machine_statuses<'session>(
        &self,
        session: &'session AuthenticatedSession,
    ) -> Result<HashMap<&'session str, MachineStatus>, AuthenticatedSessionError> {
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
            return Err(AuthenticatedSessionError::BadSession);
        }

        let document = response
            .text()
            .await
            .wrap_err("failed to receive response from server")?;

        let statuses: HashMap<&str, JsonMachineStatus> = serde_json::from_str(&document)
            .wrap_err("failed to deserialize json data from server")
            .with_section(|| document.clone().header("JSON"))?;

        let statuses = statuses
            .into_iter()
            .map(|(key, value)| {
                if let Some(new_key) = session.machine_mappings.get(key) {
                    Ok((
                        new_key.as_str(),
                        MachineStatus {
                            state: MachineState::try_from(&value).wrap_err_with(|| {
                                format!("encountered problem decoding machine status: {value:?}")
                            })?,
                            raw: value,
                        },
                    ))
                } else {
                    Err(eyre!("key {key} is not in machine_mappings"))
                }
            })
            .collect::<color_eyre::Result<_>>()?;

        Ok(statuses)
    }
}

#[derive(Debug)]
pub enum Pay2WashSession {
    Unauthenticated(UnauthenticatedSession),
    Authenticated(AuthenticatedSession),
}

#[derive(Debug)]
pub struct UnauthenticatedSession {
    pub csrf_token: String,
}

#[derive(Debug)]
pub struct AuthenticatedSession {
    pub csrf_token: String,
    pub user_token: UserId,
    pub location: String,
    pub machine_mappings: HashMap<String, String>,
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
// TODO: make better error handling
pub(crate) fn extract_session(html: Html) -> color_eyre::Result<Pay2WashSession> {
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
        .ok_or_else(|| eyre!("csrf selector failed to select any element"))?
        .value()
        .attr("content")
        .ok_or_else(|| {
            eyre!("csrf meta tag did not have `value` attribute").warning(
                "this should never happen as all meta tags should have a content attribute",
            )
        })?
        .to_owned();

    let user_token = html
        .select(&USER_TOKEN_SELECTOR)
        .next()
        .ok_or_else(|| eyre!("user token selector failed to select any element"))?
        .value()
        .attr("content")
        .ok_or_else(|| {
            eyre!("user token meta tag did not have `value` attribute").warning(
                "this should never happen as all meta tags should have a content attribute",
            )
        })?;

    if user_token.is_empty() {
        Ok(Pay2WashSession::Unauthenticated(UnauthenticatedSession {
            csrf_token,
        }))
    } else {
        let location = html
            .select(&LOCATION_SELECTOR)
            .next()
            .ok_or_else(|| eyre!("location selector failed to select any element"))?
            .value()
            .attr("value")
            .ok_or_else(|| eyre!("#location did not have value attribute"))?;

        let machine_mappings = html
            .select(&MACHINE_ID_SELECTOR)
            .map(|element| {
                Ok((
                    element
                        .value()
                        .attr("value")
                        .ok_or_else(|| {
                            eyre!("machine id element does not have any value attribute")
                        })
                        .with_section(|| format!("{:?}", element.value()).header("Element:"))?
                        .to_owned(),
                    {
                        let parent =
                            element.parent().and_then(ElementRef::wrap).ok_or_else(|| {
                                eyre!("element does not have have parent").with_section(|| {
                                    format!("{:?}", element.value()).header("Element:")
                                })
                            })?;

                        parent
                            .select(&MACHINE_NAME_SELECTOR)
                            .next()
                            .ok_or_else(|| {
                                eyre!("machine name selector failed to select any element")
                                    .with_section(|| {
                                        format!("{:?}", parent.value()).header("Element:")
                                    })
                            })?
                            .text()
                            .next()
                            .ok_or_else(|| {
                                eyre!("element does not have any text nodes").with_section(|| {
                                    format!("{:?}", parent.value()).header("Element:")
                                })
                            })?
                            .trim()
                            .to_owned()
                    },
                ))
            })
            .collect::<color_eyre::Result<HashMap<String, String>>>()?;

        Ok(Pay2WashSession::Authenticated(AuthenticatedSession {
            csrf_token,
            user_token: user_token
                .parse()
                .wrap_err("user_token was a non-integer")?,
            location: location.to_owned(),
            machine_mappings,
        }))
    }
}
