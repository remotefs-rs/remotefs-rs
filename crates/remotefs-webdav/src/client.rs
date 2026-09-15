//! The WebDAV implementation of [`AsyncRemoteFs`].

use std::path::Path;

use dav_xml_client::dav_xml::elements::Depth;
use dav_xml_client::headers::Overwrite;
use dav_xml_client::transport::AsyncTransport;
use dav_xml_client::transport::reqwest::Client;
use dav_xml_client::{AsyncDavClient, Auth, Error, Resource};
use http::{HeaderValue, StatusCode, header};
use remotefs::fs::{
    AsyncReadStream, AsyncRemoteFs, AsyncWriteStream, Capabilities, ExecOutput, ReadOptions,
    SetMetadata, UnixPex, WriteOptions,
};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};

use crate::error::map_error;
use crate::resource::resource_to_file_at;
use crate::stream::{WebDavReader, WebDavWriter};
use crate::url::resource_url;

/// An [`AsyncRemoteFs`] client speaking WebDAV.
///
/// The client keeps the base URL of the share and an `AsyncDavClient` built
/// over the transport `T`, which defaults to the `reqwest` client. Every path
/// is absolute and resolved against the base URL. WebDAV has no streaming
/// bodies, so reads download the whole (optionally ranged) body before the
/// stream is returned and writes buffer until `finish` issues one `PUT`.
///
/// `append`, `set_metadata`, `symlink`, and `exec` always fail with
/// [`RemoteErrorType::UnsupportedFeature`].
///
/// # Examples
///
/// ```rust,no_run
/// use std::path::Path;
///
/// use remotefs::AsyncRemoteFs;
/// use remotefs_webdav::{Auth, WebDAVFs};
///
/// # async fn run() -> remotefs::RemoteResult<()> {
/// let mut client = WebDAVFs::new("http://localhost:3080", Auth::basic("alice", "secret1234"))?;
/// client.connect().await?;
/// for entry in client.list_dir(Path::new("/")).await? {
///     println!("{name}", name = entry.name());
/// }
/// client.disconnect().await?;
/// # Ok(())
/// # }
/// ```
pub struct WebDAVFs<T = Client> {
    url: String,
    client: AsyncDavClient<T>,
    connected: bool,
}

impl<T> std::fmt::Debug for WebDAVFs<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebDAVFs")
            .field("url", &self.url)
            .field("connected", &self.connected)
            .finish_non_exhaustive()
    }
}

impl WebDAVFs<Client> {
    /// Creates a client for `url` over the default `reqwest` transport.
    ///
    /// `url` is the base URL of the WebDAV share; a trailing slash is
    /// ignored. `auth` is sent with every request. No request is sent until
    /// [`AsyncRemoteFs::connect`] is called.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::ConnectionError`] when the HTTP client
    /// cannot be built, for example when the platform TLS backend fails to
    /// initialise.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use remotefs_webdav::{Auth, WebDAVFs};
    ///
    /// let client = WebDAVFs::new("http://localhost:3080", Auth::basic("alice", "secret1234"))
    ///     .expect("http client");
    /// assert_eq!(client.url(), "http://localhost:3080");
    /// ```
    pub fn new(url: &str, auth: Auth) -> RemoteResult<Self> {
        let client = AsyncDavClient::reqwest(auth).map_err(map_error)?;
        Ok(Self::with_client(url, client))
    }
}

