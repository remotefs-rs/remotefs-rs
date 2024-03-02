// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use mime::Mime;

use super::super::{Element, Error, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `getcontenttype` property as defined in
/// [RFC 4918](http://webdav.org/specs/rfc4918.html#PROPERTY_getcontenttype).
#[derive(Clone, Debug, PartialEq)]
pub struct ContentType(pub Mime);

impl Element for ContentType {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "getcontenttype";
}

impl TryFrom<&Value> for ContentType {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        value.to_str()?.parse().map(Self).map_err(Error::other)
    }
}

impl From<ContentType> for Value {
    fn from(ContentType(content_type): ContentType) -> Value {
        content_type.to_string().into()
    }
}
