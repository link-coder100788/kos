//! Argon2id password hashing, for user accounts. Hashes are PHC strings
//! (`$argon2id$v=19$m=19456,t=2,p=1$salt$hash`), so they carry their own
//! salt and parameters, and the parameters can change later without
//! breaking old hashes.

use alloc::string::{String, ToString};
use argon2::{Argon2, PasswordHasher, PasswordVerifier};

const SALT_LEN: usize = 16;

/// Hashes `password` with a fresh random salt, using OWASP's Argon2id
/// parameters (19 MiB, 2 passes, 1 lane). The memory comes from the
/// kernel heap, and a hash takes a while: keep it off the UI task.
pub fn hash(password: &[u8]) -> String {
    let mut salt = [0u8; SALT_LEN];
    crate::rand::fill(&mut salt);
    Argon2::default()
        .hash_password_with_salt(password, &salt)
        .expect("the default parameters and a 16-byte salt are valid")
        .to_string()
}

/// Checks `password` against a PHC string from `hash`, with the parameters
/// stored in it. A malformed string never matches.
pub fn verify(password: &[u8], phc: &str) -> bool {
    Argon2::default().verify_password(password, phc).is_ok()
}

#[cfg(feature = "test")]
pub mod tests {
    use crate::println;
    use super::*;

    fn test_hash_pipeline() {
        let password = b"password67";
        let hash = hash(password);
        println!("{:?}'s hash is {}", password, hash);
        let result = verify(password, &hash);
        assert!(result);
    }

    pub fn run() {
        crate::test::run_all(&[
            &test_hash_pipeline,
        ]);
    }
}
