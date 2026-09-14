//! Newtype for process-memory addresses.

use std::fmt;
use std::ops::{Add, AddAssign, Sub};

/// An address in the address space of the game process.
///
/// Deliberately a newtype so addresses never get confused with plain integers
/// (node counts, sizes, offsets read from memory, ...).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct Address(pub u64);

impl Address {
    pub const NULL: Address = Address(0);

    pub const fn null() -> Address {
        Address(0)
    }

    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    /// Checked addition of a byte offset.
    pub fn offset_by(self, offset: u64) -> Option<Address> {
        self.0.checked_add(offset).map(Address)
    }

    /// Distance from `self` to `other` in bytes (`None` if `other` is below `self`).
    pub fn distance_to(self, other: Address) -> Option<u64> {
        other.0.checked_sub(self.0)
    }
}

impl Add<u64> for Address {
    type Output = Address;

    fn add(self, rhs: u64) -> Address {
        Address(self.0 + rhs)
    }
}

impl AddAssign<u64> for Address {
    fn add_assign(&mut self, rhs: u64) {
        self.0 += rhs;
    }
}

impl Sub<u64> for Address {
    type Output = Address;

    fn sub(self, rhs: u64) -> Address {
        Address(self.0 - rhs)
    }
}

impl Sub for Address {
    type Output = u64;

    fn sub(self, rhs: Address) -> u64 {
        self.0 - rhs.0
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

impl fmt::LowerHex for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}", self.0)
    }
}

/// A half-open byte range `[start, end)` in a process address space.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AddressRange {
    pub start: Address,
    pub end: Address,
}

impl AddressRange {
    pub fn new(start: Address, size_bytes: u64) -> Self {
        AddressRange {
            start,
            end: start + size_bytes,
        }
    }

    pub fn contains(&self, address: Address) -> bool {
        address >= self.start && address < self.end
    }

    pub fn size_bytes(&self) -> u64 {
        self.end - self.start
    }
}
