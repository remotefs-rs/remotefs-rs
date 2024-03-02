// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::super::{Element, Error, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `creationdate` property as defined in [RFC 4918](http://webdav.org/specs/rfc4918.html#PROPERTY_creationdate).
#[derive(Clone, Debug, PartialEq)]
pub struct CreationDate(pub OffsetDateTime);

impl Element for CreationDate {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "creationdate";
}

impl TryFrom<&Value> for CreationDate {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        OffsetDateTime::parse(value.to_str()?, &Rfc3339)
            .map(Self)
            .map_err(Error::other)
    }
}

impl From<CreationDate> for Value {
    fn from(CreationDate(datetime): CreationDate) -> Value {
        datetime.format(&Rfc3339).unwrap().into()
    }
}
