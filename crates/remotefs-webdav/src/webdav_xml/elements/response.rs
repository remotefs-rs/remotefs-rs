// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use nonempty::NonEmpty;

use super::super::elements::{Href, Propstat, ResponseDescription, Status};
use super::super::utils::NonEmptyExt;
use super::super::value::ValueMap;
use super::super::{Element, Error, OptionExt, Value, DAV_NAMESPACE, DAV_PREFIX};

/// The `response` XML element as defined in [RFC 4918](http://webdav.org/specs/rfc4918.html#ELEMENT_response).
#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    Propstat {
        href: Href,
        propstat: NonEmpty<Propstat>,
        // error: Option<Error>,
        responsedescription: Option<ResponseDescription>,
        // location: Option<Location>,
    },
    Status {
        href: NonEmpty<Href>,
        status: Status,
        // error: Option<Error>,
        responsedescription: Option<ResponseDescription>,
        // location: Option<Location>,
    },
}

impl Element for Response {
    const NAMESPACE: &'static str = DAV_NAMESPACE;
    const PREFIX: &'static str = DAV_PREFIX;
    const LOCAL_NAME: &'static str = "response";
}

impl TryFrom<&Value> for Response {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        let map = value.to_map()?;

        match NonEmpty::try_collect(map.iter_all::<Propstat>())? {
            Some(propstat) => Ok(Self::Propstat {
                href: map.get().required::<Href>()??,
                propstat,
                responsedescription: map.get().transpose()?,
            }),
            None => Ok(Self::Status {
                href: NonEmpty::try_collect(map.iter_all())?.required::<Href>()?,
                status: map.get().required::<Status>()??,
                responsedescription: map.get().transpose()?,
            }),
        }
    }
}

impl From<Response> for Value {
    fn from(response: Response) -> Value {
        let mut map = ValueMap::new();

        match response {
            Response::Propstat {
                href,
                propstat,
                responsedescription,
            } => {
                map.insert::<Href>(href.into());
                map.insert::<Propstat>(Value::List(Box::new(
                    NonEmpty::collect(propstat.into_iter().map(Value::from)).expect(
                        "iterator is created from a `NonEmpty` and is guaranteed to be nonempty",
                    ),
                )));
                if let Some(responsedescription) = responsedescription {
                    map.insert::<ResponseDescription>(responsedescription.into())
                }
            }
            Response::Status {
                href,
                status,
                responsedescription,
            } => {
                map.insert::<Href>(Value::List(Box::new(
                    NonEmpty::collect(href.into_iter().map(Value::from)).expect(
                        "iterator is created from a `NonEmpty` and is guaranteed to be nonempty",
                    ),
                )));
                map.insert::<Status>(status.into());
                if let Some(responsedescription) = responsedescription {
                    map.insert::<ResponseDescription>(responsedescription.into())
                }
            }
        }

        Value::Map(map)
    }
}
