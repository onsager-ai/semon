# Test-only client certificates

These public test keys and certificates are exclusively for deterministic listener tests. Never use them for deployment. Certificates are X.509 v3, generated once with OpenSSL 3 and P-256 keys. Every certificate is valid from 2026-01-01 through 2126-01-01, so generation time cannot put notBefore ahead of the CI clock. Tests do not invoke OpenSSL. No expired fixture is included; registry validity boundaries use injected Unix seconds.

The committed `openssl.cnf` defines critical CA/basic-constraint and key-usage extensions, clientAuth extended usage, and subject/authority key identifiers. The following exact recipe runs from this directory, reusing the committed client private keys to preserve SPKI pins. CA signing keys, seed certificates, CSRs and database remain temporary and are not committed. The client keys were originally made with `openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out client-key.pem` (and the same command for `foreign-key.pem`).

```sh
export FIXTURE_CA_DIR="$(mktemp -d /tmp/semon-v3-fixtures-XXXXXX)"
touch "$FIXTURE_CA_DIR/index.txt"
printf '1000\n' > "$FIXTURE_CA_DIR/serial"

make_chain() {
    prefix="$1"
    subject="$2"
    leaf="$3"
    key="$4"
    client_subject="$5"
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$FIXTURE_CA_DIR/$prefix-key.pem"
    openssl req -new -key "$FIXTURE_CA_DIR/$prefix-key.pem" -out "$FIXTURE_CA_DIR/$prefix.csr" -subj "/CN=$subject"
    openssl req -new -x509 -key "$FIXTURE_CA_DIR/$prefix-key.pem" -out "$FIXTURE_CA_DIR/$prefix-seed.pem" -subj "/CN=$subject" -days 36500
    openssl ca -batch -config openssl.cnf -selfsign -keyfile "$FIXTURE_CA_DIR/$prefix-key.pem" -cert "$FIXTURE_CA_DIR/$prefix-seed.pem" -in "$FIXTURE_CA_DIR/$prefix.csr" -out "$prefix.pem" -startdate 20260101000000Z -enddate 21260101000000Z -extensions ca_extensions -notext
    openssl req -new -key "$key" -out "$FIXTURE_CA_DIR/$leaf.csr" -subj "/CN=$client_subject"
    openssl ca -batch -config openssl.cnf -keyfile "$FIXTURE_CA_DIR/$prefix-key.pem" -cert "$prefix.pem" -in "$FIXTURE_CA_DIR/$leaf.csr" -out "$leaf" -startdate 20260101000000Z -enddate 21260101000000Z -extensions client_extensions -notext
}
make_chain ca 'Semon Test Client CA' client.pem client-key.pem 'Semon Test Client'
make_chain foreign-ca 'Semon Foreign Test CA' foreign.pem foreign-key.pem 'Semon Foreign Test Client'

openssl verify -CAfile ca.pem -purpose sslclient client.pem
openssl verify -CAfile foreign-ca.pem -purpose sslclient foreign.pem
for cert in ca.pem client.pem foreign-ca.pem foreign.pem; do
    openssl x509 -in "$cert" -noout -dates
    openssl x509 -in "$cert" -noout -text
done
openssl x509 -in client.pem -pubkey -noout | openssl pkey -pubin -outform DER | openssl dgst -sha256
```

The committed client SPKI SHA-256 remains `b8699d8cd2e33c9b795b1c59868fd78cdc5423ab0600362882d6ee8125e26608`. Tests derive leaf pins from DER and do not hard-code leaf fingerprints.

Files: `ca.pem` (trusted client CA), `client.pem`/`client-key.pem` (signed client), `foreign-ca.pem` (untrusted CA), `foreign.pem`/`foreign-key.pem` (foreign signed client), and `openssl.cnf` (generation extensions). The existing parent-directory server certificate remains the server fixture.
