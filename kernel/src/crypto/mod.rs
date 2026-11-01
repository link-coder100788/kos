//! Hashes, MACs and authenticated encryption, for programs (the crypto
//! syscall). They come from the RustCrypto crates TLS already uses.

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};

pub mod argon;

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub fn sha512(data: &[u8]) -> [u8; 64] {
    Sha512::digest(data).into()
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Encrypts `data` in place with AES-256-GCM and returns the tag `open`
/// checks. `aad` is authenticated too, but not encrypted. A nonce must
/// never be used twice with the same key.
pub fn aes256gcm_seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], data: &mut [u8]) -> [u8; 16] {
    Aes256Gcm::new(key.into())
        .encrypt_in_place_detached(Nonce::from_slice(nonce), aad, data)
        .expect("AES-GCM takes up to 64 GiB")
        .into()
}

/// Decrypts `data` in place if `tag` proves it (and `aad`) unchanged.
/// Returns false, leaving `data` as it was, if not.
pub fn aes256gcm_open(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], data: &mut [u8], tag: &[u8; 16]) -> bool {
    Aes256Gcm::new(key.into())
        .decrypt_in_place_detached(Nonce::from_slice(nonce), aad, data, Tag::from_slice(tag))
        .is_ok()
}

#[cfg(feature = "test")]
pub mod tests {
    fn hex(s: &str) -> alloc::vec::Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    // Test vectors: FIPS 180-2 ("abc") and RFC 4231 test case 2.
    fn test_crypto_sha256() {
        assert_eq!(super::sha256(b"abc").to_vec(), hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
    }

    fn test_crypto_sha512() {
        assert_eq!(super::sha512(b"abc").to_vec(), hex(concat!(
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a",
            "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f")));
    }

    fn test_crypto_hmac_sha256() {
        assert_eq!(super::hmac_sha256(b"Jefe", b"what do ya want for nothing?").to_vec(),
            hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"));
    }

    fn test_crypto_aes256gcm_round_trip_and_tampering() {
        let (key, nonce) = ([7u8; 32], [9u8; 12]);
        let plain = *b"attack at dawn!";
        let mut data = plain;
        let tag = super::aes256gcm_seal(&key, &nonce, b"header", &mut data);
        assert_ne!(data, plain);
        let sealed = data;

        let mut bad_tag = tag;
        bad_tag[0] ^= 1;
        assert!(!super::aes256gcm_open(&key, &nonce, b"header", &mut data, &bad_tag));
        assert!(!super::aes256gcm_open(&key, &nonce, b"other", &mut data, &tag));
        assert_eq!(data, sealed, "a failed open leaves the data alone");

        assert!(super::aes256gcm_open(&key, &nonce, b"header", &mut data, &tag));
        assert_eq!(data, plain);
    }

    pub fn run() {
        crate::test::run_all(&[
            &test_crypto_sha256,
            &test_crypto_sha512,
            &test_crypto_hmac_sha256,
            &test_crypto_aes256gcm_round_trip_and_tampering,
        ]);
        super::argon::tests::run();
    }
}
