// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::super::element::Element;
use super::super::properties::{ContentLength, CreationDate, LastModified};
use super::super::value::{Value, ValueMap};
use super::super::{Error, DAV_NAMESPACE, DAV_PREFIX};

/// The `prop` XML element as defined in [RFC 4918](http://webdav.org/specs/rfc4918.html#ELEMENT_prop).
///
/// This element can contain arbitrary child elements and supports extracting
/// them using [`Properties::get()`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Properties(ValueMap);

impl Properties {
    /// Read a specific property from this `prop` element.
    ///
    /// Returns
    /// - `None` if the property doesn't exist
    /// - `Some(None)` if the property exists and is empty
    /// - `Some(Some(Ok(_)))` if the property exists and was successfully
    ///   extracted
    /// - `Some(Some(Err(_)))` if the property exists and extraction failed
    pub fn get<'v, P>(&'v self) -> Option<Option<Result<P, Error>>>
    where
        P: Element + TryFrom<&'v Value, Error = Error>,
    {
        self.0.get_optional()
    }
}

impl Properties {
    /// Read the `creationdate` property.
    ///
    /// See [`Properties::get()`] for an overview of the possible return values.
    pub fn creationdate(&self) -> Option<Option<Result<CreationDate, Error>>> {
        self.get()
    }

    /// Read the `getcontentlength` property.
    ///
    /// See [`Properties::get()`] for an overview of the possible return values.
    pub fn getcontentlength(&self) -> Option<Option<Result<ContentLength, Error>>> {
        self.get()
    }

    /// Read the `getlastmodified` property.
    ///
    /// See [`Properties::get()`] for an overview of the possible return values.
    pub fn getlastmodified(&self) -> Option<Option<Result<LastModified, Error>>> {
        self.get()
    }
}

impl Element for Properties {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "prop";
}

impl TryFrom<&Value> for Properties {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        value.to_map().cloned().map(Self)
    }
}

impl From<Properties> for Value {
    fn from(Properties(map): Properties) -> Value {
        Value::Map(map)
    }
}
