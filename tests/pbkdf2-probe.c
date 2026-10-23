/* SPDX-License-Identifier: MIT. Own-code synthetic PBKDF2 compatibility probe. */
#include <windows.h>
#include <bcrypt.h>
#include <stdio.h>
#include <string.h>

#ifndef BCRYPT_PBKDF2_ALG_HANDLE
#define BCRYPT_PBKDF2_ALG_HANDLE ((BCRYPT_ALG_HANDLE)0x331)
#endif
#ifndef BCRYPT_PBKDF2_ALGORITHM
#define BCRYPT_PBKDF2_ALGORITHM L"PBKDF2"
#endif

struct vector { const char *name; const WCHAR *hash; const char *password, *salt;
                ULONGLONG iterations; ULONG length; const char *expected; };
static const struct vector vectors[] = {
    {"rfc6070-sha1-1", BCRYPT_SHA1_ALGORITHM, "password", "salt", 1, 20, "0c60c80f961f0e71f3a9b524af6012062fe037a6"},
    {"sha256-2", BCRYPT_SHA256_ALGORITHM, "password", "salt", 2, 32, "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"},
    {"sha384-4096", BCRYPT_SHA384_ALGORITHM, "synthetic-password", "synthetic-salt", 4096, 48, "0c7e21b7a7e71835431c9094476b48a7610377450e1e946a83f9aa88ec0f6290ca055a5f861dec64b29c8eb3994cc318"},
    {"sha512-long-3", BCRYPT_SHA512_ALGORITHM, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "synthetic-long-password-salt", 3, 80, "f8fe70c5d708a11199b8e31245690079f1bc6b262a42bd54a1d4f3a9e1be8dbbea1b359cac2060da19f69c0763e2255b27432889d66856a84fe940cae40212b9792af8a47128c96a7cbb6fdf2687c196"},
    {"empty-sha256-1", BCRYPT_SHA256_ALGORITHM, "", "", 1, 16, "f7ce0b653d2d72a4108cf5abe912ffdd"},
};

static int run(const struct vector *v, const char *mode)
{
    BCRYPT_ALG_HANDLE alg = NULL;
    BCRYPT_KEY_HANDLE key = NULL;
    NTSTATUS status;
    UCHAR output[80], expected[80];
    ULONG written = 0, i;
    BCryptBuffer buffers[3];
    BCryptBufferDesc desc;
    int failed = 0;
    const int pseudo = !strcmp(mode, "pseudo");
    const int legacy = !strcmp(mode, "legacy");

    for (i = 0; i < v->length; ++i) {
        unsigned int value;
        sscanf(v->expected + 2 * i, "%2x", &value);
        expected[i] = (UCHAR)value;
    }
    if (pseudo) alg = BCRYPT_PBKDF2_ALG_HANDLE;
    else {
        status = BCryptOpenAlgorithmProvider(&alg, legacy ? v->hash : BCRYPT_PBKDF2_ALGORITHM,
                                             NULL, legacy ? BCRYPT_ALG_HANDLE_HMAC_FLAG : 0);
        if (status) { printf("FAIL %s %s open=%08lx\n", mode, v->name, (ULONG)status); return 1; }
    }
    if (legacy) {
        status = BCryptDeriveKeyPBKDF2(alg, (UCHAR *)v->password, strlen(v->password),
                                      (UCHAR *)v->salt, strlen(v->salt), v->iterations,
                                      output, v->length, 0);
        written = v->length;
    } else {
        status = BCryptGenerateSymmetricKey(alg, &key, NULL, 0,
                                            (UCHAR *)v->password, strlen(v->password), 0);
        if (status) { printf("FAIL %s %s generate=%08lx\n", mode, v->name, (ULONG)status); failed = 1; goto done; }
        buffers[0].BufferType = KDF_ITERATION_COUNT;
        buffers[0].pvBuffer = (void *)&v->iterations;
        buffers[0].cbBuffer = sizeof(v->iterations);
        buffers[1].BufferType = KDF_SALT;
        buffers[1].pvBuffer = (void *)v->salt;
        buffers[1].cbBuffer = strlen(v->salt);
        buffers[2].BufferType = KDF_HASH_ALGORITHM;
        buffers[2].pvBuffer = (void *)v->hash;
        buffers[2].cbBuffer = (lstrlenW(v->hash) + 1) * sizeof(WCHAR);
        desc.ulVersion = BCRYPTBUFFER_VERSION;
        desc.cBuffers = 3;
        desc.pBuffers = buffers;
        status = BCryptKeyDerivation(key, &desc, output, v->length, &written, 0);
    }
    failed = status != 0 || written != v->length || memcmp(output, expected, v->length);
    printf("%s %s %s derive=%08lx written=%lu expected_match=%s\n",
           failed ? "FAIL" : "PASS", mode, v->name, (ULONG)status, written,
           !status && written == v->length && !memcmp(output, expected, v->length) ? "yes" : "no");
done:
    if (key) BCryptDestroyKey(key);
    if (!pseudo) BCryptCloseAlgorithmProvider(alg, 0);
    return failed;
}

int main(void)
{
    int failed = 0;
    ULONG i, value, size;
    NTSTATUS status;
    const char *modes[] = {"opened", "pseudo", "legacy"};
    for (i = 0; i < sizeof(vectors)/sizeof(vectors[0]); ++i) {
        unsigned int j;
        for (j = 0; j < 3; ++j) failed += run(vectors + i, modes[j]);
    }
    status = BCryptGetProperty(BCRYPT_PBKDF2_ALG_HANDLE, BCRYPT_OBJECT_LENGTH,
                              (UCHAR *)&value, sizeof(value), &size, 0);
    printf("PBKDF2 pseudo ObjectLength status=%08lx\n", (ULONG)status);
    status = BCryptGenerateSymmetricKey((BCRYPT_ALG_HANDLE)0x321, NULL, NULL, 0, (UCHAR *)"x", 1, 0);
    printf("unsupported CAPI pseudo generate status=%08lx\n", (ULONG)status);
    failed += status != (NTSTATUS)0xc0000008;
    status = BCryptCloseAlgorithmProvider(BCRYPT_PBKDF2_ALG_HANDLE, 0);
    printf("PBKDF2 pseudo close status=%08lx\n", (ULONG)status);
    failed += status != (NTSTATUS)0xc0000008;
    printf("SUMMARY vector_cases=15 failures=%d\n", failed);
    return failed ? 1 : 0;
}
