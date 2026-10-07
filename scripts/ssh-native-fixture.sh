#!/bin/sh
# Explicit local fixture for native controller qualification, never a user server.
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
: "${SEMON_CODEX_PACKAGE:?complete pinned native package required}"
: "${SEMON_SSH_BINARY:?qualified Semon push/readback binary required}"
[ "$(id -u)" -ne 0 ] || { echo "native fixture requires an unprivileged invoking user" >&2; exit 1; }
fixture=$(mktemp -d)
name="semon-ssh-native-$$"
cleanup() {
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker image rm "$name" >/dev/null 2>&1 || true
    rm -rf "$fixture"
}
trap cleanup EXIT HUP INT TERM
ssh-keygen -q -t ed25519 -N '' -f "$fixture/client"
cp "$fixture/client.pub" "$fixture/authorized_keys"
sed "s/useradd -m/useradd -u $(id -u) -m/" "$root/tests/ssh/Dockerfile" > "$fixture/Dockerfile"
# Fixture-only unprivileged user namespaces; no alternate controller isolation.
printf '\nRUN apt-get update && apt-get install -y --no-install-recommends bubblewrap && rm -rf /var/lib/apt/lists/*\n' >> "$fixture/Dockerfile"
docker build -q -t "$name" "$fixture"
docker run --rm -d --name "$name" --security-opt seccomp=unconfined --security-opt apparmor=unconfined --security-opt systempaths=unconfined --mount "type=bind,src=$SEMON_CODEX_PACKAGE,dst=/opt/semon-codex,readonly" --mount "type=bind,src=$SEMON_SSH_BINARY,dst=/opt/semon-mirror,readonly" -p 127.0.0.1::22 "$name" >/dev/null
address=$(docker port "$name" 22/tcp)
SEMON_SSH_KEY_FILE="$fixture/client"
SEMON_SSH_PORT=${address##*:}
SEMON_SSH_FIXTURE_CONTAINER="$name"
export SEMON_SSH_KEY_FILE SEMON_SSH_PORT SEMON_SSH_FIXTURE_CONTAINER
i=0
while ! ssh-keyscan -T 1 -t ed25519 -p "$SEMON_SSH_PORT" 127.0.0.1 >/dev/null 2>&1; do
    i=$((i+1))
    [ "$i" -lt 20 ] || exit 1
    sleep .2
done
"$@"