impl<T> WebDAVFs<T>
where
    T: AsyncTransport + Clone + Send + Sync + Unpin + 'static,
{
    /// Creates a client for `url` over a caller-supplied transport.
    ///
    /// Use this to configure the HTTP client or to plug in another
    /// [`AsyncTransport`] implementation.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use remotefs_webdav::dav_xml_client::transport::reqwest::default_client;
    /// use remotefs_webdav::{Auth, WebDAVFs};
    ///
    /// let transport = default_client().expect("http client");
    /// let client = WebDAVFs::with_transport("http://localhost:3080", transport, Auth::None);
    /// assert_eq!(client.url(), "http://localhost:3080");
    /// ```
    pub fn with_transport(url: &str, transport: T, auth: Auth) -> Self {
        Self::with_client(url, AsyncDavClient::new(transport, auth))
    }

    /// Returns the base URL of the share, without a trailing slash.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use remotefs_webdav::{Auth, WebDAVFs};
    ///
    /// let client = WebDAVFs::new("http://localhost:3080/", Auth::None).expect("http client");
    /// assert_eq!(client.url(), "http://localhost:3080");
    /// ```
    pub fn url(&self) -> &str {
        &self.url
    }

    fn with_client(url: &str, client: AsyncDavClient<T>) -> Self {
        Self {
            url: url.trim_end_matches('/').to_string(),
            client,
            connected: false,
        }
    }

    fn require_connected(&self) -> RemoteResult<()> {
        if self.connected {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// Resolves `path` and checks the connection.
    ///
    /// The path is validated first so a relative path is always reported as
    /// `InvalidPath`, even on a disconnected client.
    fn url_for(&self, path: &Path, collection: bool) -> RemoteResult<String> {
        let url = resource_url(&self.url, path, collection)?;
        self.require_connected()?;
        Ok(url)
    }

    /// Runs a depth-0 `PROPFIND` on `path`.
    ///
    /// Servers answer a redirect when a collection is requested without its
    /// trailing slash; the request is then retried in collection form.
    async fn stat_url(&self, path: &Path) -> RemoteResult<File> {
        let url = self.url_for(path, false)?;
        debug!("stat {url}");
        let resource = match self.client.stat(&url).await {
            Ok(resource) => resource,
            Err(Error::Status { status, .. }) if status.is_redirection() => {
                let url = self.url_for(path, true)?;
                debug!("stat redirected, retrying as collection {url}");
                self.client.stat(&url).await.map_err(map_error)?
            }
            Err(error) => return Err(map_error(error)),
        };
        resource
            .map(|resource| resource_to_file_at(&resource, path))
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))
    }

    fn unsupported() -> RemoteError {
        RemoteError::new(RemoteErrorType::UnsupportedFeature)
    }
}

fn is_self_resource(resource: &Resource, request_url: &str, requested_path: &Path) -> bool {
    if !resource.is_collection {
        return false;
    }
    let requested_path = requested_path.to_string_lossy();
    let requested_path = canonical_path(&requested_path);
    let href_path = resource.path();
    if canonical_path(href_path) == requested_path {
        return true;
    }
    let Ok(request_uri) = request_url.parse::<http::Uri>() else {
        return false;
    };
    let request_uri_path = canonical_path(request_uri.path());
    if canonical_path(href_path) == request_uri_path {
        return true;
    }
    let resolved = if href_path.is_empty() || matches!(href_path, "." | "./") {
        request_uri.path().to_owned()
    } else if href_path.starts_with('/') {
        href_path.to_owned()
    } else {
        format!("{}{href_path}", request_uri.path())
    };
    canonical_path(&resolved) == request_uri_path
}

fn canonical_path(path: &str) -> &str {
    let path = path.trim_end_matches('/');
    if path.is_empty() { "/" } else { path }
}

/// Builds the `Range` header for `offset` and `length`, or `None` when the
/// whole body is requested.
///
/// A length that would overflow the end position falls back to an open
/// range; the body is then trimmed locally by [`slice_body`].
fn range_header(offset: u64, length: Option<u64>) -> dav_xml_client::Result<Option<HeaderValue>> {
    let end = length
        .and_then(|length| offset.checked_add(length))
        .and_then(|end| end.checked_sub(1));
    let value = match (offset, length, end) {
        (0, None, _) => return Ok(None),
        (_, Some(_), Some(end)) if end >= offset => format!("bytes={offset}-{end}"),
        _ => format!("bytes={offset}-"),
    };
    Ok(Some(HeaderValue::from_str(&value)?))
}

/// Applies `offset` and `length` to a body the server returned in full.
fn slice_body(mut body: Vec<u8>, offset: u64, length: Option<u64>) -> Vec<u8> {
    let start = usize::try_from(offset).map_or(body.len(), |offset| offset.min(body.len()));
    body.drain(..start);
    if let Some(length) = length {
        let end = usize::try_from(length).map_or(body.len(), |length| length.min(body.len()));
        body.truncate(end);
    }
    body
}

