// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use bytestring::ByteString;

use super::super::elements::Properties;
use super::super::{Element, Error, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `propfind` XML element as defined in [RFC 4918](http://webdav.org/specs/rfc4918.html#ELEMENT_propfind).
#[derive(Clone, Debug, PartialEq)]
pub enum Propfind {
    Propname,
    Allprop { include: Option<Include> },
    Prop(Properties),
}

impl Element for Propfind {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "propfind";
}

impl TryFrom<&Value> for Propfind {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        let map = value.to_map()?;

        match (
            map.get::<Propname>(),
            map.get::<Allprop>(),
            map.get::<Properties>(),
        ) {
            (Some(_), None, None) => Ok(Propfind::Propname),
            (None, Some(_), None) => Ok(Propfind::Allprop {
                include: map.get().transpose()?,
            }),
            (None, None, Some(prop)) => Ok(Propfind::Prop(prop?)),
            _ => Err(Error::ConflictingElements(
                "`propname`, `allprop` and `include` must not be used at the same time",
            )),
        }
    }
}

/// The `propname` XML element as defined in [RFC 4918](http://webdav.org/specs/rfc4918.html#ELEMENT_propname).
#[derive(Clone, Debug, PartialEq)]
pub struct Propname;

impl Element for Propname {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "propname";
}

impl TryFrom<&Value> for Propname {
    type Error = Error;

    fn try_from(_: &Value) -> Result<Self, Self::Error> {
        Ok(Propname)
    }
}

/// The `allprop` XML element as defined in [RFC 4918](http://webdav.org/specs/rfc4918.html#ELEMENT_allprop).
#[derive(Clone, Debug, PartialEq)]
pub struct Allprop;

impl Element for Allprop {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "allprop";
}

impl TryFrom<&Value> for Allprop {
    type Error = Error;

    fn try_from(_: &Value) -> Result<Self, Self::Error> {
        Ok(Allprop)
    }
}

/// The `include` XML element as defined in [RFC 4918](http://webdav.org/specs/rfc4918.html#ELEMENT_include).
#[derive(Clone, Debug, PartialEq)]
pub struct Include(Vec<ByteString>);

impl Element for Include {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "include";
}

impl TryFrom<&Value> for Include {
    type Error = Error;

    fn try_from(_: &Value) -> Result<Self, Self::Error> {
        todo!()
    }
}
