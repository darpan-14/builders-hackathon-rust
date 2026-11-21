use crate::input::Str;
use rustc_hash::FxBuildHasher;
use std::hash::BuildHasher;

/// Store a common short user ID in one integer. The long representation also
/// preserves trailing NULs, uppercase IDs and every other spelling verbatim.
#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub enum Name<'a> {
    Short(u64),
    Other(Str<'a>),
}
impl<'a> Name<'a> {
    pub fn new(name: Str<'a>) -> Self {
        if let Some(rest) = name.strip_prefix(b"u")
            && rest.len() <= 8
            && rest.last() != Some(&0)
        {
            let raw = if let Ok(eight) = <&[u8; 8]>::try_from(rest) {
                *eight
            } else {
                let mut padded = [0; 8];
                padded[..rest.len()].copy_from_slice(rest);
                padded
            };
            Self::Short(u64::from_le_bytes(raw))
        } else {
            Self::Other(name)
        }
    }
    pub fn hash(self) -> usize {
        FxBuildHasher.hash_one(self) as usize
    }
    pub fn write(self, mut output: impl FnMut(&[u8])) {
        match self {
            Self::Other(s) => output(s),
            Self::Short(value) => {
                let mut buf = [b'u'; 9];
                buf[1..].copy_from_slice(&value.to_le_bytes());
                output(&buf[..9 - (value.leading_zeros() / 8) as usize]);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_spelling() {
        for s in [
            b"".as_slice(),
            b"u",
            b"u01234567",
            b"u012345678",
            b"u\0a",
            b"ua\0",
            b"U1",
            "u写真".as_bytes(),
        ] {
            Name::new(s).write(|out| assert_eq!(out, s));
        }
    }
}