impl<T> WebDAVFs<T>
where
    T: AsyncTransport + Clone + Send + Sync + Unpin + 'static,
{
    /// Sends a `GET`, adding a `Range` header when a partial body is wanted.
    async fn ranged_get(
        &self,
        url: &str,
        offset: u64,
        length: Option<u64>,
    ) -> dav_xml_client::Result<http::Response<Vec<u8>>> {
        match range_header(offset, length)? {
            None => self.client.get(url).await,
            Some(range) => {
                self.client
                    .clone()
                    .with_header(header::RANGE, range)
                    .get(url)
                    .await
            }
        }
    }

    fn empty_stream() -> AsyncReadStream {
        AsyncReadStream::new(WebDavReader::new(Vec::new()))
    }
}

/// A blocking view of [`WebDAVFs`] that implements [`remotefs::RemoteFs`].
///
/// Every call blocks on the supplied Tokio handle and must not be made from
/// inside an async context. Build one with [`WebDAVFs::into_blocking`].
#[cfg(feature = "tokio")]
pub type BlockingWebDAVFs<T = Client> = remotefs::adapters::blocking::BlockOn<WebDAVFs<T>>;

#[cfg(feature = "tokio")]
impl<T> WebDAVFs<T>
where
    T: AsyncTransport + Clone + Send + Sync + Unpin + 'static,
{
    /// Wraps the client for blocking callers using the given runtime handle.
    ///
    /// The returned value implements [`remotefs::RemoteFs`] and can be stored
    /// as `Box<dyn RemoteFs>`. Calling it from an async context panics, as
    /// documented by [`tokio::runtime::Handle::block_on`].
    ///
    /// # Panics
    ///
    /// Panics if `handle` belongs to a current-thread runtime, which cannot
    /// drive the blocked operation from a non-async caller.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use remotefs::RemoteFs;
    /// use remotefs_webdav::{Auth, WebDAVFs};
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let runtime = tokio::runtime::Runtime::new()?;
    /// let mut client: Box<dyn RemoteFs> = Box::new(
    ///     WebDAVFs::new("http://localhost:3080", Auth::basic("alice", "secret1234"))?
    ///         .into_blocking(runtime.handle().clone()),
    /// );
    /// client.connect()?;
    /// client.disconnect()?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn into_blocking(self, handle: tokio::runtime::Handle) -> BlockingWebDAVFs<T> {
        assert_ne!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread,
            "into_blocking requires a multi-thread Tokio runtime"
        );
        remotefs::adapters::blocking::BlockOn::new(self, handle)
    }
}

