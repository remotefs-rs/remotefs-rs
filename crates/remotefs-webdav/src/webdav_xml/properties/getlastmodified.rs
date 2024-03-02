// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use httpdate::HttpDate;

use super::super::{Element, Error, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `getlastmodified` property as defined in
/// [RFC 4918](http://webdav.org/specs/rfc4918.html#PROPERTY_getlastmodified).
#[derive(Clone, Debug, PartialEq)]
pub struct LastModified(pub HttpDate);

impl Element for LastModified {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "getlastmodified";
}

impl TryFrom<&Value> for LastModified {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        value.to_str()?.parse().map(Self).map_err(Error::other)
    }
}

impl From<LastModified> for Value {
    fn from(LastModified(date): LastModified) -> Value {
        date.to_string().into()
    }
}
