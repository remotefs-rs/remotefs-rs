// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::borrow::Cow;

use bytestring::ByteString;
use quick_xml::escape::{resolve_predefined_entity, unescape};

use super::element::ElementName;
use super::utils::BytesExt;
use super::value::ValueMap;
use super::{Error, Result, Value};

pub(crate) fn read_xml(xml: impl Into<bytes::Bytes>) -> Result<Value> {
    let xml = xml.into();
    let mut reader = XmlReader::new(std::str::from_utf8(&xml)?);
    reader.read_into_value(&xml)
}
struct XmlReader<'x> {
    reader: quick_xml::NsReader<&'x [u8]>,
    last: Option<quick_xml::events::Event<'x>>,
}

impl<'x> XmlReader<'x> {
    fn new(xml: &'x str) -> Self {
        Self {
            reader: quick_xml::NsReader::from_str(xml),
            last: None,
        }
    }
    fn last(&self) -> Option<&quick_xml::events::Event<'x>> {
        self.last.as_ref()
    }
    fn read_resolved_event(
        &mut self,
    ) -> quick_xml::Result<(
        quick_xml::name::ResolveResult<'_>,
        quick_xml::events::Event<'_>,
    )> {
        let (resolve_result, event) = self.reader.read_resolved_event()?;
        self.last = Some(event.clone());
        Ok((resolve_result, event))
    }

    fn read_into_value(&mut self, xml: &bytes::Bytes) -> Result<Value> {
        use quick_xml::events::{BytesStart, Event};
        use quick_xml::name::ResolveResult;

        fn key(
            xml: &bytes::Bytes,
            resolve_result: &ResolveResult,
            tag: &BytesStart<'_>,
        ) -> Result<ElementName<ByteString>> {
            match resolve_result {
                ResolveResult::Bound(ns) => {
                    if ns.as_ref().is_empty() {
                        return Err(Error::InvalidNamespace(
                            xml.maybe_slice_ref(ns.as_ref().as_bytes()),
                        ));
                    }

                    Ok(ElementName {
                        namespace: Some(xml.maybe_slice_ref(ns.as_ref().as_bytes()).try_into()?),
                        prefix: None,
                        local_name: xml
                            .maybe_slice_ref(tag.local_name().as_ref().as_bytes())
                            .try_into()?,
                    })
                }
                ResolveResult::Unbound | ResolveResult::Unknown(_) => Ok(ElementName {
                    namespace: None,
                    prefix: None,
                    local_name: xml
                        .maybe_slice_ref(tag.name().as_ref().as_bytes())
                        .try_into()?,
                }),
            }
        }

        let mut map = ValueMap::new();

        loop {
            let (resolve_result, event) = self.read_resolved_event()?;
            match event {
                Event::Text(text) if text.chars().all(char::is_whitespace) => {
                    continue;
                }
                Event::Text(text) => {
                    let head = unescape(&text).map_err(quick_xml::Error::from)?;
                    let head: ByteString = xml.maybe_slice_ref(head.as_bytes()).try_into()?;
                    drop(text);

                    return Ok(Value::Text(self.read_trailing_text(head)?));
                }
                Event::Start(start) => {
                    let key = key(xml, &resolve_result, &start)?;
                    let start_name = xml.maybe_slice_ref(start.name().as_ref().as_bytes());
                    drop(resolve_result);
                    drop(start);

                    map.insert_raw(key, self.read_into_value(xml)?);

                    if !matches!(self.last(), Some(Event::End(end)) if start_name == end.name().as_ref().as_bytes())
                    {
                        return Err(Error::UnexpectedTag);
                    }
                }
                Event::Empty(tag) => {
                    map.insert_raw(key(xml, &resolve_result, &tag)?, Value::Empty);
                }
                Event::End(_) | Event::Eof => break,
                Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => continue,
                Event::CData(_) | Event::GeneralRef(_) => return Err(Error::UnexpectedTag),
            }
        }

        Ok(Value::Map(map))
    }

    /// Consumes the remaining character data of the current element and appends
    /// it to `head`.
    ///
    /// `quick-xml` splits character data at every entity or character reference
    /// and at every CDATA section, so a single text node can arrive as several
    /// events. The common case is a single [`Event::Text`], which is returned
    /// without copying.
    fn read_trailing_text(&mut self, head: ByteString) -> Result<ByteString> {
        let mut tail: Option<String> = None;

        loop {
            let (_, event) = self.read_resolved_event()?;
            let chunk: Cow<'_, str> = match event {
                quick_xml::events::Event::Text(ref text) => {
                    unescape(text).map_err(quick_xml::Error::from)?
                }
                quick_xml::events::Event::CData(ref cdata) => Cow::Borrowed(cdata),
                quick_xml::events::Event::GeneralRef(ref reference) => {
                    match reference.resolve_char_ref()? {
                        Some(c) => Cow::Owned(c.to_string()),
                        None => Cow::Borrowed(
                            resolve_predefined_entity(reference)
                                .ok_or_else(|| Error::UnknownEntity(reference.to_string()))?,
                        ),
                    }
                }
                quick_xml::events::Event::Comment(_) | quick_xml::events::Event::PI(_) => continue,
                quick_xml::events::Event::End(_) | quick_xml::events::Event::Eof => break,
                _ => return Err(Error::UnexpectedTag),
            };
            tail.get_or_insert_with(String::new).push_str(&chunk);
        }

        Ok(match tail {
            None => head,
            Some(tail) => ByteString::from(format!("{head}{tail}")),
        })
    }
}
