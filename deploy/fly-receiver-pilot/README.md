# Fly receiver pilot configuration

**Draft only. Deployment is held for the R2 design discussion and owner choices.**
This directory defines a reviewable single-machine ingress and storage template;
it does not provision or deploy a Fly app, machine, volume, DNS name, image, or
credential. Merging it must not trigger deployment, release, cleanup, or deletion.

## Public route allowlist

`Caddyfile` is the only public application listener. The Fly proxy terminates
HTTPS and forwards to Caddy on port 8080. Caddy accepts only the explicit
`PUBLIC_HOST` hostname placeholder in `fly.toml`. Each public route requires both
its listed method and exact path, and forwards to the private receiver at
`127.0.0.1:8735`:

- `POST /v1/pair` for a one-time pairing code exchange.
- `POST /v1/unpair` for authenticated sender revocation.
- `POST /v1/push/append`, `/v1/push/facts`, and `/v1/push/status` for
  authenticated synchronization.
- `HEAD /v1/push/status` for the sender's authenticated status capability probe.

All other public paths and methods return 404, including every Viewer, owner
read, token issuance, and management route. Owner credentials do not change this
filter. Caddy forwards the original Host and discards its access log; neither
component is configured to log request headers or bodies. There is no public
plaintext listener. The receiver binds only to loopback. Its `--public-url` must
be set to the owner-approved public HTTPS origin so generated links and Host
binding use the public origin.

Pairing codes and owner tokens are created locally in the receiver machine's
console using the documented `semon pair --dir /data` and `semon token --dir
/data` commands. Do not put credentials in this repository or a Fly image or
configuration value.

## Unresolved packaging prerequisite

The existing `release-binary.yml` workflow uploads a temporary Linux binary
artifact; it does not publish a container image or combine the receiver with
Caddy. An owner-approved immutable image must package the reviewed Caddy config,
the `semon` binary, and a launcher that starts the receiver on
`127.0.0.1:8735` with the approved `--public-url`, then starts Caddy on port
8080. No image is named or built here. The placeholders in `fly.toml` are
intentionally not deployable until R2 decisions establish the app, image, and
volume names.

Before any pilot, owner decisions must also set the Fly app to exactly one
machine and provision exactly one persistent volume for `/data`. The template
requests one shared CPU and 512 MiB; capacity and volume sizing remain pilot
choices. This configuration has not been exercised against a Fly machine or a
real sender. Route behavior, TLS/Host forwarding, storage persistence, and
recovery still require pilot verification.
