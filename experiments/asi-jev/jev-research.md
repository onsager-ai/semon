# Jev / TypeSafe System One research

**Evidence retrieved:** 2026-10-29. Official TypeSafe pages and the live OpenAPI schema were fetched directly. No paid Jev request was made by this research task.

## What Jev does

Jev is TypeSafe's System One model: one shared `state` plus named, typed questions yields structured decisions and probability values. It does not provide free-form chat text. Questions in one request are evaluated independently against the same state, and the answer map uses the question IDs as keys. TypeSafe recommends small, focused judgments and combining them in application code. [Introduction](https://docs.typesafe.ai/introduction) · [Primitives](https://docs.typesafe.ai/primitives)

## HTTP API and schema

The current API is `POST https://api.typesafe.ai/v1/systemone`, authenticated with `Authorization: Bearer <token>` and JSON. `GET /v1/models` lists account-available aliases. The required request fields are `model`, `state`, and a nonempty `questions` object. `state` can be a string, JSON object, or array. Each question is named by the caller and carries a `type`; question types can be mixed in the same request. [API reference](https://docs.typesafe.ai/api/system-one) · [live OpenAPI 3.1.0, version 0.2.0](https://api.typesafe.ai/openapi.json)

Schema-valid example (response values are illustrative):

```json
{
  "model": "jev-1.14.0",
  "state": {"message": "I was charged twice. Please refund the extra charge."},
  "questions": {
    "billing": {
      "type": "noul",
      "instructions": "Is this message about billing?"
    },
    "intent": {
      "type": "choice",
      "instructions": "What is the customer's intent?",
      "criteria": {
        "refund": "Requests a refund",
        "complaint": "Reports a problem without asking for a refund",
        "other": "Neither of the above"
      }
    },
    "urgency": {
      "type": "score",
      "instructions": "How urgently does support need to respond?",
      "criteria": ["Can wait", "This week", "Today"]
    }
  }
}
```

The success response is `{ "model", "answers", "usage" }`. `answers` is keyed by the same IDs, with a `type` discriminator matching each question. `usage` contains integer `input_tokens` and `output_tokens`.

```json
{
  "model": "jev-1.14.0",
  "answers": {
    "billing": {"type": "noul", "noul": 0.98},
    "intent": {
      "type": "choice",
      "choice": "refund",
      "confidence": 0.91,
      "probabilities": {"refund": 0.91, "complaint": 0.07, "other": 0.02}
    },
    "urgency": {
      "type": "score",
      "score": 1.7,
      "confidence": 0.55,
      "legend": {"0": "Can wait", "1": "This week", "2": "Today"},
      "probabilities": {"0": 0.1, "1": 0.1, "2": 0.8}
    }
  },
  "usage": {"input_tokens": 123, "output_tokens": 20}
}
```

| Type | Request fields | Response fields |
| --- | --- | --- |
| `noul` | `type`; optional `instructions`; optional `criteria.true` / `criteria.false` | `noul`: probability of yes/true, 0–1 |
| `choice` | `type`; `criteria` object mapping option names to descriptions; optional `instructions` | `choice`: highest-probability option key; `confidence`; `probabilities`: one value per option, approximately summing to 1 |
| `score` | `type`; ordered `criteria` array of levels; optional `instructions` | `score`: probability-weighted expected level, possibly fractional; `confidence`; `legend`; `probabilities` per level, approximately summing to 1 |
| `box` | `type`; image in request; optional `instructions` | normalized `box` edges (`top`, `bottom`, `left`, `right`) and nullable `confidence` |

`choice` criteria accept 1–255 options; OpenAPI accepts 1–10 score levels, while the Score guide recommends at least 2. The API schema requires at least one question and declares no maximum number of questions. The Models page explicitly says any number is allowed while context limits are respected. No question can consume another question's answer: each sees the common state and its own question in isolation. A target-specific prefix in each `instructions` value is therefore valid. The request context includes all question text, and `usage.input_tokens` is the request's billable input count; docs do not state whether identical repeated text is deduplicated. [OpenAPI schema](https://api.typesafe.ai/openapi.json) · [Models and context limits](https://docs.typesafe.ai/models) · [Choice](https://docs.typesafe.ai/primitives/choice) · [Score](https://docs.typesafe.ai/primitives/score)

The API returns JSON over HTTP. The live OpenAPI declares one ordinary JSON `POST /v1/systemone` operation and no streaming operation or `stream` parameter; the API reference has no streaming mode. Treat this as synchronous request/response based on the published contract, while recognizing that undocumented behavior cannot be ruled out. [OpenAPI](https://api.typesafe.ai/openapi.json) · [API reference](https://docs.typesafe.ai/api/system-one)

## Limits and capability notes

- **Context:** Default limits are 64k tokens per request and 32k per question. The request limit covers the state plus all questions; the per-question limit covers the state plus one question. `TypeSafe-Danger-Zone-Long-Context: true` raises both to 262k. TypeSafe warns that long context is slower and less accurate, and that each question adds to the request-wide context. There is no separate documented question-count cap. [Models](https://docs.typesafe.ai/models)
- **Request size:** The Images guide documents a 32 MiB HTTP request-body limit, including JSON, base64, and multipart framing, plus a 32 MiB expanded-request limit after image references are substituted. Its image-specific limits include 16 MiB per image, PNG/JPEG still images, 280–1568 pixels per side, and 78,400–2,458,624 total pixels. The OpenAPI schema does not publish a byte limit for ordinary text fields, so a distinct text-only maximum is not specified there. [Images and limits](https://docs.typesafe.ai/images)
- **Images and boxes:** Send `TypeSafe-Danger-Zone-Images: true` for assets or `box` questions. Images work on 1.13 and 1.14; `box` requires 1.14. TypeSafe labels image and Box support “dreamer-ready” and says they are not yet production-ready to its standards; formats, limits, and accuracy may change. [Images](https://docs.typesafe.ai/images) · [Box](https://docs.typesafe.ai/primitives/box) · [1.14 announcement](https://typesafe.ai/blog/jev-1-14)
- **Language and task fit:** English is the primary training language and currently has best accuracy. Other languages, including CJK, are supported with lower accuracy. The intended use is short, atomic judgments; open-ended analysis should be decomposed and composed in caller code. [Models](https://docs.typesafe.ai/models) · [Primitives](https://docs.typesafe.ai/primitives)
- **Rate limits:** Limits are organization-specific and measured in requests per second and tokens per second. Current numeric limits are shown in the account console; exceeding one returns `429`. Public docs do not state a universal default limit. [Models](https://docs.typesafe.ai/models)

## Versions, pricing, and latency

As of the fetched Models page, `jev-latest` and `jev-preview` both resolve to `jev-1.14.0`; TypeSafe says no separate preview build is currently available. `jev-preview` may move ahead when a preview exists, and aliases can change without code changes. The page says versioned IDs such as `jev-1.14.0` are accepted even if not returned by the model-list endpoint. Pin `jev-1.14.0` when reproducibility matters and record the response's `model` field. [Models](https://docs.typesafe.ai/models)

TypeSafe lists Jev 1.14 at **$42 per billion input tokens ($0.042 per million)**; its table gives Jev 1.13 the same rate. Output tokens are returned in `usage.output_tokens` but are currently free. Long-context requests have the same per-token price. The documented price is for TypeSafe direct API use; the fetched Magpie model listing contained no Jev entry, so it provides no independent Jev gateway price. [Models](https://docs.typesafe.ai/models) · [API response schema](https://api.typesafe.ai/openapi.json)

TypeSafe gives qualitative latency guidance (“adding questions barely changes response time”) but no numeric latency SLA or percentile in the docs. The Manager reported one Magpie-routed smoke using `/v1/systemone` and `typesafe/jev-latest` on 2026-10-29: it resolved to Jev 1.14.0 and took **1.14 s**, with `usage` of **371 input / 38 output tokens**. This includes gateway/network effects and is a single request observation, not a direct TypeSafe measurement, benchmark, or guarantee. [Introduction](https://docs.typesafe.ai/introduction)

## Privacy, retention, and residency

TypeSafe states it does not train on customer requests or responses. The Help Center says inputs may still be processed to provide the service, calculate charges, prevent fraud and abuse, and generate telemetry; telemetry may be used to improve TypeSafe services. Retention and deletion depend on the data, agreement, law, and any agreed ZDR arrangement. [Submitted-data FAQ, last updated 2026-10-11](https://help.typesafe.ai/articles/5974925716-how-is-submitted-data-used-or-retained)

The public Privacy Policy says the Services are hosted in the United States, with personal data from other regions transferred to the U.S. for storage and processing. It states personal data is retained as reasonably necessary for service or business purposes, without a fixed duration. TypeSafe's Master Customer Agreement permits processing Customer Data to derive telemetry, monitor fraud/abuse, and comply with law “in perpetuity”; it also says TypeSafe has no obligation to store or retain Customer Data and may delete it at any time, while confidential information may remain in standard backups. The DPA describes retention as as long as necessary for processing purposes and legal compliance. These documents do not establish a fixed request/response retention period. [Privacy Policy](https://typesafe.ai/legal/privacy-policy) · [Master Customer Agreement](https://typesafe.ai/legal/mca) · [DPA](https://typesafe.ai/legal/data-processing)

Zero Data Retention is offered to enterprise customers, but public documentation does not specify the exact configuration or retention behavior for a particular account. There is no public regional processing selector in the docs reviewed. Confirm the applicable DPA, ZDR setup, and residency commitments with TypeSafe for sensitive data. [Legal documentation](https://docs.typesafe.ai/legal) · [Enterprise](https://typesafe.ai/enterprise)

## Evidence record

Primary-source pages above were fetched on 2026-10-29. `https://api.typesafe.ai/openapi.json` returned 22,467 bytes and identified itself as OpenAPI 3.1.0, API version 0.2.0. The Models page and current docs are live and can change; in particular, aliases, pricing, feature maturity, and limits should be rechecked before production decisions. The Jev 1.14 announcement is dated 2026-10-14; the Privacy Policy shows last updated 2025-11-19; the DPA shows last updated 2026-04-24; the submitted-data FAQ shows last updated 2026-10-11.