#[remotefs::async_trait]
impl<T> AsyncRemoteFs for WebDAVFs<T>
where
    T: AsyncTransport + Clone + Send + Sync + Unpin + 'static,
{
    async fn connect(&mut self) -> RemoteResult<()> {
        if self.connected {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        let url = format!("{base}/", base = self.url);
        debug!("connecting to {url}");
        self.client
            .stat(&url)
            .await
            .map_err(map_error)?
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> RemoteResult<()> {
        self.require_connected()?;
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAM_READ
            | Capabilities::STREAM_WRITE
            | Capabilities::RANGE_READ
            | Capabilities::SEEK_READ
            | Capabilities::COPY
    }

    async fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        let url = self.url_for(path, true)?;
        debug!("listing {url}");
        let resources = self.client.list(&url).await.map_err(map_error)?;
        Ok(resources
            .iter()
            .filter(|resource| !is_self_resource(resource, &url, path))
            .map(|resource| resource_to_file_at(resource, &path.join(resource.name())))
            .collect())
    }

    async fn stat(&self, path: &Path) -> RemoteResult<File> {
        self.stat_url(path).await
    }

    async fn exists(&self, path: &Path) -> RemoteResult<bool> {
        match self.stat_url(path).await {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn set_metadata(&self, path: &Path, _metadata: &SetMetadata) -> RemoteResult<()> {
        self.url_for(path, false)?;
        Err(Self::unsupported())
    }

    async fn create_dir(&self, path: &Path, _mode: Option<UnixPex>) -> RemoteResult<()> {
        let url = self.url_for(path, true)?;
        if self.exists(path).await? {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        debug!("creating collection {url}");
        self.client.mkcol(&url).await.map_err(map_error)
    }

    async fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        let url = self.url_for(path, false)?;
        debug!("removing file {url}");
        self.client.delete(&url).await.map_err(map_error)
    }

    async fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        let url = self.url_for(path, true)?;
        if !self.list_dir(path).await?.is_empty() {
            return Err(RemoteError::new(RemoteErrorType::DirectoryNotEmpty));
        }
        debug!("removing collection {url}");
        self.client.delete(&url).await.map_err(map_error)
    }

    /// Removes an entry with a single `DELETE`; WebDAV deletes collections
    /// recursively (RFC 4918 section 9.6.1).
    async fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        let entry = self.stat_url(path).await?;
        let url = self.url_for(path, entry.is_dir())?;
        debug!("removing {url} recursively");
        self.client.delete(&url).await.map_err(map_error)
    }

    async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let entry = self.stat_url(src).await?;
        let src_url = self.url_for(src, entry.is_dir())?;
        let dest_url = self.url_for(dest, entry.is_dir())?;
        debug!("moving {src_url} to {dest_url}");
        self.client
            .mv(&src_url, &dest_url, Overwrite::True)
            .await
            .map_err(map_error)
    }

    async fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let entry = self.stat_url(src).await?;
        let src_url = self.url_for(src, entry.is_dir())?;
        let dest_url = self.url_for(dest, entry.is_dir())?;
        debug!("copying {src_url} to {dest_url}");
        self.client
            .copy(&src_url, &dest_url, Depth::Infinity, Overwrite::True)
            .await
            .map_err(map_error)
    }

    async fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        self.url_for(path, false)?;
        self.url_for(target, false)?;
        Err(Self::unsupported())
    }

    /// Downloads the requested range and returns it as an owned stream.
    ///
    /// `length == Some(0)` only verifies that the entry exists. A server that
    /// ignores the `Range` header answers `200` with the whole body, which is
    /// then trimmed locally; `416` beyond the end of the file yields an empty
    /// stream.
    async fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<AsyncReadStream> {
        let url = self.url_for(path, false)?;
        if opts.length == Some(0) {
            self.stat_url(path).await?;
            return Ok(Self::empty_stream());
        }
        let offset = opts.offset.unwrap_or(0);
        debug!(
            "opening {url} at offset {offset}, length {length:?}",
            length = opts.length
        );
        let response = match self.ranged_get(&url, offset, opts.length).await {
            Ok(response) => response,
            Err(Error::Status {
                status: StatusCode::RANGE_NOT_SATISFIABLE,
                ..
            }) => return Ok(Self::empty_stream()),
            Err(error) => return Err(map_error(error)),
        };
        let body = if response.status() == StatusCode::PARTIAL_CONTENT {
            response.into_body()
        } else {
            slice_body(response.into_body(), offset, opts.length)
        };
        Ok(AsyncReadStream::new(WebDavReader::new(body)))
    }

    /// Returns a buffering stream; the file is created when `finish` runs.
    ///
    /// A missing parent collection is reported by `finish`, not here.
    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let url = self.url_for(path, false)?;
        debug!("creating {url}");
        Ok(AsyncWriteStream::new(WebDavWriter::new(
            self.client.clone(),
            url,
            opts.size_hint,
        )))
    }

    async fn append(&self, path: &Path, _opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        self.url_for(path, false)?;
        Err(Self::unsupported())
    }

    async fn exec(&self, _cmd: &str) -> RemoteResult<ExecOutput> {
        Err(Self::unsupported())
    }
}

