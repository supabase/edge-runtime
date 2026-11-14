import { X509Certificate } from "node:crypto";
import { Buffer } from "node:buffer";
import { assert, assertEquals } from "jsr:@std/assert";

// Self-signed P-256 certificate used only for this test.
const certPem = `-----BEGIN CERTIFICATE-----
MIIBmDCCAT+gAwIBAgIUZvFME6lS+gCRp2TW6rejL0xPkBowCgYIKoZIzj0EAwIw
ITEfMB0GA1UEAwwWZWRnZS1ydW50aW1lLWlzc3VlLTc1MDAgFw0yNjEwMjIyMTM2
MzBaGA8yMTI2MDkyODIxMzYzMFowITEfMB0GA1UEAwwWZWRnZS1ydW50aW1lLWlz
c3VlLTc1MDBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABPKODS4qPOVJwIian74M
o4OmDUZbLWkJhnHGrf1ER77xKIWk+WcE+JAeoSDtI5u9sBZ8TbrGuMP9n1CRtNf9
F96jUzBRMB0GA1UdDgQWBBT8U051422VypWE2dR54oprbN6zdDAfBgNVHSMEGDAW
gBT8U051422VypWE2dR54oprbN6zdDAPBgNVHRMBAf8EBTADAQH/MAoGCCqGSM49
BAMCA0cAMEQCIAqC893782NwmrA5wqYLjeVF/zmzPWv1ryRSbv66ga2oAiBCabb0
pkV3tLijLh+GE/uEm0yLll/wfpLVtGemROGMPA==
-----END CERTIFICATE-----
`;

const certDer = Buffer.from(
  certPem
    .replace("-----BEGIN CERTIFICATE-----", "")
    .replace("-----END CERTIFICATE-----", "")
    .replace(/\s+/g, ""),
  "base64",
);

export default {
  fetch() {
    // PEM input
    const fromPem = new X509Certificate(certPem);
    const raw = fromPem.raw;
    assert(Buffer.isBuffer(raw));
    assertEquals(raw.length, 412);
    assert(raw.equals(certDer));
    assertEquals(fromPem.toString(), certPem);
    assertEquals(fromPem.toJSON(), certPem);

    // DER input
    const fromDer = new X509Certificate(certDer);
    assert(fromDer.raw.equals(certDer));
    assertEquals(fromDer.toString(), certPem);

    return new Response(null);
  },
};
