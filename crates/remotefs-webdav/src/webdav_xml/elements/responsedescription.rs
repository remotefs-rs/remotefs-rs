// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use bytestring::ByteString;

use super::super::{Element, Error, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `responsedescription` XML element as defined in [RFC 4918](http://webdav.org/specs/rfc4918.html#ELEMENT_responsedescription).
#[derive(Clone, Debug, PartialEq)]
pub struct ResponseDescription(pub ByteString);

impl Element for ResponseDescription {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "responsedescription";
}

impl TryFrom<&Value> for ResponseDescription {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        Ok(Self(value.to_str()?.to_owned()))
    }
}

impl From<ResponseDescription> for Value {
    fn from(ResponseDescription(s): ResponseDescription) -> Value {
        Value::Text(s)
    }
}

impl<S: Into<ByteString>> From<S> for ResponseDescription {
    fn from(s: S) -> Self {
        ResponseDescription(s.into())
    }
}
