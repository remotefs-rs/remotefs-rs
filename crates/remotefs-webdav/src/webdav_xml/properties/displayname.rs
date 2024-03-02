// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use bytestring::ByteString;

use super::super::{Element, Error, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `displayname` property as defined in
/// [RFC 4918](http://webdav.org/specs/rfc4918.html#PROPERTY_displayname).
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayName(pub ByteString);

impl Element for DisplayName {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "displayname";
}

impl TryFrom<&Value> for DisplayName {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        Ok(Self(value.to_str()?.clone()))
    }
}

impl From<DisplayName> for Value {
    fn from(DisplayName(s): DisplayName) -> Value {
        Value::Text(s)
    }
}
