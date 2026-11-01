#ifndef C_CRYPTO_H
#define C_CRYPTO_H

// Hashes, MACs and authenticated encryption, done by the kernel (the same
// code its HTTPS uses). For random keys and nonces, see random_bytes() in
// sys.h. On failure these return -1, and last_error() says why.

#include <stddef.h>
#include <stdint.h>

#define SHA256_LEN 32
#define SHA512_LEN 64

long sha256(const void* data, size_t len, uint8_t out[SHA256_LEN]);
long sha512(const void* data, size_t len, uint8_t out[SHA512_LEN]);

// A message authentication code: proves data came from someone with key.
// Check one with crypto_equal, not memcmp.
long hmac_sha256(const void* key, size_t key_len, const void* data, size_t len, uint8_t out[SHA256_LEN]);

// Whether a and b hold the same len bytes, taking the same time whichever
// byte differs, so an attacker can't time its way to a valid MAC.
int crypto_equal(const void* a, const void* b, size_t len);

// ---- AES-256-GCM ---------------------------------------------------------------
// Encryption that also detects tampering. Never use a nonce twice with
// the same key: a random one (random_bytes) or a counter both work.

#define AES_GCM_KEY_LEN 32
#define AES_GCM_NONCE_LEN 12
#define AES_GCM_TAG_LEN 16

typedef struct {
    const uint8_t* key;   // AES_GCM_KEY_LEN bytes
    const uint8_t* nonce; // AES_GCM_NONCE_LEN bytes
    const void* aad;      // authenticated but not encrypted (e.g. a header); may be NULL
    size_t aad_len;
    void* data;           // encrypted or decrypted in place
    size_t len;
    uint8_t* tag;         // AES_GCM_TAG_LEN bytes
} aead_t;

// Encrypts a->data in place and writes a->tag. Returns 0, or -1.
long aes_gcm_seal(const aead_t* a);

// Checks a->tag against a->data and a->aad, then decrypts a->data in
// place. Returns 0, or -1 (data left as it was) if anything was changed.
long aes_gcm_open(const aead_t* a);

#endif //C_CRYPTO_H
