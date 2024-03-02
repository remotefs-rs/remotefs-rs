// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::super::value::ValueMap;
use super::super::{Element, Error, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `resourcetype` property as defined in
/// [RFC 4918](http://webdav.org/specs/rfc4918.html#PROPERTY_resourcetype).
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceType(ValueMap);

impl Element for ResourceType {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "resourcetype";
}

impl TryFrom<&Value> for ResourceType {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        value.to_map().cloned().map(Self)
    }
}

impl From<ResourceType> for Value {
    fn from(ResourceType(map): ResourceType) -> Value {
        Value::Map(map)
    }
}

/// The `collection` XML element as defined in
/// [RFC 4918](http://webdav.org/specs/rfc4918.html#ELEMENT_collection).
pub struct Collection;

impl Element for Collection {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "collection";
}
