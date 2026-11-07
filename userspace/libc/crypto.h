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

// ---- passwords ------------------------------------------------------------------
// Argon2id, which makes guessing a password from its hash slow and costly.
// A hash is a PHC string ("$argon2id$v=19$m=19456,t=2,p=1$salt$hash") that
// carries its own random salt and settings. Each call takes a while (about
// a second) and 19 MiB of the kernel's memory.

// Hashes password (len bytes) into out, NUL-terminated. Returns the
// string's length, or -1 (if it doesn't fit in max bytes, say; 128 is plenty).
long argon2id_hash(const char* password, size_t len, char* out, size_t max);

// Whether password (len bytes) is the one phc was made from.
int argon2id_verify(const char* password, size_t len, const char* phc);

// ---- Curve25519 ---------------------------------------------------------------
// Done here in the library rather than by the kernel.

// X25519 key agreement (RFC 7748): q = n * p, with p the base point if NULL.
// For a key pair, n is 32 random bytes and q = x25519(n, NULL) the public
// key; two sides' x25519(mine, theirs) then agree.
void x25519(uint8_t q[32], const uint8_t n[32], const uint8_t* p);

// Ed25519 signatures (RFC 8032). A secret key is a 32-byte random seed.
void ed25519_public_key(uint8_t pk[32], const uint8_t seed[32]);
// Writes seed's 64-byte signature of msg. Returns 0, or -1 (out of memory).
long ed25519_sign(uint8_t sig[64], const uint8_t seed[32], const void* msg, size_t len);
// Whether sig is public key pk's signature of msg.
int ed25519_verify(const uint8_t pk[32], const void* msg, size_t len, const uint8_t sig[64]);

// ---- base64 -------------------------------------------------------------------
// Done here in the library rather than by the kernel.

// The base64 (with padding) of len bytes of data, into out, NUL-terminated.
// out needs BASE64_LEN(len) bytes. Returns the length written.
#define BASE64_LEN(len) (4 * (((len) + 2) / 3) + 1)
size_t base64_encode(char* out, const void* data, size_t len);

// Decodes base64 text (len characters; padding, spaces and newlines are
// skipped) into out, which needs len / 4 * 3 + 3 bytes. Returns how many
// bytes it decoded, or -1 if the text isn't base64.
long base64_decode(uint8_t* out, const char* text, size_t len);

#endif //C_CRYPTO_H
