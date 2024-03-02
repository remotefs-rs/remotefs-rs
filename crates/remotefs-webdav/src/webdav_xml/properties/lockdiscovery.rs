// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::super::{Element, Error, Todo, Value, DAV_NAMESPACE, DAV_PREFIX};

/// Not yet implemented.
///
/// The `lockdiscovery` property as defined in
/// [RFC 4918](http://webdav.org/specs/rfc4918.html#PROPERTY_lockdiscovery).
#[derive(Clone, Debug, PartialEq)]
pub struct LockDiscovery(Todo);

impl Element for LockDiscovery {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "lockdiscovery";
}

impl TryFrom<&Value> for LockDiscovery {
    type Error = Error;

    fn try_from(_: &Value) -> Result<Self, Self::Error> {
        todo!()
    }
}

impl From<LockDiscovery> for Value {
    fn from(_: LockDiscovery) -> Value {
        todo!()
    }
}
