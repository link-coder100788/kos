// Tries out crypto.h: checks the kernel's answers against published test
// vectors, then encrypts a message, tampers with it and decrypts it.

#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/string.h"
#include "../libc/crypto.h"

static int failures = 0;

static void check(const char* what, int ok) {
    printf("%-44s %s\n", what, ok ? "ok" : "FAILED");
    if (!ok) failures++;
}

static int matches_hex(const uint8_t* bytes, size_t len, const char* hex) {
    char buf[2 * SHA512_LEN + 1];
    for (size_t i = 0; i < len; i++) snprintf(buf + 2 * i, 3, "%02x", bytes[i]);
    return strcmp(buf, hex) == 0;
}

void _start(void) {
    uint8_t hash[SHA512_LEN];

    sha256("abc", 3, hash);
    check("sha256(\"abc\")", matches_hex(hash, SHA256_LEN,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));

    sha512("abc", 3, hash);
    check("sha512(\"abc\")", matches_hex(hash, SHA512_LEN,
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a"
        "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"));

    const char* msg = "what do ya want for nothing?";
    uint8_t mac[SHA256_LEN], again[SHA256_LEN];
    hmac_sha256("Jefe", 4, msg, strlen(msg), mac);
    check("hmac_sha256 (RFC 4231 case 2)", matches_hex(mac, SHA256_LEN,
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"));
    hmac_sha256("Jeff", 4, msg, strlen(msg), again);
    check("a different key gives a different MAC", !crypto_equal(mac, again, SHA256_LEN));

    uint8_t key[AES_GCM_KEY_LEN], nonce[AES_GCM_NONCE_LEN], tag[AES_GCM_TAG_LEN];
    random_bytes(key, sizeof(key));
    random_bytes(nonce, sizeof(nonce));
    char text[] = "meet me by the KOS logo at noon";
    aead_t a = { key, nonce, "v1", 2, text, strlen(text), tag };
    check("aes_gcm_seal", aes_gcm_seal(&a) == 0 && memcmp(text, "meet", 4) != 0);

    text[3] ^= 1;
    check("aes_gcm_open refuses a changed message", aes_gcm_open(&a) == -1);
    printf("  (%s)\n", last_error());
    text[3] ^= 1;
    a.aad = "v2";
    check("aes_gcm_open refuses changed additional data", aes_gcm_open(&a) == -1);
    a.aad = "v1";
    check("aes_gcm_open decrypts the real one",
        aes_gcm_open(&a) == 0 && strcmp(text, "meet me by the KOS logo at noon") == 0);

    check("a bad pointer is an error, not a crash", sha256((void*)0x10, 64, hash) == -1);

    printf(failures ? "%d FAILED\n" : "all passed\n", failures);
    exit();
}
