//! Configures authentication for Google Cloud Storage.

/// Google Cloud authentication types and credential builders.
pub use google_cloud_auth::credentials::*;

/// Selects credentials used to authenticate storage requests.
#[derive(Clone, Debug, Default)]
pub enum GoogleCloudStorageCredentials {
    /// Discovers Application Default Credentials when connecting.
    #[default]
    ApplicationDefault,
    /// Sends no authentication headers.
    Anonymous,
    /// Uses caller-provided Google Cloud credentials.
    Custom(google_cloud_auth::credentials::Credentials),
}

impl GoogleCloudStorageCredentials {
    /// Creates anonymous credentials.
    #[must_use]
    pub const fn anonymous() -> Self {
        Self::Anonymous
    }

    /// Wraps caller-provided Google Cloud credentials.
    #[must_use]
    pub fn custom(credentials: google_cloud_auth::credentials::Credentials) -> Self {
        Self::Custom(credentials)
    }
}

#[cfg(test)]
mod tests {
    use super::{Builder, Credentials, GoogleCloudStorageCredentials, anonymous};

    #[test]
    fn google_cloud_auth_types_are_reexported() {
        fn accepts_credentials(_: Credentials) {}

        accepts_credentials(anonymous::Builder::new().build());
        let _application_default_builder = Builder::default();
    }

    #[test]
    fn default_credentials_use_application_default_credentials() {
        assert!(matches!(
            GoogleCloudStorageCredentials::default(),
            GoogleCloudStorageCredentials::ApplicationDefault
        ));
    }

    #[test]
    fn anonymous_credentials_are_explicit() {
        assert!(matches!(
            GoogleCloudStorageCredentials::anonymous(),
            GoogleCloudStorageCredentials::Anonymous
        ));
    }
}
