// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::super::{Element, Error, Todo, Value, DAV_NAMESPACE, DAV_PREFIX};

/// Not yet implemented.
///
/// The `supportedlock` property as defined in
/// [RFC 4918](http://webdav.org/specs/rfc4918.html#PROPERTY_supportedlock).
#[derive(Clone, Debug, PartialEq)]
pub struct SupportedLock(Todo);

impl Element for SupportedLock {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "supportedlock";
}

impl TryFrom<&Value> for SupportedLock {
    type Error = Error;

    fn try_from(_: &Value) -> Result<Self, Self::Error> {
        todo!()
    }
}

impl From<SupportedLock> for Value {
    fn from(_: SupportedLock) -> Value {
        todo!()
    }
}
