// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use bytestring::ByteString;
use indexmap::IndexMap;
use nonempty::{nonempty, NonEmpty};

use super::element::{Element, ElementExt, ElementName};
use super::Error;

/// Represents the content of an XML element.
///
/// This data structure is intended to be similar to
/// [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html).
/// Unlike when deserializing JSON, which has explicit arrays, we have to
/// manually group multiple adjacent elements into an array-like structure.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    /// The element is empty, e.g. `<foo />`
    #[default]
    Empty,
    /// The element contains a text node, e.g. `<foo>bar</foo>`
    Text(ByteString),
    /// The element contains other elements, e.g. `<foo><bar /></foo>`
    Map(ValueMap),
    /// The parent element contains multiple elements of this type, e.g. `<foo
    /// /><foo />`
    List(Box<NonEmpty<Value>>),
}

impl Value {
    pub fn to_str(&self) -> Result<&ByteString, Error> {
        match self {
            Self::Text(s) => Ok(s),
            _ => Err(Error::InvalidValueType("expected text")),
        }
    }
    pub fn to_map(&self) -> Result<&ValueMap, Error> {
        match self {
            Self::Map(map) => Ok(map),
            _ => Err(Error::InvalidValueType("expected a map")),
        }
    }

    pub fn to_list(&self) -> Result<&NonEmpty<Value>, Error> {
        match self {
            Self::List(list) => Ok(list),
            _ => Err(Error::InvalidValueType("expected a list")),
        }
    }

    pub fn is_list(&self) -> bool {
        matches!(self, Self::List(_))
    }

    pub fn is_map(&self) -> bool {
        matches!(self, Self::Map(_))
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Text(s.into())
    }
}

type InnerValueMap = IndexMap<ElementName<ByteString>, Value>;

/// A mapping from tag names to [`Value`]s.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ValueMap(pub(crate) InnerValueMap);

impl ValueMap {
    pub fn new() -> Self {
        Self(IndexMap::new())
    }
    /// Extract a child element of a specific type.
    ///
    /// # Returns
    ///
    /// - `None` if the element doesn't exist
    /// - `Some(Ok(_))` if the element exists and was successfully extracted
    /// - `Some(Err(_))` if the element exists and extraction failed
    pub fn get<'v, E>(&'v self) -> Option<Result<E, Error>>
    where
        E: Element + TryFrom<&'v Value, Error = Error>,
    {
        self.0
            .get(&E::element_name::<&'static str>())
            .map(E::try_from)
    }
    /// Extract a non-empty child element of a specific type.
    ///
    /// # Returns
    ///
    /// - `None` if the element doesn't exist
    /// - `Some(None)` if the element exists and is empty
    /// - `Some(Some(Ok(_)))` if the element exists, is not empty and was
    ///   successfully extracted
    /// - `Some(Some(Err(_)))` if the element exists, is not empty and
    ///   extraction failed
    pub fn get_optional<'v, E>(&'v self) -> Option<Option<Result<E, Error>>>
    where
        E: Element + TryFrom<&'v Value, Error = Error>,
    {
        self.0
            .get(&E::element_name::<&'static str>())
            .map(|value| match value {
                Value::Empty => None,
                v => Some(v.try_into()),
            })
    }
    /// Insert a child value into the map.
    pub fn insert<E: Element>(&mut self, value: Value) {
        let key = E::element_name();
        self.insert_raw(key, value)
    }
}

impl ValueMap {
    pub(crate) fn iter_all<'v, E>(&'v self) -> impl Iterator<Item = Result<E, Error>> + 'v
    where
        E: Element + TryFrom<&'v Value, Error = Error> + 'v,
    {
        enum ElementIter<'a> {
            List(nonempty::Iter<'a, Value>),
            Single(std::iter::Once<&'a Value>),
            Empty,
        }

        impl<'a> Iterator for ElementIter<'a> {
            type Item = &'a Value;

            fn next(&mut self) -> Option<Self::Item> {
                match self {
                    Self::List(inner) => inner.next(),
                    Self::Single(value) => value.next(),
                    Self::Empty => None,
                }
            }
        }

        match self.0.get(&E::element_name::<&'static str>()) {
            Some(Value::List(list)) => ElementIter::List(list.iter()),
            Some(value) => ElementIter::Single(std::iter::once(value)),
            None => ElementIter::Empty,
        }
        .map(E::try_from)
    }
    // pub(crate) fn iter_all_nonempty<'v, E>(&'v self) -> impl Iterator<Item =
    // Result<E, Error>> + 'v where
    //     E: Element + TryFrom<&'v Value, Error = Error>,
    // {
    //     self.iter_all()
    //         .map(|v| v.ok_or(Error::EmptyElement(E::LOCAL_NAME))?)
    // }
    // pub(crate) fn get_all_nonempty<'v, E>(&'v self) ->
    // Result<Option<NonEmpty<E>>, Error> where
    //     E: Element + TryFrom<&'v Value, Error = Error>,
    // {
    //     NonEmpty::try_collect(
    //         self.iter_all()
    //             .map(|v| v.ok_or(Error::EmptyElement(E::LOCAL_NAME))?),
    //     )
    // }
    // pub(crate) fn get_all_required<'v, E>(&'v self) -> Result<NonEmpty<E>, Error>
    // where
    //     E: Element + TryFrom<&'v Value, Error = Error>,
    // {
    //     NonEmpty::try_collect(
    //         self.iter_all()
    //             .map(|v| v.ok_or(Error::EmptyElement(E::LOCAL_NAME))?),
    //     )
    //     .transpose()
    //     .ok_or(Error::MissingElement(E::LOCAL_NAME))?
    // }
    pub(crate) fn insert_raw(&mut self, key: ElementName<ByteString>, value: Value) {
        match self.0.get_mut(&key) {
            Some(old_value) => {
                *old_value = Value::List(Box::new(nonempty![std::mem::take(old_value), value]));
            }
            None => {
                self.0.insert(key, value);
            }
        }
    }
}

impl AsRef<InnerValueMap> for ValueMap {
    fn as_ref(&self) -> &InnerValueMap {
        &self.0
    }
}

impl AsMut<InnerValueMap> for ValueMap {
    fn as_mut(&mut self) -> &mut InnerValueMap {
        &mut self.0
    }
}

impl From<InnerValueMap> for ValueMap {
    fn from(map: InnerValueMap) -> Self {
        Self(map)
    }
}
