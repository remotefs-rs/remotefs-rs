// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::super::{Element, Error, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `getcontentlength` property as defined in
/// [RFC 4918](http://webdav.org/specs/rfc4918.html#PROPERTY_getcontentlength).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentLength(pub u64);

impl Element for ContentLength {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "getcontentlength";
}

impl TryFrom<&Value> for ContentLength {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        value.to_str()?.parse().map(Self).map_err(Error::other)
    }
}

impl From<ContentLength> for Value {
    fn from(ContentLength(len): ContentLength) -> Value {
        len.to_string().into()
    }
}
