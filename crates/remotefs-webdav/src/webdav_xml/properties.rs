// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! XML property definitions based on
//! [RFC 4918](http://webdav.org/specs/rfc4918.html#dav.properties).

mod creationdate;
mod displayname;
mod getcontentlanguage;
mod getcontentlength;
mod getcontenttype;
mod getetag;
mod getlastmodified;
mod lockdiscovery;
mod resourcetype;
mod supportedlock;

pub use self::creationdate::CreationDate;
pub use self::getcontentlength::ContentLength;
pub use self::getlastmodified::LastModified;
