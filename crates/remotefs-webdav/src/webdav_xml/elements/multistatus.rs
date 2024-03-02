// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use nonempty::NonEmpty;

use super::super::elements::response::Response;
use super::super::elements::ResponseDescription;
use super::super::value::ValueMap;
use super::super::{Element, Error, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `multistatus` XML element as defined in [RFC 4918](http://webdav.org/specs/rfc4918.html#ELEMENT_multistatus).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Multistatus {
    pub response: Vec<Response>,
    pub responsedescription: Option<ResponseDescription>,
}

impl Element for Multistatus {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "multistatus";
}

impl TryFrom<&Value> for Multistatus {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        let map = value.to_map()?;

        fn iter_response_items(
            mut acc: Vec<Response>,
            value: &Value,
        ) -> Result<Vec<Response>, Error> {
            if value.is_list() {
                for item in value.to_list()? {
                    if item.is_list() {
                        acc = iter_response_items(acc, item)?;
                    } else {
                        acc.push(Response::try_from(item)?);
                    }
                }
            } else if value.is_map() {
                acc.push(Response::try_from(value)?);
            }

            Ok(acc)
        }

        let mut response = Vec::new();
        for (_, value) in &map.0 {
            response.extend(iter_response_items(vec![], value)?);
        }

        Ok(Multistatus {
            response,
            responsedescription: map.get().transpose()?,
        })
    }
}

impl From<Multistatus> for Value {
    fn from(
        Multistatus {
            response,
            responsedescription,
        }: Multistatus,
    ) -> Value {
        let mut map = ValueMap::new();

        map.insert::<Response>(
            match NonEmpty::collect(response.into_iter().map(Value::from)) {
                Some(responses) => Value::List(Box::new(responses)),
                None => Value::Empty,
            },
        );
        if let Some(responsedescription) = responsedescription {
            map.insert::<ResponseDescription>(responsedescription.into())
        }

        Value::Map(map)
    }
}
