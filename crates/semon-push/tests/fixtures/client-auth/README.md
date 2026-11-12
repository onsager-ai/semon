# Test-only client certificates

These public test keys and certificates are exclusively for deterministic listener tests. Never use them for deployment. Generated once with OpenSSL 3, using P-256 keys, on 2026-11-12; validity is 36500 days. Tests do not invoke OpenSSL. No expired fixture is included; registry validity boundaries use injected Unix seconds.

Generation commands (temporary CA key and CSR are not committed):

```sh
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout /tmp/semon-listener-ca-key.pem -out ca.pem -days 36500 -subj '/CN=Semon Test Client CA'
openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout client-key.pem -out /tmp/semon-listener-client.csr -subj '/CN=Semon Test Client'
openssl x509 -req -in /tmp/semon-listener-client.csr -CA ca.pem -CAkey /tmp/semon-listener-ca-key.pem -set_serial 2 -out client.pem -days 36500
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout foreign-key.pem -out foreign.pem -days 36500 -subj '/CN=Semon Foreign Test CA'
openssl x509 -in client.pem -pubkey -noout -out /tmp/semon-listener-public.pem
openssl pkey -pubin -in /tmp/semon-listener-public.pem -outform DER -out /tmp/semon-listener-spki.der
openssl dgst -sha256 /tmp/semon-listener-spki.der
```

The committed client SPKI SHA-256 is `b8699d8cd2e33c9b795b1c59868fd78cdc5423ab0600362882d6ee8125e26608`.

Files: `ca.pem` (trusted client CA), `client.pem`/`client-key.pem` (signed client), `foreign.pem`/`foreign-key.pem` (untrusted self-signed identity). The existing parent-directory server certificate remains the server fixture.
