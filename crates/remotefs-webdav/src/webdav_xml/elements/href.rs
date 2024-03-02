// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::str::FromStr;

use super::super::value::Value;
use super::super::{Element, Error, DAV_NAMESPACE, DAV_PREFIX};

/// The `href` XML element as defined in [RFC 4918](http://webdav.org/specs/rfc4918.html#ELEMENT_href).
#[derive(Clone, Debug, PartialEq)]
pub struct Href(pub http::Uri);

impl Element for Href {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "href";
}

impl TryFrom<&Value> for Href {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        Ok(Self(value.to_str()?.parse().map_err(Error::other)?))
    }
}

impl From<Href> for Value {
    fn from(Href(uri): Href) -> Value {
        Value::Text(uri.to_string().into())
    }
}

impl From<http::Uri> for Href {
    fn from(uri: http::Uri) -> Self {
        Href(uri)
    }
}

impl FromStr for Href {
    type Err = <http::Uri as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        http::Uri::from_str(s).map(Href)
    }
}
