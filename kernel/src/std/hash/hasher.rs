use core::hash::{BuildHasherDefault, Hasher};
use crate::std::hash::fnv::fnv1a_64_continue;

pub struct FnvHasher {
    state: u64,
}

impl Default for FnvHasher {
    #[inline]
    fn default() -> Self {
        Self { state: 14695981039346656037 }
    }
}

impl Hasher for FnvHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.state = fnv1a_64_continue(self.state, bytes);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.state
    }
}

pub type BuildHasher = BuildHasherDefault<FnvHasher>;
