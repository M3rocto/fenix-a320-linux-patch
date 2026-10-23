/* SPDX-License-Identifier: MIT. Own synthetic NCrypt ECDH contract probe. */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <bcrypt.h>
#include <ncrypt.h>
#include <stdio.h>
#include <string.h>
#include "ncrypt-ecdh-vectors.h"
#ifndef NCRYPTBUFFER_ECC_CURVE_NAME
#define NCRYPTBUFFER_ECC_CURVE_NAME 60
#endif
static unsigned int checks, failures;
static void expect(const char *label, SECURITY_STATUS actual, SECURITY_STATUS wanted)
{
    checks++; failures += actual != wanted;
    printf("%s %s actual=%08lx expected=%08lx\n",actual==wanted?"PASS":"FAIL",label,(ULONG)actual,(ULONG)wanted);
}
static void truth(const char *label, int result)
{
    checks++; failures += !result; printf("%s %s\n",result?"PASS":"FAIL",label);
}
static SECURITY_STATUS import(NCRYPT_PROV_HANDLE provider, const UCHAR *data, DWORD size, BOOL private_key, NCRYPT_KEY_HANDLE *key)
{
    return NCryptImportKey(provider,0,private_key?BCRYPT_ECCPRIVATE_BLOB:BCRYPT_ECCPUBLIC_BLOB,NULL,key,(BYTE *)data,size,0);
}
static void derivation(NCRYPT_SECRET_HANDLE secret, const char *label, const WCHAR *kdf, const WCHAR *hash,
                       const UCHAR *expected, DWORD length)
{
    NCryptBuffer buffer;
    NCryptBufferDesc desc;
    UCHAR output[80]; DWORD written=0;
    NCryptBufferDesc *params=NULL;
    DWORD flags=wcscmp(kdf,BCRYPT_KDF_HASH)?0:KDF_USE_SECRET_AS_HMAC_KEY_FLAG;
    SECURITY_STATUS status;
    printf("DERIVE %s\n",label);
    if(hash) {
        buffer.BufferType=KDF_HASH_ALGORITHM;buffer.pvBuffer=(void *)hash;buffer.cbBuffer=(lstrlenW(hash)+1)*sizeof(WCHAR);
        desc.ulVersion=0;desc.cBuffers=1;desc.pBuffers=&buffer;params=&desc;
    }
    status=NCryptDeriveKey(secret,kdf,params,NULL,0,&written,flags);
    expect("derive size query",status,0);truth("derive required length",written==length);
    memset(output,0xcc,sizeof(output));
    status=NCryptDeriveKey(secret,kdf,params,output,sizeof(output),&written,flags);
    expect("derive full",status,0);truth("derive fixed reference",written==length&&!memcmp(output,expected,length)&&output[length]==0xcc);
    memset(output,0xcc,sizeof(output));
    status=NCryptDeriveKey(secret,kdf,params,output,13,&written,flags);
    expect("derive short success truncation",status,0);truth("derive short prefix and bound",written==13&&!memcmp(output,expected,13)&&output[13]==0xcc);
    memset(output,0xcc,sizeof(output));
    status=NCryptDeriveKey(secret,kdf,params,output,0,&written,flags);
    expect("derive zero capacity success",status,0);truth("derive zero copied",written==0&&output[0]==0xcc);
}
static int lifetime_cycles(void)
{
    unsigned int completed=0;
    for(unsigned int i=0;i<1000;i++) {
        NCRYPT_PROV_HANDLE pa=0,pb=0;
        NCRYPT_KEY_HANDLE ka=0,kb=0;
        NCRYPT_SECRET_HANDLE secret=0;
        UCHAR result[32]; DWORD written=0;
        SECURITY_STATUS status=NCryptOpenStorageProvider(&pa,MS_KEY_STORAGE_PROVIDER,0);
        if(!status)status=NCryptOpenStorageProvider(&pb,MS_KEY_STORAGE_PROVIDER,0);
        if(!status)status=import(pa,alice_private,sizeof(alice_private),TRUE,&ka);
        if(!status)status=import(pb,bob_public,sizeof(bob_public),FALSE,&kb);
        if(!status)status=NCryptSecretAgreement(ka,kb,&secret,0);
        if(pa)NCryptFreeObject(pa);
        if(pb)NCryptFreeObject(pb);
        if(ka)NCryptFreeObject(ka);
        if(kb)NCryptFreeObject(kb);
        if(!status)status=NCryptDeriveKey(secret,BCRYPT_KDF_RAW_SECRET,NULL,result,sizeof(result),&written,0);
        if(secret)NCryptFreeObject(secret);
        if(status||written!=sizeof(raw_cng_little_endian)||memcmp(result,raw_cng_little_endian,written))break;
        completed++;
    }
    printf("LIFETIME completed=%u\n",completed);
    return completed==1000;
}
int main(void)
{
    NCRYPT_PROV_HANDLE p1=0,p2=0,owned=0,unsupported=0;
    NCRYPT_KEY_HANDLE generated=0,a=0,b=0,bp=0,cp=0,rsa=0,bad=0;
    NCRYPT_SECRET_HANDLE secret=0,other=0;
    SECURITY_STATUS status;
    DWORD size,value,capacity;
    UCHAR output[160],blob[sizeof(alice_private)+1];
    WCHAR algorithm[64];
    BYTE ephemeral=1;
    NCryptBuffer parameter; NCryptBufferDesc desc;
    expect("provider1",NCryptOpenStorageProvider(&p1,MS_KEY_STORAGE_PROVIDER,0),0);
    expect("provider2",NCryptOpenStorageProvider(&p2,MS_KEY_STORAGE_PROVIDER,0),0);
    status=NCryptCreatePersistedKey(p1,&generated,BCRYPT_ECDH_P256_ALGORITHM,NULL,0,0);
    expect("ephemeral P256 creation",status,0);
    if(status) goto done;
    expect("null provider",NCryptCreatePersistedKey(0,&bad,BCRYPT_ECDH_P256_ALGORITHM,NULL,0,0),NTE_INVALID_HANDLE);
    expect("wrong provider type",NCryptCreatePersistedKey(generated,&bad,BCRYPT_ECDH_P256_ALGORITHM,NULL,0,0),NTE_INVALID_HANDLE);
    expect("wrong RSA provider type",NCryptCreatePersistedKey(generated,&bad,BCRYPT_RSA_ALGORITHM,NULL,0,0),NTE_INVALID_HANDLE);
    expect("named key unsupported",NCryptCreatePersistedKey(p1,&bad,BCRYPT_ECDH_P256_ALGORITHM,L"synthetic-never-persisted",0,0),NTE_NOT_SUPPORTED);
    expect("create unsupported flags",NCryptCreatePersistedKey(p1,&bad,BCRYPT_ECDH_P256_ALGORITHM,NULL,0,0x20),NTE_BAD_FLAGS);
    expect("create unsupported curve",NCryptCreatePersistedKey(p1,&bad,BCRYPT_ECDH_P384_ALGORITHM,NULL,0,0),NTE_NOT_SUPPORTED);
    expect("create legacy key spec",NCryptCreatePersistedKey(p1,&bad,BCRYPT_ECDH_P256_ALGORITHM,NULL,1,0),NTE_INVALID_PARAMETER);
    expect("provider unsupported identity open baseline",NCryptOpenStorageProvider(&unsupported,L"synthetic unsupported KSP",0),0);
    expect("unsupported provider ECDH",NCryptCreatePersistedKey(unsupported,&bad,BCRYPT_ECDH_P256_ALGORITHM,NULL,0,0),NTE_NOT_SUPPORTED);
    NCryptFreeObject(unsupported);unsupported=0;
    expect("algorithm size",NCryptGetProperty(generated,NCRYPT_ALGORITHM_PROPERTY,NULL,0,&size,0),0);
    truth("algorithm property length",size==sizeof(L"ECDH_P256"));
    expect("algorithm short buffer",NCryptGetProperty(generated,NCRYPT_ALGORITHM_PROPERTY,(BYTE *)algorithm,1,&size,0),NTE_BUFFER_TOO_SMALL);
    expect("algorithm value",NCryptGetProperty(generated,NCRYPT_ALGORITHM_PROPERTY,(BYTE *)algorithm,sizeof(algorithm),&size,0),0);
    truth("algorithm exact",!wcscmp(algorithm,L"ECDH_P256"));
    expect("null property result",NCryptGetProperty(generated,NCRYPT_LENGTH_PROPERTY,output,sizeof(output),NULL,0),NTE_INVALID_PARAMETER);
    value=384;expect("fixed curve length",NCryptSetProperty(generated,NCRYPT_LENGTH_PROPERTY,(BYTE *)&value,sizeof(value),0),NTE_INVALID_PARAMETER);
    value=256;expect("length short input",NCryptSetProperty(generated,NCRYPT_LENGTH_PROPERTY,(BYTE *)&value,2,0),NTE_INVALID_PARAMETER);
    expect("length unchanged",NCryptSetProperty(generated,NCRYPT_LENGTH_PROPERTY,(BYTE *)&value,sizeof(value),0),0);
    expect("provider property immutable",NCryptSetProperty(generated,NCRYPT_PROVIDER_HANDLE_PROPERTY,(BYTE *)&value,1,0),NTE_NOT_SUPPORTED);
    expect("custom zero size rejected",NCryptSetProperty(generated,L"synthetic empty custom",NULL,0,NCRYPT_PERSIST_ONLY_FLAG),NTE_INVALID_PARAMETER);
    expect("CLR ephemeral custom",NCryptSetProperty(generated,L"CLR IsEphemeral",&ephemeral,1,NCRYPT_PERSIST_ONLY_FLAG),0);
    expect("public export before finalize",NCryptExportKey(generated,0,BCRYPT_ECCPUBLIC_BLOB,NULL,NULL,0,&size,0),NTE_BAD_KEY_STATE);
    expect("import Alice private",import(p1,alice_private,sizeof(alice_private),TRUE,&a),0);
    expect("import Bob private",import(p2,bob_private,sizeof(bob_private),TRUE,&b),0);
    expect("import Bob public",import(p2,bob_public,sizeof(bob_public),FALSE,&bp),0);
    expect("import Carol public",import(p2,carol_public,sizeof(carol_public),FALSE,&cp),0);
    if(!a||!b||!bp||!cp) goto done;
    expect("agreement before finalized",NCryptSecretAgreement(generated,bp,&other,0),NTE_BAD_KEY_STATE);
    expect("finalize",NCryptFinalizeKey(generated,0),0);
    expect("finalize twice",NCryptFinalizeKey(generated,0),NTE_INVALID_HANDLE);
    expect("change length after finalize",NCryptSetProperty(generated,NCRYPT_LENGTH_PROPERTY,(BYTE *)&value,sizeof(value),0),NTE_BAD_KEY_STATE);
    expect("public export size",NCryptExportKey(bp,0,BCRYPT_ECCPUBLIC_BLOB,NULL,NULL,0,&size,0),0);truth("public export size72",size==72);
    memset(output,0xcc,sizeof(output));expect("export short fails",NCryptExportKey(bp,0,BCRYPT_ECCPUBLIC_BLOB,NULL,output,71,&size,0),NTE_BUFFER_TOO_SMALL);
    truth("export short bounds",size==72&&output[0]==0xcc);
    expect("export public",NCryptExportKey(bp,0,BCRYPT_ECCPUBLIC_BLOB,NULL,output,sizeof(output),&size,0),0);truth("export exact public vector",size==sizeof(bob_public)&&!memcmp(output,bob_public,size));
    expect("private export default denied",NCryptExportKey(a,0,BCRYPT_ECCPRIVATE_BLOB,NULL,NULL,0,&size,0),NTE_PERM);
    value=NCRYPT_ALLOW_PLAINTEXT_EXPORT_FLAG;expect("private export policy",NCryptSetProperty(a,NCRYPT_EXPORT_POLICY_PROPERTY,(BYTE *)&value,sizeof(value),NCRYPT_PERSIST_FLAG),0);
    expect("private export allowed",NCryptExportKey(a,0,BCRYPT_ECCPRIVATE_BLOB,NULL,output,sizeof(output),&size,0),0);truth("export exact private vector",size==sizeof(alice_private)&&!memcmp(output,alice_private,size));
    expect("public cannot export private",NCryptExportKey(bp,0,BCRYPT_ECCPRIVATE_BLOB,NULL,NULL,0,&size,0),NTE_BAD_KEY_STATE);
    expect("RSA create unchanged",NCryptCreatePersistedKey(p1,&rsa,BCRYPT_RSA_ALGORITHM,NULL,0,0),0);
    expect("wrong key algorithm",NCryptSecretAgreement(rsa,bp,&other,0),NTE_NOT_SUPPORTED);
    expect("null agreement key",NCryptSecretAgreement(0,bp,&other,0),NTE_INVALID_HANDLE);
    expect("provider as key",NCryptSecretAgreement(p1,bp,&other,0),NTE_INVALID_HANDLE);
    expect("public as private",NCryptSecretAgreement(bp,bp,&other,0),NTE_BAD_KEY_STATE);
    expect("agreement null output",NCryptSecretAgreement(a,bp,NULL,0),NTE_INVALID_PARAMETER);
    expect("agreement flags",NCryptSecretAgreement(a,bp,&other,1),NTE_BAD_FLAGS);
    expect("agreement separate provider handles",NCryptSecretAgreement(a,bp,&secret,0),0);
    if(!secret) goto done;
    expect("secret as ECDH provider",NCryptCreatePersistedKey(secret,&bad,BCRYPT_ECDH_P256_ALGORITHM,NULL,0,0),NTE_INVALID_HANDLE);
    expect("secret as RSA provider",NCryptCreatePersistedKey(secret,&bad,BCRYPT_RSA_ALGORITHM,NULL,0,0),NTE_INVALID_HANDLE);
    expect("secret get property rejected",NCryptGetProperty(secret,NCRYPT_LENGTH_PROPERTY,output,sizeof(output),&size,0),NTE_INVALID_HANDLE);
    expect("secret set property rejected",NCryptSetProperty(secret,L"synthetic",&ephemeral,1,0),NTE_INVALID_HANDLE);
    derivation(secret,"default SHA1",BCRYPT_KDF_HASH,NULL,sha1,sizeof(sha1));
    derivation(secret,"SHA256",BCRYPT_KDF_HASH,BCRYPT_SHA256_ALGORITHM,sha256,sizeof(sha256));
    derivation(secret,"SHA384",BCRYPT_KDF_HASH,BCRYPT_SHA384_ALGORITHM,sha384,sizeof(sha384));
    derivation(secret,"SHA512",BCRYPT_KDF_HASH,BCRYPT_SHA512_ALGORITHM,sha512,sizeof(sha512));
    derivation(secret,"RAW little endian",BCRYPT_KDF_RAW_SECRET,NULL,raw_cng_little_endian,sizeof(raw_cng_little_endian));
    expect("derive wrong handle type",NCryptDeriveKey(a,BCRYPT_KDF_HASH,NULL,output,sizeof(output),&size,0),NTE_INVALID_HANDLE);
    expect("derive null handle",NCryptDeriveKey(0,BCRYPT_KDF_HASH,NULL,output,sizeof(output),&size,0),NTE_INVALID_HANDLE);
    expect("derive null result",NCryptDeriveKey(secret,BCRYPT_KDF_HASH,NULL,output,sizeof(output),NULL,0),NTE_INVALID_PARAMETER);
    expect("derive unsupported HMAC",NCryptDeriveKey(secret,BCRYPT_KDF_HMAC,NULL,output,sizeof(output),&size,0),NTE_NOT_SUPPORTED);
    expect("derive bad flags",NCryptDeriveKey(secret,BCRYPT_KDF_HASH,NULL,output,sizeof(output),&size,2),NTE_BAD_FLAGS);
    parameter.BufferType=KDF_SECRET_APPEND;parameter.cbBuffer=1;parameter.pvBuffer=&ephemeral;desc.ulVersion=0;desc.cBuffers=1;desc.pBuffers=&parameter;
    expect("derive unsupported append",NCryptDeriveKey(secret,BCRYPT_KDF_HASH,&desc,output,sizeof(output),&size,0),NTE_NOT_SUPPORTED);
    parameter.BufferType=KDF_HASH_ALGORITHM;parameter.cbBuffer=2;parameter.pvBuffer=(void *)BCRYPT_SHA256_ALGORITHM;
    expect("derive unterminated hash",NCryptDeriveKey(secret,BCRYPT_KDF_HASH,&desc,output,sizeof(output),&size,0),NTE_INVALID_PARAMETER);
    memcpy(blob,bob_public,sizeof(bob_public));((BCRYPT_ECCKEY_BLOB *)blob)->cbKey=31;
    expect("import wrong width",import(p1,blob,sizeof(bob_public),FALSE,&bad),NTE_INVALID_PARAMETER);
    expect("import truncated blob",import(p1,bob_public,sizeof(bob_public)-1,FALSE,&bad),NTE_INVALID_PARAMETER);
    memcpy(blob,bob_public,sizeof(bob_public));blob[72]=0;
    expect("import extra bytes",import(p1,blob,sizeof(bob_public)+1,FALSE,&bad),NTE_INVALID_PARAMETER);
    expect("import truncated header",import(p1,bob_public,4,FALSE,&bad),NTE_INVALID_PARAMETER);
    expect("import wrong blob type",import(p1,bob_public,sizeof(bob_public),TRUE,&bad),NTE_INVALID_PARAMETER);
    memcpy(blob,bob_public,sizeof(bob_public));((BCRYPT_ECCKEY_BLOB *)blob)->dwMagic=BCRYPT_ECDH_PUBLIC_P384_MAGIC;
    truth("import mismatched curve rejected",import(p1,blob,sizeof(bob_public),FALSE,&bad)!=0);
    memcpy(blob,bob_public,sizeof(bob_public));blob[71]^=1;
    truth("import off-curve rejected",import(p1,blob,sizeof(bob_public),FALSE,&bad)!=0);
    memcpy(blob,alice_private,sizeof(alice_private));memset(blob+72,0,32);
    truth("import scalar zero rejected",import(p1,blob,sizeof(alice_private),TRUE,&bad)!=0);
    memcpy(blob,alice_private,sizeof(alice_private));memset(blob+72,0xff,32);
    truth("import over-order scalar rejected",import(p1,blob,sizeof(alice_private),TRUE,&bad)!=0);
    memcpy(blob,alice_private,sizeof(alice_private));blob[103]=2;
    truth("import inconsistent Q rejected",import(p1,blob,sizeof(alice_private),TRUE,&bad)!=0);
    expect("owner size query",NCryptGetProperty(a,NCRYPT_PROVIDER_HANDLE_PROPERTY,NULL,0,&capacity,0),0);
    for(unsigned int i=0;i<100;i++) {
        status=NCryptGetProperty(a,NCRYPT_PROVIDER_HANDLE_PROPERTY,(BYTE *)&owned,sizeof(owned),&size,0);
        if(status||size!=sizeof(owned)||NCryptFreeObject(owned)) { failures++;break; }
        owned=0;
    }
    truth("repeated owner references survive",NCryptGetProperty(a,NCRYPT_LENGTH_PROPERTY,(BYTE *)&value,sizeof(value),&size,0)==0&&value==256);
    expect("release provider1 before keys",NCryptFreeObject(p1),0);p1=0;
    expect("release provider2 before keys",NCryptFreeObject(p2),0);p2=0;
    NCryptFreeObject(generated);generated=0;NCryptFreeObject(rsa);rsa=0;
    NCryptFreeObject(a);a=0;NCryptFreeObject(b);b=0;NCryptFreeObject(bp);bp=0;NCryptFreeObject(cp);cp=0;
    derivation(secret,"secret after keys/providers freed",BCRYPT_KDF_HASH,BCRYPT_SHA256_ALGORITHM,sha256,sizeof(sha256));
    expect("release surviving secret",NCryptFreeObject(secret),0);secret=0;
    truth("1000 independent disposal cycles",lifetime_cycles());
done:
    if(owned)NCryptFreeObject(owned);if(other)NCryptFreeObject(other);if(secret)NCryptFreeObject(secret);
    if(a)NCryptFreeObject(a);if(b)NCryptFreeObject(b);if(bp)NCryptFreeObject(bp);if(cp)NCryptFreeObject(cp);
    if(generated)NCryptFreeObject(generated);if(rsa)NCryptFreeObject(rsa);if(bad)NCryptFreeObject(bad);
    if(p1)NCryptFreeObject(p1);if(p2)NCryptFreeObject(p2);if(unsupported)NCryptFreeObject(unsupported);
    printf("SUMMARY checks=%u failures=%u\n",checks,failures);
    return failures?1:0;
}
