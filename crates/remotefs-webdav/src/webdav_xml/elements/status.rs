// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fmt::Display;
use std::str::FromStr;

use super::super::{Element, Error, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `status` XML element as defined in [RFC 4918](http://webdav.org/specs/rfc4918.html#ELEMENT_status).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Status(pub http::StatusCode);

impl Element for Status {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "status";
}

impl From<http::StatusCode> for Status {
    fn from(code: http::StatusCode) -> Self {
        Self(code)
    }
}

impl FromStr for Status {
    type Err = InvalidStatus;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split(' ');
        parts.next().ok_or_else(|| InvalidStatus(s.into()))?;
        let status_code = parts
            .next()
            .and_then(|code| code.parse().ok())
            .ok_or_else(|| InvalidStatus(s.into()))?;
        Ok(Self(status_code))
    }
}

impl Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("HTTP/1.1 {}", self.0))
    }
}
impl TryFrom<&Value> for Status {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        value.to_str()?.parse().map_err(Error::other)
    }
}
impl From<Status> for Value {
    fn from(status: Status) -> Value {
        status.to_string().into()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid status: {0}")]
pub struct InvalidStatus(String);
