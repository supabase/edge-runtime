import { createPublicKey, X509Certificate } from "node:crypto";
import { Buffer } from "node:buffer";
import { assert, assertEquals, assertThrows } from "jsr:@std/assert";

// Test-only certificates, generated with OpenSSL. The ECDSA chain mirrors the
// one Apple uses to sign App Store JWS payloads: a P-384 root
// (ecdsa-with-SHA384) issues a P-256 intermediate (ecdsa-with-SHA384), which
// issues a P-256 leaf (ecdsa-with-SHA256).
const rootPem = `-----BEGIN CERTIFICATE-----
MIIB1DCCAVugAwIBAgIUHavvpoT+MWdw9a+6gZ3cpMFDCjEwCgYIKoZIzj0EAwMw
KTEnMCUGA1UEAwweZWRnZS1ydW50aW1lLWlzc3VlLTc2MSBSb290IENBMCAXDTI2
MTAyNDE1NTIxOVoYDzIxMjYwOTMwMTU1MjE5WjApMScwJQYDVQQDDB5lZGdlLXJ1
bnRpbWUtaXNzdWUtNzYxIFJvb3QgQ0EwdjAQBgcqhkjOPQIBBgUrgQQAIgNiAAQL
UR5mbV+Mc44XVPNOk7brbLbPy7MYAzFxZp3FWgIFeFpCT3lChio5BO5A6FPBgIMZ
nwjESza+SERgR3v8HDgVl2fhLJGz/maD5zDLn+6PjQw7AjGNBeHQ5cmCb6WZdl6j
QjBAMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0PAQH/BAQDAgEGMB0GA1UdDgQWBBSR
cIqs72WBmGVhKGqpjH2rJwprvDAKBggqhkjOPQQDAwNnADBkAjBWs2XXyZmFMa1f
3JmD+akunsagm2ARQvlpXjE2j9wHFKwhax6EEAr101tV8R7f2Y8CMHBp5e7+kuVG
C5HvgrE8egoQUZxbWoAgo65d/kjJevmg4a2JnHGdZlmYNaxQu+5BHw==
-----END CERTIFICATE-----
`;

const intermediatePem = `-----BEGIN CERTIFICATE-----
MIICOjCCAb+gAwIBAgIBAjAKBggqhkjOPQQDAzApMScwJQYDVQQDDB5lZGdlLXJ1
bnRpbWUtaXNzdWUtNzYxIFJvb3QgQ0EwIBcNMjYxMDI0MTU1MjE5WhgPMjEyNjA5
MzAxNTUyMTlaMDExLzAtBgNVBAMMJmVkZ2UtcnVudGltZS1pc3N1ZS03NjEgSW50
ZXJtZWRpYXRlIENBMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEJI5YW/192Uwh
dP4ekMIUGuig1Mm+WQqrmqG8tBtuy4dld2vu7E8BBeJUab0/n1jSp/Z9CfynqKY4
lH844PM6eqOBzTCByjASBgNVHRMBAf8ECDAGAQH/AgEAMA4GA1UdDwEB/wQEAwIB
BjAdBgNVHQ4EFgQU6O84BfK2JuPVQ46dRP+A9ItE2lkwHwYDVR0jBBgwFoAUkXCK
rO9lgZhlYShqqYx9qycKa7wwZAYIKwYBBQUHAQEEWDBWMCgGCCsGAQUFBzABhhxo
dHRwOi8vb2NzcC5leGFtcGxlLmNvbS9yb290MCoGCCsGAQUFBzAChh5odHRwOi8v
Y2EuZXhhbXBsZS5jb20vcm9vdC5jZXIwCgYIKoZIzj0EAwMDaQAwZgIxALlm4p/N
+ATtlzCaiyelRUNitkNp2kARYd1vinqxt6KA7xgW+iIErluHUkdOoq+PhwIxAOfS
NSadI8cn8Fai4zrYn6oSE2+PVhL+vB8Hk7Z0k/TBpR1v9OGgNKorgxJGHQXPuQ==
-----END CERTIFICATE-----
`;

