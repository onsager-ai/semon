# Pinned hosted Codex authentication prerequisites (#249 / #252)

Measured 2026-10-03 against unmodified `@openai/codex@0.159.0-alpha.3`.
This is local protocol evidence, not authenticated hosted-use qualification.
The earlier [E2B experiment](codex-e2b-results.md) remains the retained-filesystem
personal-auth evidence; it is not repeated or promoted into a Hub durability claim.

## Protocol evidence

`codex login --help` exposes `--with-api-key`, `--device-auth` and
`--with-access-token` in this exact version. Its generated app-server v2 schema
contains `account/login/start` parameters `apiKey` and `chatgptDeviceCode`.
Device-code responses contain `verificationUrl`, `userCode` and `loginId`;
`account/login/completed` contains the login correlation ID and success flag.
An isolated empty-home app-server accepted initialize/initialized and
`account/read` with `refreshToken: false`, returning `account: null` and
`requiresOpenaiAuth: true`. No login, credentials, model task, E2B sandbox or
paid API call was used for this smoke.

The generated schema explicitly describes `chatgptAuthTokens` as
**[UNSTABLE] FOR OPENAI INTERNAL USE ONLY - DO NOT USE**. Its presence is not
an approved third-party token acquisition or refresh route. It is excluded from
the implementation. Browser cookies and extracted `auth.json` tokens are not
onboarding inputs. Ordinary `chatgpt` browser login with a localhost callback
is also excluded from this headless device-code exchange.

Reproduce the read-only check using
`python3 tests/spikes/codex-auth-protocol.py /absolute/path/to/pinned/codex`.
This command is developer qualification tooling, never a user onboarding step.

| Generated schema | SHA-256 |
| --- | --- |
| LoginAccountParams.json | `48fb9bea54d7e0890052653ac06737efb3200ba2261515b2c4014027f36bea3d` |
| LoginAccountResponse.json | `72e6d77f49bc5809c3af50f183e2a0862dd8f7836816d4796ff69bb13eae0e89` |
| AccountLoginCompletedNotification.json | `d81e8821fd5a4a9ad118dfc3506ea156d32dcb18da388b336501bbc6a5ccb4a6` |

## Implemented boundary

`semon_runtime::codex_auth` supplies the pinned public wire subset and an
ephemeral exchange state machine. The caller chooses API-key or device-code
auth once. Unexpected start/account types fail with a billing-method mismatch;
there is no fallback. Only the exact official device verification URL is
accepted. An unrelated, missing-ID, cancelled or late completion cannot connect
the exchange. A matching success still needs account readback in the isolated
home. `AccountPresent` means a local credential is present, not validated model
access or production readiness. Secret-bearing requests and challenges redact
Debug output; vendor error strings are discarded. These ephemeral types are
not serialized into lifecycle records.

The embedding owns protected transport, request correlation, timeout/cancel,
owner/workspace authorization, connection persistence and secure credential-home
custody. It must isolate the home and selected billing source from inherited
credentials/configuration; the module cannot prove that deployment boundary.
It commits/claims the exchange before starting login and does not restart an
uncertain exchange blindly. A pending cancellation must use
`account/login/cancel` with the retained loginId or terminate the isolated
process before dropping custody. Codex itself performs device polling and
managed refresh. Keep exec/resume as the task driver; using app-server for
onboarding does not require replacing it.

## Hosted qualification remains blocked

This execution environment has no configured E2B or OpenAI credential, no
subscription device approval, and no Business/Enterprise entitlement. It cannot
independently exercise the two hosted billing sources or account token policy.
Local protocol support must not enable hosted launch capability flags.

| Qualification | API key | Personal device-code sign-in |
| --- | --- | --- |
| Pinned public login interface | Present | Present |
| Hosted login and model access | Not run | Not run |
| Hub owner-only custody and managed enrollment | Not implemented | Not implemented |
| Retained filesystem cold boot | Not run | Earlier isolated personal experiment only |
| Expiry/refresh, rotation/revocation | Not run | Not run |
| Secure replacement/reinjection | Not run | Not run |
| Real authenticated Hub ingestion/readback and ACK durability | Not run | Not run |

Business/Enterprise Codex access tokens are a separate eligible option under
[the dedicated official token documentation](https://developers.openai.com/codex/enterprise/access-tokens/),
subject to workspace permissions and actual entitlement. The pinned CLI exposes
token entry, but this app-server subset does not invent a token wire method or
qualify token lifecycle. Personal users follow device code; they do not need
workspace-token entitlement. Device auth may require personal security settings
or workspace permission. See [official authentication](https://developers.openai.com/codex/auth/)
and [app-server documentation](https://developers.openai.com/codex/app-server/).

Hub #85/#86 must preserve the task and chosen method through reconnect failures,
resolve credentials only for their owner, and recheck authorization/usability
before launch. Automatic pause remains Off. Reconstruction/deletion, independent
recovery and automatic policies remain gated on #255/#256 and real hosted
acceptance. This subset adds no provider calls, hosted route, policy activation,
deployment or change to billing.
