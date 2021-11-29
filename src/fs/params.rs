//! ## Params
//!
//! file transfer parameters

/**
 * MIT License
 *
 * remotefs - Copyright (c) 2021 Christian Visintin
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */

/// ### RemoteParams
///
/// Holds connection parameters for file transfers
#[derive(Debug, Clone)]
pub enum RemoteParams {
    Generic(GenericParams),
    #[cfg(feature = "s3")]
    AwsS3(AwsS3Params),
}

/// Protocol params used by most common protocols
#[derive(Debug, Clone)]
pub struct GenericParams {
    pub address: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// Connection parameters for AWS S3 protocol
#[derive(Debug, Clone)]
#[cfg(feature = "s3")]
pub struct AwsS3Params {
    pub bucket_name: String,
    pub region: String,
    pub profile: Option<String>,
}

impl Default for RemoteParams {
    fn default() -> Self {
        Self::Generic(GenericParams::default())
    }
}

impl RemoteParams {
    /// Retrieve generic parameters from protocol params if any
    #[allow(unreachable_patterns)]
    pub fn generic_params(&self) -> Option<&GenericParams> {
        match self {
            RemoteParams::Generic(params) => Some(params),
            _ => None,
        }
    }

    /// Retrieve mutable generic parameters from protocol params if any
    #[allow(unreachable_patterns)]
    pub fn mut_generic_params(&mut self) -> Option<&mut GenericParams> {
        match self {
            RemoteParams::Generic(params) => Some(params),
            _ => None,
        }
    }

    /// Retrieve AWS S3 parameters if any
    #[cfg(feature = "s3")]
    pub fn s3_params(&self) -> Option<&AwsS3Params> {
        match self {
            RemoteParams::AwsS3(params) => Some(params),
            _ => None,
        }
    }

    /// Retrieve AWS S3 parameters if any
    #[cfg(feature = "s3")]
    pub fn mut_s3_params(&mut self) -> Option<&mut AwsS3Params> {
        match self {
            RemoteParams::AwsS3(params) => Some(params),
            _ => None,
        }
    }
}

// -- Generic protocol params

impl Default for GenericParams {
    fn default() -> Self {
        Self {
            address: "localhost".to_string(),
            port: 22,
            username: None,
            password: None,
        }
    }
}

impl GenericParams {
    /// Set address to params
    pub fn address<S: AsRef<str>>(mut self, address: S) -> Self {
        self.address = address.as_ref().to_string();
        self
    }

    /// Set port to params
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set username for params
    pub fn username<S: AsRef<str>>(mut self, username: Option<S>) -> Self {
        self.username = username.map(|x| x.as_ref().to_string());
        self
    }

    /// Set password for params
    pub fn password<S: AsRef<str>>(mut self, password: Option<S>) -> Self {
        self.password = password.map(|x| x.as_ref().to_string());
        self
    }
}

// -- S3 params

#[cfg(feature = "s3")]
impl AwsS3Params {
    /// Instantiates a new `AwsS3Params` struct
    pub fn new<S: AsRef<str>>(bucket: S, region: S, profile: Option<S>) -> Self {
        Self {
            bucket_name: bucket.as_ref().to_string(),
            region: region.as_ref().to_string(),
            profile: profile.map(|x| x.as_ref().to_string()),
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn params_default() {
        let params: GenericParams = RemoteParams::default().generic_params().unwrap().to_owned();
        assert_eq!(params.address.as_str(), "localhost");
        assert_eq!(params.port, 22);
        assert!(params.username.is_none());
        assert!(params.password.is_none());
    }

    #[test]
    #[cfg(feature = "s3")]
    fn params_aws_s3() {
        let params: AwsS3Params = AwsS3Params::new("omar", "eu-west-1", Some("test"));
        assert_eq!(params.bucket_name.as_str(), "omar");
        assert_eq!(params.region.as_str(), "eu-west-1");
        assert_eq!(params.profile.as_deref().unwrap(), "test");
    }

    #[test]
    fn generic_references() {
        let mut params = RemoteParams::default();
        #[cfg(feature = "s3")]
        assert!(params.s3_params().is_none());
        assert!(params.generic_params().is_some());
        assert!(params.mut_generic_params().is_some());
    }

    #[test]
    #[cfg(feature = "s3")]
    fn s3_references() {
        let mut params = RemoteParams::AwsS3(AwsS3Params::new("omar", "eu-west-1", Some("test")));
        assert!(params.s3_params().is_some());
        assert!(params.generic_params().is_none());
        assert!(params.mut_generic_params().is_none());
    }
}
