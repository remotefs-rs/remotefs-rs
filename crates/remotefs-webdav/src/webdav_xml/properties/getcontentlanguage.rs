// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use bytestring::ByteString;

use super::super::{Element, Error, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `getcontentlanguage` property as defined in
/// [RFC 4918](http://webdav.org/specs/rfc4918.html#PROPERTY_getcontentlanguage).
#[derive(Clone, Debug, PartialEq)]
pub struct ContentLanguage(pub ByteString);

impl Element for ContentLanguage {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "getcontentlanguage";
}

impl TryFrom<&Value> for ContentLanguage {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        Ok(Self(value.to_str()?.clone()))
    }
}

impl From<ContentLanguage> for Value {
    fn from(ContentLanguage(s): ContentLanguage) -> Value {
        Value::Text(s)
    }
}
