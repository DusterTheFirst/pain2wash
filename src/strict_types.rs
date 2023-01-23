use std::{fmt, ops::Deref};

use std::fmt::Debug;

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(transparent)]
pub struct Password(String);

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
pub struct Email(String);

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
