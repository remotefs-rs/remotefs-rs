// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::super::elements::{Properties, ResponseDescription, Status};
use super::super::value::ValueMap;
use super::super::{Element, Error, OptionExt, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `propstat` XML element as defined in [RFC 4918](http://webdav.org/specs/rfc4918.html#ELEMENT_propstat).
#[derive(Clone, Debug, PartialEq)]
pub struct Propstat {
    pub prop: Properties,
    pub status: Status,
    // pub error: Option<Error>,
    pub responsedescription: Option<ResponseDescription>,
}

impl Element for Propstat {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "propstat";
}

impl TryFrom<&Value> for Propstat {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        let map = value.to_map()?;
        Ok(Self {
            prop: map.get().required::<Properties>()??,
            status: map.get().required::<Status>()??,
            responsedescription: map.get().transpose()?,
        })
    }
}

impl From<Propstat> for Value {
    fn from(
        Propstat {
            prop,
            status,
            responsedescription,
        }: Propstat,
    ) -> Value {
        let mut map = ValueMap::new();

        map.insert::<Properties>(prop.into());
        map.insert::<Status>(status.into());
        if let Some(responsedescription) = responsedescription {
            map.insert::<ResponseDescription>(responsedescription.into())
        }

        Value::Map(map)
    }
}
