//! ## Permissions
//!
//! POSIX permissions

/// Describes the permissions on POSIX system.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct UnixPex(UnixPexClass, UnixPexClass, UnixPexClass);

impl UnixPex {
    /// Create a new `UnixPex`
    pub fn new(user: UnixPexClass, group: UnixPexClass, others: UnixPexClass) -> Self {
        Self(user, group, others)
    }

    /// Returns unix permissions class for `user`
    pub fn user(&self) -> UnixPexClass {
        self.0
    }

    /// Returns unix permissions class for `group`
    pub fn group(&self) -> UnixPexClass {
        self.1
    }

    /// Returns unix permissions class for `others`
    pub fn others(&self) -> UnixPexClass {
        self.2
    }
}

impl From<UnixPex> for u32 {
    fn from(pex: UnixPex) -> Self {
        (u32::from(pex.0) << 6) + (u32::from(pex.1) << 3) + u32::from(pex.2)
    }
}

impl From<u32> for UnixPex {
    fn from(x: u32) -> Self {
        UnixPex::new(
            UnixPexClass::from(((x >> 6) & 0x7) as u8),
            UnixPexClass::from(((x >> 3) & 0x7) as u8),
            UnixPexClass::from((x & 0x7) as u8),
        )
    }
}

/// Describes the permissions on POSIX system for a user class
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct UnixPexClass {
    read: bool,
    write: bool,
    execute: bool,
}

impl UnixPexClass {
    /// Instantiates a new `UnixPex`
    pub fn new(read: bool, write: bool, execute: bool) -> Self {
        Self {
            read,
            write,
            execute,
        }
    }

    /// Returns whether user can read
    pub fn read(&self) -> bool {
        self.read
    }

    /// Returns whether user can write
    pub fn write(&self) -> bool {
        self.write
    }

    /// Returns whether user can execute
    pub fn execute(&self) -> bool {
        self.execute
    }

    /// Convert permission to byte as on POSIX systems
    pub fn as_byte(&self) -> u8 {
        ((self.read as u8) << 2) + ((self.write as u8) << 1) + (self.execute as u8)
    }
}

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
        assert_eq!(pex.read(), true);
        assert_eq!(pex.write(), false);
        assert_eq!(pex.execute(), false);
        let pex: UnixPexClass = UnixPexClass::from(0);
        assert_eq!(pex.read(), false);
        assert_eq!(pex.write(), false);
        assert_eq!(pex.execute(), false);
        let pex: UnixPexClass = UnixPexClass::from(3);
        assert_eq!(pex.read(), false);
        assert_eq!(pex.write(), true);
        assert_eq!(pex.execute(), true);
        let pex: UnixPexClass = UnixPexClass::from(7);
        assert_eq!(pex.read(), true);
        assert_eq!(pex.write(), true);
        assert_eq!(pex.execute(), true);
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