#[cfg(all(test, feature = "with-containers"))]
mod container_tests;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use dav_xml_client::transport::MockTransport;
    use futures::io::{AsyncReadExt as _, Cursor};
    use pretty_assertions::assert_eq;
    use remotefs::RemoteErrorType;
    use remotefs::fs::{Capabilities, ReadOptions, SetMetadata, WriteOptions};

    use super::*;

    pub(super) const LISTING: &str = r#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/d/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response><D:response><D:href>/d/f</D:href><D:propstat><D:prop><D:getcontentlength>1</D:getcontentlength></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#;
    pub(super) const PREFIXED_LISTING: &str = r#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/dav/files/user/d/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response><D:response><D:href>/dav/files/user/d/hello%20world</D:href><D:propstat><D:prop><D:getcontentlength>1</D:getcontentlength></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#;
    pub(super) const EMPTY_LISTING: &str = r#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/d/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#;
    pub(super) const FILE_STAT: &str = r#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/d/f</D:href><D:propstat><D:prop><D:getcontentlength>6</D:getcontentlength></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#;

    pub(super) fn fs() -> (WebDAVFs<MockTransport>, MockTransport) {
        crate::mock::logger();
        let mock = MockTransport::new();
        (
            WebDAVFs::with_transport("http://h/", mock.clone(), Auth::basic("a", "b")),
            mock,
        )
    }

    pub(super) fn reply(mock: &MockTransport, status: u16, body: &str) {
        let mut builder = http::Response::builder().status(status);
        if !body.is_empty() {
            builder = builder.header("content-type", "application/xml");
        }
        mock.reply(builder.body(body.as_bytes().to_vec()).unwrap());
    }

    pub(super) async fn connected() -> (WebDAVFs<MockTransport>, MockTransport) {
        let (mut fs, mock) = fs();
        reply(&mock, 207, EMPTY_LISTING);
        fs.connect().await.unwrap();
        (fs, mock)
    }

    #[test]
    fn new_trims_the_trailing_slash_and_starts_disconnected() {
        let (fs, _mock) = fs();
        assert_eq!(fs.url(), "http://h");
        assert!(!fs.is_connected());
        let reqwest = WebDAVFs::new("http://localhost:3080/", Auth::basic("a", "b")).unwrap();
        assert_eq!(reqwest.url(), "http://localhost:3080");
    }

    #[tokio::test]
    async fn relative_paths_are_invalid_before_the_connection_is_checked() {
        let (fs, _mock) = fs();
        assert_eq!(
            fs.stat(Path::new("a.txt")).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            fs.stat(Path::new("/a.txt")).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
    }

    #[tokio::test]
    async fn connect_probes_the_root_and_tracks_state() {
        let (mut fs, mock) = fs();
        reply(&mock, 207, EMPTY_LISTING);
        fs.connect().await.unwrap();
        let request = mock.last_request().unwrap();
        assert_eq!(request.method().as_str(), "PROPFIND");
        assert_eq!(request.uri(), "http://h/");
        assert_eq!(request.headers()["depth"], "0");
        assert!(fs.is_connected());
        assert_eq!(
            fs.connect().await.unwrap_err().kind(),
            RemoteErrorType::AlreadyConnected
        );
        fs.disconnect().await.unwrap();
        assert!(!fs.is_connected());
        assert_eq!(
            fs.disconnect().await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
    }

    #[tokio::test]
    async fn connect_reports_authentication_failures() {
        let (mut fs, mock) = fs();
        reply(&mock, 401, "");
        assert_eq!(
            fs.connect().await.unwrap_err().kind(),
            RemoteErrorType::AuthenticationFailed
        );
        assert!(!fs.is_connected());
    }

    #[tokio::test]
    async fn list_dir_skips_the_collection_itself() {
        let (fs, mock) = connected().await;
        reply(&mock, 207, LISTING);
        let entries = fs.list_dir(Path::new("/d")).await.unwrap();
        assert_eq!(mock.last_request().unwrap().uri(), "http://h/d/");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Path::new("/d/f"));
        assert!(entries[0].is_file());
        assert_eq!(entries[0].metadata().size, Some(1));
    }

    #[tokio::test]
    async fn list_dir_skips_the_collection_itself_with_a_trailing_slash() {
        let (fs, mock) = connected().await;
        reply(&mock, 207, LISTING);
        let entries = fs.list_dir(Path::new("/d/")).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Path::new("/d/f"));
    }

    #[tokio::test]
    async fn list_dir_returns_paths_in_the_client_namespace() {
        let mock = MockTransport::new();
        let mut fs = WebDAVFs::with_transport(
            "http://h/dav/files/user",
            mock.clone(),
            Auth::basic("a", "b"),
        );
        reply(&mock, 207, EMPTY_LISTING);
        fs.connect().await.unwrap();
        reply(&mock, 207, PREFIXED_LISTING);
        let entries = fs.list_dir(Path::new("/d")).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Path::new("/d/hello world"));
    }

    #[tokio::test]
    async fn stat_retries_a_redirected_collection_with_a_trailing_slash() {
        let (fs, mock) = connected().await;
        reply(&mock, 301, "");
        reply(&mock, 207, EMPTY_LISTING);
        let entry = fs.stat(Path::new("/d")).await.unwrap();
        assert!(entry.is_dir());
        assert_eq!(entry.path(), Path::new("/d"));
        let requests = mock.requests();
        assert_eq!(requests[1].uri(), "http://h/d");
        assert_eq!(requests[2].uri(), "http://h/d/");
    }

    #[tokio::test]
    async fn stat_and_exists_agree_on_missing_entries() {
        let (fs, mock) = connected().await;
        reply(&mock, 404, "");
        assert_eq!(
            fs.stat(Path::new("/missing")).await.unwrap_err().kind(),
            RemoteErrorType::NoSuchFileOrDirectory
        );
        reply(&mock, 404, "");
        assert!(!fs.exists(Path::new("/missing")).await.unwrap());
        reply(&mock, 207, FILE_STAT);
        assert!(fs.exists(Path::new("/d/f")).await.unwrap());
        reply(&mock, 500, "");
        assert_eq!(
            fs.exists(Path::new("/d/f")).await.unwrap_err().kind(),
            RemoteErrorType::ProtocolError
        );
    }

    #[test]
    fn client_is_send_sync_and_object_safe() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WebDAVFs>();
        assert_send_sync::<WebDAVFs<MockTransport>>();
        let (fs, _mock) = fs();
        let _: Box<dyn AsyncRemoteFs> = Box::new(fs);
    }

    #[test]
    fn capabilities_advertise_buffered_streams_ranges_and_copy() {
        let (fs, _mock) = fs();
        let capabilities = fs.capabilities();
        for expected in [
            Capabilities::STREAM_READ,
            Capabilities::STREAM_WRITE,
            Capabilities::RANGE_READ,
            Capabilities::SEEK_READ,
            Capabilities::COPY,
        ] {
            assert!(capabilities.contains(expected), "{expected:?}");
        }
        for unsupported in [
            Capabilities::APPEND,
            Capabilities::SEEK_WRITE,
            Capabilities::SYMLINK,
            Capabilities::SET_METADATA,
            Capabilities::POSIX_MODE,
            Capabilities::EXEC,
        ] {
            assert!(!capabilities.contains(unsupported), "{unsupported:?}");
        }
    }

    #[tokio::test]
    async fn create_dir_rejects_existing_collections_and_sends_mkcol() {
        let (fs, mock) = connected().await;
        reply(&mock, 207, EMPTY_LISTING);
        assert_eq!(
            fs.create_dir(Path::new("/d"), None)
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::AlreadyExists
        );
        reply(&mock, 404, "");
        reply(&mock, 201, "");
        fs.create_dir(Path::new("/new"), None).await.unwrap();
        let request = mock.last_request().unwrap();
        assert_eq!(request.method().as_str(), "MKCOL");
        assert_eq!(request.uri(), "http://h/new/");
    }

    #[tokio::test]
    async fn create_dir_without_parent_is_no_such_file() {
        let (fs, mock) = connected().await;
        reply(&mock, 404, "");
        reply(&mock, 409, "");
        assert_eq!(
            fs.create_dir(Path::new("/missing/new"), None)
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::NoSuchFileOrDirectory
        );
    }

    #[tokio::test]
    async fn remove_file_sends_delete() {
        let (fs, mock) = connected().await;
        reply(&mock, 204, "");
        fs.remove_file(Path::new("/d/f")).await.unwrap();
        let request = mock.last_request().unwrap();
        assert_eq!(request.method(), http::Method::DELETE);
        assert_eq!(request.uri(), "http://h/d/f");
    }

    #[tokio::test]
    async fn remove_dir_refuses_non_empty_collections() {
        let (fs, mock) = connected().await;
        reply(&mock, 207, LISTING);
        assert_eq!(
            fs.remove_dir(Path::new("/d")).await.unwrap_err().kind(),
            RemoteErrorType::DirectoryNotEmpty
        );
        reply(&mock, 207, EMPTY_LISTING);
        reply(&mock, 204, "");
        fs.remove_dir(Path::new("/d")).await.unwrap();
        let request = mock.last_request().unwrap();
        assert_eq!(request.method(), http::Method::DELETE);
        assert_eq!(request.uri(), "http://h/d/");
    }

    #[tokio::test]
    async fn remove_dir_all_deletes_the_collection_in_one_request() {
        let (fs, mock) = connected().await;
        reply(&mock, 207, EMPTY_LISTING);
        reply(&mock, 204, "");
        fs.remove_dir_all(Path::new("/d")).await.unwrap();
        let requests = mock.requests();
        assert_eq!(requests.len(), 3, "connect, stat, delete");
        assert_eq!(requests[2].method(), http::Method::DELETE);
        assert_eq!(requests[2].uri(), "http://h/d/");
        reply(&mock, 207, FILE_STAT);
        reply(&mock, 204, "");
        fs.remove_dir_all(Path::new("/d/f")).await.unwrap();
        assert_eq!(mock.last_request().unwrap().uri(), "http://h/d/f");
    }

    #[tokio::test]
    async fn rename_and_copy_forward_destination_and_overwrite() {
        let (fs, mock) = connected().await;
        reply(&mock, 207, FILE_STAT);
        reply(&mock, 201, "");
        fs.rename(Path::new("/d/f"), Path::new("/d/g"))
            .await
            .unwrap();
        let request = mock.last_request().unwrap();
        assert_eq!(request.method().as_str(), "MOVE");
        assert_eq!(request.uri(), "http://h/d/f");
        assert_eq!(request.headers()["destination"], "http://h/d/g");
        assert_eq!(request.headers()["overwrite"], "T");

        reply(&mock, 207, EMPTY_LISTING);
        reply(&mock, 201, "");
        fs.copy(Path::new("/d"), Path::new("/e")).await.unwrap();
        let request = mock.last_request().unwrap();
        assert_eq!(request.method().as_str(), "COPY");
        assert_eq!(request.uri(), "http://h/d/");
        assert_eq!(request.headers()["destination"], "http://h/e/");
        assert_eq!(request.headers()["depth"], "infinity");
    }

    #[tokio::test]
    async fn open_sends_a_range_and_trusts_partial_content() {
        let (fs, mock) = connected().await;
        reply(&mock, 206, "cd");
        let mut stream = fs
            .open(
                Path::new("/d/f"),
                &ReadOptions::default().offset(2).length(2),
            )
            .await
            .unwrap();
        assert_eq!(mock.last_request().unwrap().headers()["range"], "bytes=2-3");
        let mut output = Vec::new();
        stream.read_to_end(&mut output).await.unwrap();
        stream.finish().await.unwrap();
        assert_eq!(output, b"cd");
    }

    #[tokio::test]
    async fn open_slices_locally_when_the_server_ignores_the_range() {
        let (fs, mock) = connected().await;
        reply(&mock, 200, "abcdef");
        let mut output = Vec::new();
        fs.read_file(
            Path::new("/d/f"),
            &ReadOptions::default().offset(2).length(2),
            &mut output,
        )
        .await
        .unwrap();
        assert_eq!(output, b"cd");

        reply(&mock, 200, "abcdef");
        let mut output = Vec::new();
        fs.read_file(
            Path::new("/d/f"),
            &ReadOptions::default().offset(4),
            &mut output,
        )
        .await
        .unwrap();
        assert_eq!(mock.last_request().unwrap().headers()["range"], "bytes=4-");
        assert_eq!(output, b"ef");
    }

    #[tokio::test]
    async fn open_without_options_sends_no_range_header() {
        let (fs, mock) = connected().await;
        reply(&mock, 200, "abcdef");
        let mut output = Vec::new();
        let count = fs
            .read_file(Path::new("/d/f"), &ReadOptions::default(), &mut output)
            .await
            .unwrap();
        assert_eq!(count, 6);
        assert!(
            mock.last_request()
                .unwrap()
                .headers()
                .get("range")
                .is_none()
        );
    }

    #[tokio::test]
    async fn open_beyond_eof_and_zero_length_are_empty() {
        let (fs, mock) = connected().await;
        reply(&mock, 416, "");
        let mut output = Vec::new();
        fs.read_file(
            Path::new("/d/f"),
            &ReadOptions::default().offset(100),
            &mut output,
        )
        .await
        .unwrap();
        assert!(output.is_empty());

        reply(&mock, 207, FILE_STAT);
        let mut output = Vec::new();
        fs.read_file(
            Path::new("/d/f"),
            &ReadOptions::default().offset(2).length(0),
            &mut output,
        )
        .await
        .unwrap();
        assert!(output.is_empty());
        assert_eq!(mock.last_request().unwrap().method().as_str(), "PROPFIND");
    }

    #[tokio::test]
    async fn open_missing_file_is_no_such_file() {
        let (fs, mock) = connected().await;
        reply(&mock, 404, "");
        assert_eq!(
            fs.open(Path::new("/d/missing"), &ReadOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::NoSuchFileOrDirectory
        );
    }

    #[tokio::test]
    async fn write_file_puts_the_buffered_body() {
        let (fs, mock) = connected().await;
        reply(&mock, 204, "");
        let mut source = Cursor::new(b"hello".to_vec());
        let count = fs
            .write_file(
                Path::new("/d/new.txt"),
                &WriteOptions::default().size_hint(5),
                &mut source,
            )
            .await
            .unwrap();
        assert_eq!(count, 5);
        let request = mock.last_request().unwrap();
        assert_eq!(request.method(), http::Method::PUT);
        assert_eq!(request.uri(), "http://h/d/new.txt");
        assert_eq!(request.body(), b"hello");
    }

    #[tokio::test]
    async fn unsupported_operations_validate_the_path_first() {
        let (fs, _mock) = connected().await;
        assert_eq!(
            fs.append(Path::new("relative"), &WriteOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            fs.append(Path::new("/d/f"), &WriteOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        assert_eq!(
            fs.set_metadata(Path::new("/d/f"), &SetMetadata::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        assert_eq!(
            fs.symlink(Path::new("/d/l"), Path::new("/d/f"))
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        assert_eq!(
            fs.exec("echo 5").await.unwrap_err().kind(),
            RemoteErrorType::UnsupportedFeature
        );
    }

    #[test]
    fn range_headers_cover_offsets_and_lengths() {
        assert_eq!(range_header(0, None).unwrap(), None);
        assert_eq!(
            range_header(4, None).unwrap().unwrap(),
            http::HeaderValue::from_static("bytes=4-")
        );
        assert_eq!(
            range_header(0, Some(3)).unwrap().unwrap(),
            http::HeaderValue::from_static("bytes=0-2")
        );
        assert_eq!(
            range_header(2, Some(2)).unwrap().unwrap(),
            http::HeaderValue::from_static("bytes=2-3")
        );
        assert_eq!(
            range_header(u64::MAX, Some(2)).unwrap().unwrap(),
            http::HeaderValue::from_static("bytes=18446744073709551615-")
        );
    }

    #[test]
    fn slice_body_handles_zero_and_beyond_eof() {
        assert_eq!(slice_body(b"abcdef".to_vec(), 2, Some(2)), b"cd");
        assert_eq!(slice_body(b"abcdef".to_vec(), 100, None), b"");
        assert_eq!(slice_body(b"abcdef".to_vec(), 0, Some(0)), b"");
        assert_eq!(slice_body(b"abcdef".to_vec(), 4, Some(100)), b"ef");
        assert_eq!(slice_body(b"abcdef".to_vec(), 0, None), b"abcdef");
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn blocking_wrapper_is_a_remote_fs_trait_object() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (fs, mock) = fs();
        reply(&mock, 207, EMPTY_LISTING);
        let mut client: Box<dyn remotefs::RemoteFs> =
            Box::new(fs.into_blocking(runtime.handle().clone()));
        assert!(!client.is_connected());
        assert!(client.capabilities().contains(Capabilities::RANGE_READ));
        client.connect().unwrap();
        assert!(client.is_connected());
        reply(&mock, 207, LISTING);
        let entries = client.list_dir(Path::new("/d")).unwrap();
        assert_eq!(entries.len(), 1);
        reply(&mock, 204, "");
        let mut source = std::io::Cursor::new(b"hello".to_vec());
        let count = client
            .write_file(Path::new("/d/n"), &WriteOptions::default(), &mut source)
            .unwrap();
        assert_eq!(count, 5);
        assert_eq!(mock.last_request().unwrap().body(), b"hello");
        client.disconnect().unwrap();
    }

    #[cfg(feature = "tokio")]
    #[test]
    #[should_panic(expected = "into_blocking requires a multi-thread Tokio runtime")]
    fn blocking_wrapper_rejects_current_thread_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (fs, _mock) = fs();
        let _ = fs.into_blocking(runtime.handle().clone());
    }
}