const leafPem = `-----BEGIN CERTIFICATE-----
MIIB4zCCAYmgAwIBAgIBAzAKBggqhkjOPQQDAjAxMS8wLQYDVQQDDCZlZGdlLXJ1
bnRpbWUtaXNzdWUtNzYxIEludGVybWVkaWF0ZSBDQTAgFw0yNjEwMjQxNTUyMTla
GA8yMTI2MDkzMDE1NTIxOVowJjEkMCIGA1UEAwwbZWRnZS1ydW50aW1lLWlzc3Vl
LTc2MSBMZWFmMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEbiX7BKbzucfJxCSy
X8bu44ili8MV6MKkUh3MCFZcjUGlfQgP8gicM3TogHqNECH0bRmtuk+1aDoJ9wcZ
+YsP8aOBmjCBlzAMBgNVHRMBAf8EAjAAMA4GA1UdDwEB/wQEAwIHgDAdBgNVHQ4E
FgQU86tNwy6nCts4gYJw/hTzeW30UEswHwYDVR0jBBgwFoAU6O84BfK2JuPVQ46d
RP+A9ItE2lkwNwYIKwYBBQUHAQEEKzApMCcGCCsGAQUFBzABhhtodHRwOi8vb2Nz
cC5leGFtcGxlLmNvbS9pbnQwCgYIKoZIzj0EAwIDSAAwRQIgUxLwvhnEiQWBDbhk
/8zj/gQCka09rUvSjFOVeSLSUL0CIQDj3IgygBJMMk8rvGGYK3pVjMTlAVAXb47l
e+yQ9K8MLg==
-----END CERTIFICATE-----
`;

// Self-signed RSA-2048 certificate (sha256WithRSAEncryption).
const rsaPem = `-----BEGIN CERTIFICATE-----
MIIDHDCCAgSgAwIBAgIUZBF63j4g9Te/vv+RxoMQJkGrZQ0wDQYJKoZIhvcNAQEL
BQAwJTEjMCEGA1UEAwwaZWRnZS1ydW50aW1lLWlzc3VlLTc2MSBSU0EwIBcNMjYx
MDI0MTU1MjE5WhgPMjEyNjA5MzAxNTUyMTlaMCUxIzAhBgNVBAMMGmVkZ2UtcnVu
dGltZS1pc3N1ZS03NjEgUlNBMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKC
AQEAvSqgudUY+zUBB9kE0ML0qKr/3HGq6voUmKvwIVcnjpwpgQFFsUY0qLLBf8oa
75/KURJ5joNZpJwDAw4wTJPEmJ1H0rHdIPzHcZJNaJvwuNI2OVPSIMq8ukyJo/eT
pC+bOWw2sdiqFXlxsilK8hQ9JPm/NN8LK7akHj9kDlWXXaaBuu37m5W8FZK3xc6Z
2jld+Y+qbBvdgAudAedFkkZSxdHWF6vWH4MU+9my1rMmpLgG26KN1MamIzsrA1cB
jMHYCiHXMa61aQOm16WgFJ+KgCqhGUPm0PI0JWKUh57ZEreaiRUdCpL5ImmYWWnc
y4ccm29dyhS8IRKeBKrwhpF9TQIDAQABo0IwQDAPBgNVHRMBAf8EBTADAQH/MA4G
A1UdDwEB/wQEAwIChDAdBgNVHQ4EFgQUyHPQV/QDP760D5w9soGlYAxt61IwDQYJ
KoZIhvcNAQELBQADggEBAEwIYc90bLqL3IzHDl2gSDjUXoCc4lahtkO1UzXVrkWW
RJaOIEGNe8tt2zP9mpYokElWDzFy0nPrFDi+hkPL7c2T9b56WVedcOM7goQKKInH
kPhdP0AOx+v/63Imqxu2PodYtKD+ewyKK/aIeZH8LrJJ0SsTpT4rpfVo+KbV6Jd0
R1PqkF5xJlxUJJ86kWfvQcUgtlB2UbTWEdhCMW0LKxqWi5gxTw6xlG3Ch6aR9pnX
Fgjbc3FouYYiy0OaFsviC5JDIobfSK8p8kBN+VdVACpR5abcnifmtl7yZOXmSAqf
kS16rkikD+zVTOd24lRvGU+JheWuc020odi/n2uXK30=
-----END CERTIFICATE-----
`;

