//! A minimal blocking WebDAV client.
//!
//! The client owns a [`reqwest::blocking::Client`] and the HTTP Basic
//! credentials, and exposes exactly the WebDAV verbs [`crate::WebDAVFs`] needs.

use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::{Method, header};

/// Body sent with every `PROPFIND` request; asks the server for every property
/// it knows about the resource.
const PROPFIND_ALLPROP: &str = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
    <D:allprop/>
</D:propfind>
"#;

/// A blocking WebDAV client authenticating with HTTP Basic auth.
pub struct DavClient {
    client: Client,
    username: String,
    password: String,
}

impl DavClient {
    /// Create a client which authenticates as `username` with `password`.
    pub fn new(username: &str, password: &str) -> Self {
        Self {
            client: Client::new(),
            username: username.to_string(),
            password: password.to_string(),
        }
    }

    /// `GET` the resource at `url`.
    ///
    /// # Errors
    ///
    /// Fails if `url` is not a valid URL or if the request cannot be sent.
    pub fn get(&self, url: &str) -> reqwest::Result<Response> {
        self.request(Method::GET, url).send()
    }

    /// `PUT` `body` to `url`, creating or replacing the resource.
    ///
    /// # Errors
    ///
    /// Fails if `url` is not a valid URL or if the request cannot be sent.
    pub fn put(&self, url: &str, body: Vec<u8>) -> reqwest::Result<Response> {
        self.request(Method::PUT, url)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
    }

    /// `DELETE` the resource at `url`.
    ///
    /// # Errors
    ///
    /// Fails if `url` is not a valid URL or if the request cannot be sent.
    pub fn delete(&self, url: &str) -> reqwest::Result<Response> {
        self.request(Method::DELETE, url).send()
    }

    /// `MKCOL`: create a collection at `url`.
    ///
    /// # Errors
    ///
    /// Fails if `url` is not a valid URL or if the request cannot be sent.
    pub fn mkcol(&self, url: &str) -> reqwest::Result<Response> {
        self.request(dav_method("MKCOL"), url).send()
    }

    /// `MOVE` the resource at `from` to `to`.
    ///
    /// # Errors
    ///
    /// Fails if either URL is invalid, if `to` is not a valid header value, or
    /// if the request cannot be sent.
    pub fn mv(&self, from: &str, to: &str) -> reqwest::Result<Response> {
        self.request(dav_method("MOVE"), from)
            .header("destination", to)
            .send()
    }

    /// `PROPFIND` every property of the resource at `url`.
    ///
    /// `depth` is the value of the `Depth` header: `0` covers the resource
    /// itself, `1` the resource and its children, and `infinity` the whole
    /// subtree.
    ///
    /// # Errors
    ///
    /// Fails if `url` is not a valid URL or if the request cannot be sent.
    pub fn list(&self, url: &str, depth: &str) -> reqwest::Result<Response> {
        self.request(dav_method("PROPFIND"), url)
            .header("depth", depth)
            .body(PROPFIND_ALLPROP)
            .send()
    }

    /// Start an authenticated request for `method` and `url`.
    ///
    /// An invalid `url` is reported by [`RequestBuilder::send`] rather than
    /// here, so that every method has a single error path.
    fn request(&self, method: Method, url: &str) -> RequestBuilder {
        self.client
            .request(method, url)
            .basic_auth(&self.username, Some(&self.password))
    }
}

/// Build one of the WebDAV extension methods.
///
/// # Panics
///
/// Panics if `name` is not a valid HTTP method token. Only called with the
/// constants defined in this module.
fn dav_method(name: &str) -> Method {
    Method::from_bytes(name.as_bytes()).expect("WebDAV method name is a valid HTTP token")
}
