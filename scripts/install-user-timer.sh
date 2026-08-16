#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_dir=$(dirname "$script_dir")
bin_dir=${XDG_BIN_HOME:-"$HOME/.local/bin"}
unit_dir=${XDG_CONFIG_HOME:-"$HOME/.config"}/systemd/user

install -d "$bin_dir" "$unit_dir"
(cd "$project_dir" && cargo build --release --locked --bin semon-codex)
target_dir=${CARGO_TARGET_DIR:-"$project_dir/target"}
case "$target_dir" in
  /*) ;;
  *) target_dir="$project_dir/$target_dir" ;;
esac
install -m 0755 "$target_dir/release/semon-codex" \
  "$bin_dir/semon-codex"
install -m 0644 "$project_dir/systemd/devlog-codex-tailer.service" \
  "$unit_dir/devlog-codex-tailer.service"
install -m 0644 "$project_dir/systemd/devlog-codex-tailer.timer" \
  "$unit_dir/devlog-codex-tailer.timer"

systemctl --user daemon-reload
systemctl --user enable --now devlog-codex-tailer.timer

printf '%s\n' "Installed and started devlog-codex-tailer.timer"
