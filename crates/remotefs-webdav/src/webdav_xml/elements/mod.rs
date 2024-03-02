// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! XML element definitions based on
//! [RFC 4918](http://webdav.org/specs/rfc4918.html#xml.element.definitions).

mod href;
mod multistatus;
mod prop;
mod propfind;
mod propstat;
mod response;
mod responsedescription;
mod status;

pub use self::href::Href;
pub use self::multistatus::Multistatus;
pub use self::prop::Properties;
pub use self::propstat::Propstat;
pub use self::response::Response;
pub use self::responsedescription::ResponseDescription;
pub use self::status::Status;
