# Standalone native context — Codex 0.160.0

Source: `openai/codex` tag `rust-v0.160.0`, commit
`a956835d020762cb2b570053af06f643a11c0ecc`,
`codex-rs/models-manager/{models.json,prompt.md}` (Apache-2.0,
see LICENSE.codex). prompt-0.160.0.md is the unmodified native fallback prompt.

The frozen catalog preserves model limits, reasoning options and shell/patch
shapes, but omits model-owned message bundles. The launcher installs the fallback
prompt as `model_messages.instructions_template` for every entry. Each entry
selects `tool_mode=direct`, `multi_agent_version=disabled`, an empty experimental
tool list and `use_responses_lite=false`. Standard Responses requests allow
qualification to inspect the actual input and tool schema. An explicit catalog
suppresses live/bundled catalog fallback for known models; unknown model slugs
retain native fallback metadata. Provider namespaced suffix matching is native.

Native model selectors take precedence over `--disable` feature flags.
In particular, OpenRouter `openai/gpt-6-luna` matches bundled `gpt-6-luna`, whose
selectors otherwise request code-mode-only and multi-agent v2. The native
multi-agent role composer inserts `functions.exec` guidance; it does not imply
that the outer coding session was copied. Disabling only `multi_agent` disables
v1 and cannot suppress the v2 model selector.

The catalog is generated only inside a fresh owner home, outside executor mounts,
and pinned by a command-line override. User model configuration cannot supply
instruction files, catalogs, executors or agent settings. `agents.enabled=false`
and disabled `multi_agent_v2` also fence model overrides on resume. Credential
renewal reuses this exact command/home/catalog and does not reconstruct work.
No personal Codex configuration is changed. This is a standalone launch profile,
not a promise that every listed model/provider has execution qualification.
