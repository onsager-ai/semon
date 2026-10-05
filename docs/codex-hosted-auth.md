# Codex authentication prerequisites (#249 / #252)

## Current official route — October 4, 2026

The selected targets remain user-provided API keys and personal ChatGPT sign-in;
a user chooses one billing/authentication source. Working first-party Codex device
login is not evidence of permission for a commercial hosted integration.

The official **[Sign in with ChatGPT](https://developers.openai.com/siwc)**
[quickstart](https://developers.openai.com/siwc/quickstart) currently limits commercial
access to selected partners in a trial.
[Client-ID registration](https://developers.openai.com/siwc/request-client-id) directs
commercial applicants to the interest form. The
[OSS token-sharing route](https://developers.openai.com/siwc/token-sharing-open-source)
applies to open-source and locally hosted apps; paid or remotely hosted apps must
complete the interest form. Plus/Pro entitlement and workspace permission still
need validation. Documentation is current research, not a granted client ID or an
authenticated inference test.

The documented OSS flow uses PKCE, fresh state/nonce, a loopback callback, stable
ext_agent_host_id and initial dynamic_agent_client registration; retain the issued
oaiapp_clientid, validate token issuer/audience/nonce/expiry and granted scopes.
Do not reuse the first-party device-flow client ID as a commercial OAuth client.
The [app-server integration](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)
uses a normal provider env_key for ACCESS_TOKEN, the Responses API and externally
owned refresh; it does not require internal chatgptAuthTokens. Its current recipe
uses HTTP streaming, store:false and supports_websockets=false. Restart with renewed
credentials and resume the native thread as documented. That is an authentication
recipe, not permission to replay uncertain control writes. See also
[self-hosted VMs](https://developers.openai.com/siwc/token-sharing-open-source/self-hosted-vms)
and [preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations).

Hosted eligibility remains unqualified: obtain/verify the approved commercial
client route, then independently test API-key and personal sign-in model access,
refresh/revocation and secure custody. A successful device login or bundled model
catalog cannot substitute for those tests. No auth source is silently dropped or
substituted. Local control uses a fresh model-configured home and makes no hosted
eligibility claim; personal sign-in onboarding is not added by that slice.

## Retained alpha protocol evidence

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
managed refresh. That historical onboarding plan kept exec/resume for tasks.
The demonstrated interactive requirement now uses the single app-server driver
in [the current design](design/two-way.md); batch probes/history remain separate.

## Managed refresh boundary

The optional process driver also supports `refresh_chatgpt`: it runs only the
pinned public `account/read {refreshToken: true}` route in an embedding-supplied
isolated home. The exact generated `GetAccountParams` schema describes this as
proactive normal managed refresh. No login, model request, token extraction or
internal `chatgptAuthTokens` API is used. Account type and OpenAI authentication
requirements must still match ChatGPT. The process is killed/reaped before the
embedding reads the official opaque managed artifact, including on failure.

The embedding must durably claim the sole refresh writer before starting Codex,
commit the updated encrypted artifact before delivery, and reject stale results
following reconnect/disconnect or runtime authority changes. A lost result may
have rotated the old artifact; require explicit reconnect instead of replay.
Offline rotation fixtures establish local protocol/custody behavior only.

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
qualify token lifecycle. First-party CLI personal users can use device code; they do not need
workspace-token entitlement. Device auth may require personal security settings
or workspace permission. See [official authentication](https://developers.openai.com/codex/auth/)
and [app-server documentation](https://developers.openai.com/codex/app-server/).

Hub #85/#86 must preserve the task and chosen method through reconnect failures,
resolve credentials only for their owner, and recheck authorization/usability
before launch. Automatic pause remains Off. Reconstruction/deletion, independent
recovery and automatic policies remain gated on #255/#256 and real hosted
acceptance. This subset adds no provider calls, hosted route, policy activation,
deployment or change to billing.
