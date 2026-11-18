// A self-signed X.509 v1 certificate. rustls refuses to use it as a client
// certificate (`UnsupportedCertVersion`).
const CERT = `-----BEGIN CERTIFICATE-----
MIIBFjCBvwIJAP2WWNr7BptLMAkGByqGSM49BAEwFDESMBAGA1UEAwwJbG9jYWxo
b3N0MCAXDTI2MTEwNTIyNDMxNloYDzIxMjYxMDEyMjI0MzE2WjAUMRIwEAYDVQQD
DAlsb2NhbGhvc3QwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAAQFwoaAjxHlVcdb
ldJiloVbH0t//t14WObmbKCV/AuNnlGh3d64iO9+qiwxGJBEffVpBM3llX5MUtTW
4m0AH+zNMAkGByqGSM49BAEDRwAwRAIgFvxVjc1BpKCRVvpTrMZEelFLvfiXHxUM
CzbJ9r0Ie8YCICTI+T/dZwBj1Bdylxn/pH+KBZPVVDg5SLtYSTRzk//u
-----END CERTIFICATE-----`;

const KEY = `-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgvM7KyQdnMGspcpPv
ZbwkWkccC1wG1tThPRBA19JHnVWhRANCAAQFwoaAjxHlVcdbldJiloVbH0t//t14
WObmbKCV/AuNnlGh3d64iO9+qiwxGJBEffVpBM3llX5MUtTW4m0AH+zN
-----END PRIVATE KEY-----`;

async function errorName(fn: () => unknown) {
  try {
    await fn();
  } catch (e) {
    return e instanceof Error ? e.name : String(e);
  }
  return null;
}

Deno.serve(async (req) => {
  const port = parseInt(req.headers.get("x-port") ?? "");

  return Response.json({
    createHttpClient: await errorName(() =>
      Deno.createHttpClient({ cert: CERT, key: KEY })
    ),
    createHttpClientWithResolver: await errorName(() =>
      Deno.createHttpClient({
        // deno-lint-ignore no-explicit-any
        [Symbol.for("unstableSniResolver")]: () => {},
      } as any)
    ),
    connectTls: await errorName(() =>
      Deno.connectTls({ hostname: "127.0.0.1", port, cert: CERT, key: KEY })
    ),
  });
});
