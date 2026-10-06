# Standalone native capabilities — Codex 0.160.0

The launcher exports the installed, version-checked Codex binary's own catalog
with `codex debug models --bundled`. This native command reads only bundled
metadata: it skips user configuration, credential discovery and remote refresh.
Semon does not embed or replace any base prompt or model message bundle.

The standalone executor supports direct tools and no multi-agent host. Native
0.160.0 model selectors outrank feature flags, including for provider-namespaced
model names. The launcher therefore changes only these four capability fields:
`tool_mode=direct`, `multi_agent_version=disabled`,
`experimental_supported_tools=[]` and `use_responses_lite=false`.
All other native metadata, including every model's instruction template,
variables, permission messages and tool/role guidance, remains unchanged. Native
Codex chooses the applicable message sections from the effective capabilities.
Unknown model slugs keep native fallback behavior; they are not execution-qualified
by this profile. Standard Responses allows inspection of actual tool schemas.

The resulting catalog is pinned through a command-line override in a fresh,
private owner home outside executor mounts. `standalone-profile.json` records
profile `standalone-direct-v1`, native version, original/effective catalog SHA-256
hashes and the capability overrides. Export or parse failures refuse launch;
there is no fallback to a Semon-authored prompt. No network fetch is needed.

User configuration cannot supply instruction files, catalogs, executors or agent
settings. `agents.enabled=false` and disabled multi-agent/code-mode features fence
unsupported hosts, including on resume. Credential renewal reuses the same
command/home/catalog. This path also serves Hub-managed E2B guests; it does not
alter personal Codex configuration or the outer coding session.

Native source reference: `openai/codex` tag `rust-v0.160.0`, commit
`a956835d020762cb2b570053af06f643a11c0ecc`. The native package owns its prompts;
there are no copied upstream prompt/catalog assets in this directory.
