// SPDX-FileCopyrightText: d-k-bo <d-k-bo@mailbox.org>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use bytes::Bytes;

/// Alias for the result type that is used in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Error type to wrap all errors that might occur when (de)serializing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("unexpected value type: {0}")]
    InvalidValueType(&'static str),
    #[error("missing `{0}` element")]
    MissingElement(&'static str),
    #[error("conflicting elements: {0}")]
    ConflictingElements(&'static str),
    #[error("invalid namespace declaration: {0:?}")]
    InvalidNamespace(Bytes),
    #[error(transparent)]
    Xml(#[from] quick_xml::Error),
    #[error("unexpected tag")]
    UnexpectedTag,
    #[error(transparent)]
    Utf8(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

impl Error {
    pub fn other(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self::Other(e.into())
    }
}
