//! ## Permissions
//!
//! POSIX permissions

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

#[cfg(test)]
mod test {

    use super::*;

    use pretty_assertions::assert_eq;

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
}
