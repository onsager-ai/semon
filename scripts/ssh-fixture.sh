#!/bin/sh
# Run a qualification command against an isolated real OpenSSH server.
# Only synthetic keys are created. No external TCP connection is needed.
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
fixture=$(mktemp -d)
name="semon-ssh-fixture-$$"
cleanup() {
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker image rm "$name" >/dev/null 2>&1 || true
    rm -rf "$fixture"
}
trap cleanup EXIT HUP INT TERM
ssh-keygen -q -t ed25519 -N '' -f "$fixture/client"
cp "$fixture/client.pub" "$fixture/authorized_keys"
cp "$root/tests/ssh/Dockerfile" "$fixture/Dockerfile"
docker build -q -t "$name" "$fixture"
docker run --rm -d --name "$name" -p 127.0.0.1::22 "$name" >/dev/null
address=$(docker port "$name" 22/tcp)
SEMON_SSH_KEY_FILE="$fixture/client"
SEMON_SSH_PORT=${address##*:}
export SEMON_SSH_KEY_FILE SEMON_SSH_PORT
# Poll the local fixture's readiness; never disable host verification for auth.
i=0
while ! ssh-keyscan -T 1 -t ed25519 -p "$SEMON_SSH_PORT" 127.0.0.1 >/dev/null 2>&1; do
    i=$((i+1))
    [ "$i" -lt 20 ] || exit 1
    sleep 0.2
done
"$@"
