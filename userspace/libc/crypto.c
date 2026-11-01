#include "crypto.h"

#define SVC_CRYPTO(sub, a0, a1, a2, a3, a4) ({                           \
    register uint64_t x0 __asm("x0") = (uint64_t)(a0);                  \
    register uint64_t x1 __asm("x1") = (uint64_t)(a1);                  \
    register uint64_t x2 __asm("x2") = (uint64_t)(a2);                  \
    register uint64_t x3 __asm("x3") = (uint64_t)(a3);                  \
    register uint64_t x4 __asm("x4") = (uint64_t)(a4);                  \
    register uint64_t x16 __asm("x16") = (uint64_t)(sub);               \
    __asm volatile("svc #0xf"                                            \
                   : "+r"(x0)                                           \
                   : "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x16)       \
                   : "memory");                                         \
    (long)x0;                                                            \
})

enum { SHA256, SHA512, HMAC_SHA256, AES_GCM_SEAL, AES_GCM_OPEN };

long sha256(const void* data, size_t len, uint8_t out[SHA256_LEN]) {
    return SVC_CRYPTO(SHA256, data, len, out, 0, 0);
}

long sha512(const void* data, size_t len, uint8_t out[SHA512_LEN]) {
    return SVC_CRYPTO(SHA512, data, len, out, 0, 0);
}

long hmac_sha256(const void* key, size_t key_len, const void* data, size_t len, uint8_t out[SHA256_LEN]) {
    return SVC_CRYPTO(HMAC_SHA256, key, key_len, data, len, out);
}

int crypto_equal(const void* a, const void* b, size_t len) {
    const volatile uint8_t* x = a;
    const volatile uint8_t* y = b;
    uint8_t diff = 0;
    for (size_t i = 0; i < len; i++) diff |= x[i] ^ y[i];
    return diff == 0;
}

long aes_gcm_seal(const aead_t* a) {
    return SVC_CRYPTO(AES_GCM_SEAL, a, 0, 0, 0, 0);
}

long aes_gcm_open(const aead_t* a) {
    return SVC_CRYPTO(AES_GCM_OPEN, a, 0, 0, 0, 0);
}
