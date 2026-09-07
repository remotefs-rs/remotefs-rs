//! POSIX permission bits, split per user class.
//!
//! A POSIX mode is three groups of three bits: read, write and execute, for the
//! owning user, the owning group and everyone else. [`UnixPexClass`] models one
//! group and [`UnixPex`] models all three, so a caller sets permissions by naming
//! them rather than by assembling an octal literal.
//!
//! Both types convert to and from the packed integer form, which is what a
//! protocol puts on the wire: `UnixPexClass` is one octal digit and `UnixPex` is
//! the full `0o644`-style mode. Only the nine permission bits are modelled; the
//! setuid, setgid and sticky bits are not.

/// The POSIX permissions of an entry, for all three user classes.
///
/// # Examples
///
/// ```
/// use remotefs::fs::{UnixPex, UnixPexClass};
///
/// let mode = UnixPex::from(0o644);
///
/// assert!(mode.user().read() && mode.user().write());
/// assert!(!mode.others().write());
/// assert_eq!(u32::from(mode), 0o644);
/// ```
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct UnixPex(UnixPexClass, UnixPexClass, UnixPexClass);

impl UnixPex {
    /// Build a mode from the permissions of the three user classes.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs::fs::{UnixPex, UnixPexClass};
    ///
    /// let mode = UnixPex::new(
    ///     UnixPexClass::new(true, true, true),
    ///     UnixPexClass::new(true, false, true),
    ///     UnixPexClass::new(true, false, true),
    /// );
    ///
    /// assert_eq!(u32::from(mode), 0o755);
    /// ```
    pub fn new(user: UnixPexClass, group: UnixPexClass, others: UnixPexClass) -> Self {
        Self(user, group, others)
    }

    /// Return the permissions of the owning user.
    pub fn user(&self) -> UnixPexClass {
        self.0
    }

    /// Return the permissions of the owning group.
    pub fn group(&self) -> UnixPexClass {
        self.1
    }

    /// Return the permissions of everyone else.
    pub fn others(&self) -> UnixPexClass {
        self.2
    }
}

impl From<UnixPex> for u32 {
    fn from(pex: UnixPex) -> Self {
        (u32::from(pex.0) << 6) + (u32::from(pex.1) << 3) + u32::from(pex.2)
    }
}

/// Bits above the low nine are ignored, so a full `st_mode` converts cleanly.
impl From<u32> for UnixPex {
    fn from(x: u32) -> Self {
        UnixPex::new(
            UnixPexClass::from(((x >> 6) & 0x7) as u8),
            UnixPexClass::from(((x >> 3) & 0x7) as u8),
            UnixPexClass::from((x & 0x7) as u8),
        )
    }
}

/// The read, write and execute permissions of one POSIX user class.
///
/// # Examples
///
/// ```
/// use remotefs::fs::UnixPexClass;
///
/// let class = UnixPexClass::from(5);
///
/// assert!(class.read() && class.execute());
/// assert!(!class.write());
/// assert_eq!(class.as_byte(), 5);
/// ```
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct UnixPexClass {
    read: bool,
    write: bool,
    execute: bool,
}

impl UnixPexClass {
    /// Build the permissions of one user class from the three bits.
    pub fn new(read: bool, write: bool, execute: bool) -> Self {
        Self {
            read,
            write,
            execute,
        }
    }

    /// Return whether the class may read.
    pub fn read(&self) -> bool {
        self.read
    }

    /// Return whether the class may write.
    pub fn write(&self) -> bool {
        self.write
    }

    /// Return whether the class may execute, or traverse a directory.
    pub fn execute(&self) -> bool {
        self.execute
    }

    /// Pack the three bits into the octal digit POSIX uses.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs::fs::UnixPexClass;
    ///
    /// assert_eq!(UnixPexClass::new(true, true, true).as_byte(), 7);
    /// assert_eq!(UnixPexClass::new(false, false, false).as_byte(), 0);
    /// ```
    pub fn as_byte(&self) -> u8 {
        ((self.read as u8) << 2) + ((self.write as u8) << 1) + (self.execute as u8)
    }
}

/// Bits above the low three are ignored.
impl From<u8> for UnixPexClass {
    fn from(bits: u8) -> Self {
        Self {
            read: ((bits >> 2) & 0x01) != 0,
            write: ((bits >> 1) & 0x01) != 0,
            execute: (bits & 0x01) != 0,
        }
    }
}

impl From<UnixPexClass> for u32 {
    fn from(pex: UnixPexClass) -> Self {
        ((pex.read as u32) << 2) + ((pex.write as u32) << 1) + (pex.execute as u32)
    }
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_create_unix_pex_class() {
        let pex: UnixPexClass = UnixPexClass::from(4);
        assert!(pex.read());
        assert!(!pex.write());
        assert!(!pex.execute());
        let pex: UnixPexClass = UnixPexClass::from(0);
        assert!(!pex.read());
        assert!(!pex.write());
        assert!(!pex.execute());
        let pex: UnixPexClass = UnixPexClass::from(3);
        assert!(!pex.read());
        assert!(pex.write());
        assert!(pex.execute());
        let pex: UnixPexClass = UnixPexClass::from(7);
        assert!(pex.read());
        assert!(pex.write());
        assert!(pex.execute());
        let pex: UnixPexClass = UnixPexClass::from(3);
        assert_eq!(pex.as_byte(), 3);
        let pex: UnixPexClass = UnixPexClass::from(7);
        assert_eq!(pex.as_byte(), 7);
    }

    #[test]
    fn should_create_unix_pex() {
        let pex = UnixPex::new(
            UnixPexClass::from(6),
            UnixPexClass::from(4),
            UnixPexClass::from(0),
        );
        assert_eq!(pex.user().as_byte(), 6);
        assert_eq!(pex.group().as_byte(), 4);
        assert_eq!(pex.others().as_byte(), 0);
    }

    #[test]
    fn should_convert_unix_pex_to_byte() {
        let pex = UnixPex::new(
            UnixPexClass::from(6),
            UnixPexClass::from(4),
            UnixPexClass::from(2),
        );
        assert_eq!(u32::from(pex), 0o642);
        let pex = UnixPex::new(
            UnixPexClass::from(7),
            UnixPexClass::from(5),
            UnixPexClass::from(5),
        );
        assert_eq!(u32::from(pex), 0o755);
    }

    #[test]
    fn should_convert_u32_to_unix_pex() {
        assert_eq!(
            UnixPex::from(0o754),
            UnixPex::new(
                UnixPexClass::from(7),
                UnixPexClass::from(5),
                UnixPexClass::from(4),
            )
        );
    }
}