// Self-signed Ed25519 certificate and its public key. `publicKey` of an
// Ed25519 certificate is not supported yet, so the key is imported separately.
const ed25519Pem = `-----BEGIN CERTIFICATE-----
MIIBWDCCAQqgAwIBAgIUdZUD41Nvnm5rxlJX2Qec9lkmWLcwBQYDK2VwMCkxJzAl
BgNVBAMMHmVkZ2UtcnVudGltZS1pc3N1ZS03NjEgRWQyNTUxOTAgFw0yNjEwMjQx
NTUyMjhaGA8yMTI2MDkzMDE1NTIyOFowKTEnMCUGA1UEAwweZWRnZS1ydW50aW1l
LWlzc3VlLTc2MSBFZDI1NTE5MCowBQYDK2VwAyEAYJmoHleDwL1r8VCNXHWALbmN
MOpGiHQgoeFjXmHNsB2jQjBAMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0PAQH/BAQD
AgKEMB0GA1UdDgQWBBQ1K1BQZsrkiPxW36VsKOeUiD6hGDAFBgMrZXADQQCpv0UB
LF6neZT2jJFncOa48wHfUF1VMRcpPynGkjRJ9UAxQmgQcaVk+ov4GHfvs+9h6S2x
HzMzIH+0+FVE8LIA
-----END CERTIFICATE-----
`;

const ed25519PublicKeyPem = `-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAYJmoHleDwL1r8VCNXHWALbmNMOpGiHQgoeFjXmHNsB0=
-----END PUBLIC KEY-----
`;

export default {
  fetch() {
    const root = new X509Certificate(rootPem);
    const intermediate = new X509Certificate(intermediatePem);
    const leaf = new X509Certificate(leafPem);
    const rsa = new X509Certificate(rsaPem);
    const ed25519 = new X509Certificate(ed25519Pem);
    const ed25519PublicKey = createPublicKey(ed25519PublicKeyPem);

    // verify(publicKey): was this certificate signed by publicKey?
    assert(root.verify(root.publicKey));
    assert(intermediate.verify(root.publicKey));
    assert(leaf.verify(intermediate.publicKey));
    assert(rsa.verify(rsa.publicKey));
    assert(ed25519.verify(ed25519PublicKey));
    assert(!leaf.verify(root.publicKey));
    assert(!leaf.verify(leaf.publicKey));
    assert(!intermediate.verify(intermediate.publicKey));
    assert(!root.verify(rsa.publicKey));
    assert(!rsa.verify(root.publicKey));
    assert(!ed25519.verify(rsa.publicKey));
    assert(!rsa.verify(ed25519PublicKey));

    // A tampered signature must not verify.
    const tamperedDer = Buffer.from(leaf.raw);
    tamperedDer[tamperedDer.length - 1] ^= 1;
    const tampered = new X509Certificate(tamperedDer);
    assert(!tampered.verify(intermediate.publicKey));

    // checkIssued(otherCert): could otherCert have issued this certificate?
    // Like Node.js (OpenSSL's X509_check_issued), this compares names, key
    // identifiers and key usage, but does not check the signature.
    assert(root.checkIssued(root));
    assert(intermediate.checkIssued(root));
    assert(leaf.checkIssued(intermediate));
    assert(rsa.checkIssued(rsa));
    assert(ed25519.checkIssued(ed25519));
    assert(!leaf.checkIssued(root));
    assert(!leaf.checkIssued(leaf));
    assert(!root.checkIssued(intermediate));
    assert(!intermediate.checkIssued(leaf));
    assert(!rsa.checkIssued(root));
    assert(tampered.checkIssued(intermediate));

    // infoAccess: Authority Information Access, in the Node.js format.
    assertEquals(
      intermediate.infoAccess,
      "OCSP - URI:http://ocsp.example.com/root\n" +
        "CA Issuers - URI:http://ca.example.com/root.cer",
    );
    assertEquals(leaf.infoAccess, "OCSP - URI:http://ocsp.example.com/int");
    assertEquals(root.infoAccess, undefined);
    assertEquals(rsa.infoAccess, undefined);

    // Argument validation matches Node.js.
    assertThrows(
      // deno-lint-ignore no-explicit-any
      () => leaf.verify("not a key" as any),
      TypeError,
    );
    assertThrows(
      // deno-lint-ignore no-explicit-any
      () => leaf.checkIssued({} as any),
      TypeError,
    );

    return new Response(null);
  },
};
