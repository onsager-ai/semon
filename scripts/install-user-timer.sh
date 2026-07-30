#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_dir=$(dirname "$script_dir")
bin_dir=${XDG_BIN_HOME:-"$HOME/.local/bin"}
unit_dir=${XDG_CONFIG_HOME:-"$HOME/.config"}/systemd/user

install -d "$bin_dir" "$unit_dir"
install -m 0755 "$project_dir/scripts/codex_tailer.py" \
  "$bin_dir/devlog-codex-tailer"
install -m 0644 "$project_dir/systemd/devlog-codex-tailer.service" \
  "$unit_dir/devlog-codex-tailer.service"
install -m 0644 "$project_dir/systemd/devlog-codex-tailer.timer" \
  "$unit_dir/devlog-codex-tailer.timer"

systemctl --user daemon-reload
systemctl --user enable --now devlog-codex-tailer.timer

printf '%s\n' "Installed and started devlog-codex-tailer.timer"
