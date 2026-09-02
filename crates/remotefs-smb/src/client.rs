//! # client
//!
//! Smb fs client

/// An SMB protocol dialect used to bound protocol negotiation.
///
/// Variants are ordered from oldest to newest. [`SmbDialect::Nt1`] is the
/// deprecated SMB1/CIFS dialect and must be selected explicitly.
///
/// # Examples
///
/// ```
/// use remotefs_smb::SmbDialect;
///
/// assert!(SmbDialect::Smb202 < SmbDialect::Smb311);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SmbDialect {
    /// The deprecated SMB1/CIFS `NT1` dialect.
    Nt1,
    /// The SMB 2.0.2 dialect.
    Smb202,
    /// The SMB 2.1 dialect.
    Smb210,
    /// The SMB 3.0 dialect.
    Smb300,
    /// The SMB 3.0.2 dialect.
    Smb302,
    /// The SMB 3.1.1 dialect.
    Smb311,
}

const AUTO_MIN_DIALECT: SmbDialect = SmbDialect::Smb202;
const AUTO_MAX_DIALECT: SmbDialect = SmbDialect::Smb311;

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_use_secure_auto_dialect_bounds() {
        assert_eq!(AUTO_MIN_DIALECT, SmbDialect::Smb202);
        assert_eq!(AUTO_MAX_DIALECT, SmbDialect::Smb311);
        assert_ne!(AUTO_MIN_DIALECT, SmbDialect::Nt1);
    }

    #[test]
    fn should_copy_dialects() {
        let dialect = SmbDialect::Smb302;
        let copied = dialect;
        assert_eq!(copied, SmbDialect::Smb302);
        assert_eq!(dialect, SmbDialect::Smb302);
    }
}

// -- unix client

#[cfg(target_family = "unix")]
mod unix;
#[cfg(target_family = "unix")]
pub use unix::*;

// -- windows client

#[cfg(target_family = "windows")]
mod windows;
#[cfg(target_family = "windows")]
pub use windows::*;
