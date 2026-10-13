# Identity authentication observability

## Purpose

This runbook defines the provider-neutral authentication telemetry contract for `memo_server` during identity rollout and steady-state operation.

It complements `docs/authentication.md`. The application must make authentication regressions visible without leaking bearer credentials, user identity, signing-key identifiers, provider details, or internal topology.

## Authentication runtime event

Every authentication extraction is classified exactly once after header parsing and mode-specific authentication complete. The application keeps process-local monotonic counters and emits one aggregate INFO event every 100 authentication attempts.

The event shape is stable and low-cardinality:

```text
auth verification aggregate: last_result=<category> total=<n> success=<n> unauthorized=<n> dependency_unavailable=<n> internal_error=<n>
```

`last_result` is one of exactly:

| Category | Meaning |
| --- | --- |
| `success` | The request authentication boundary returned an authenticated identity. |
| `unauthorized` | The boundary rejected the request as unauthenticated/invalid, including malformed credential transport or invalid token verification. |
| `dependency_unavailable` | Authentication could not complete because the verification-key/dependency path was unavailable. |
| `internal_error` | The authentication extraction failed for another server-side reason. |

Classification is based on the stable `AppError` variant only. Error-message text is never promoted into a telemetry label or category.

## JWKS refresh runtime event

Actual JWKS refresh attempts and refresh suppressions use a second fixed event:

```text
auth jwks refresh: trigger=<trigger> outcome=<outcome> total=<n> scheduled=<n> unknown_kid=<n> invalid_signature=<n> success=<n> stale_fallback=<n> throttled=<n> unavailable=<n> invalid_response=<n>
```

The trigger set is fixed:

| Trigger | Meaning |
| --- | --- |
| `scheduled` | Normal non-forced refresh after initial cache miss or cache expiry. It does not mean a background timer exists. |
| `unknown_kid` | A bounded forced refresh because the current key set did not contain the requested `kid`. The `kid` value itself is never logged. |
| `invalid_signature` | A bounded forced refresh after signature verification failed with the selected key, allowing safe key rotation recovery. |

The outcome set is fixed:

| Outcome | Log level | Meaning |
| --- | --- | --- |
| `success` | INFO | A JWKS fetch completed and the validated cache snapshot was replaced. |
| `stale_fallback` | WARN | A normal refresh failed but an eligible bounded stale snapshot was retained. |
| `throttled` | DEBUG | A forced refresh was suppressed by the existing cooldown; cached keys are used when available. |
| `unavailable` | WARN | Transport, upstream status, or response-read failure prevented a usable JWKS response. |
| `invalid_response` | WARN | The response was oversized, malformed, or failed the configured JWKS/key-set validation contract. |

Fresh cache hits do not produce JWKS refresh events. The event observes the existing refresh path rather than turning authentication requests into per-request telemetry.

The counters are process-local. Trigger counts and outcome counts are operational aggregates only; they are not billing, audit, or security-decision state.

## Privacy and cardinality boundary

Authentication and JWKS events MUST NOT contain:

- raw access tokens or refresh/session credentials;
- authorization codes or browser session cookies;
- JWT payloads or decoded claims;
- `sub`, user UUIDs, email addresses, or other account identifiers;
- `jti`;
- raw `kid` values;
- `client_id`;
- issuer/provider/product names;
- JWKS URLs or upstream hostnames;
- internal repositories, database identifiers, or topology;
- raw HTTP/transport/JSON/provider error text or response payloads.

All telemetry dimensions come from fixed enums. Do not add dynamic error strings, HTTP paths/statuses, user identifiers, key identifiers, issuer names, or URLs as labels.

The verifier deliberately maps transport and response details into `unavailable` or `invalid_response` instead of interpolating the underlying `reqwest`, JSON, URL, or upstream status diagnostics into logs.

## Aggregation semantics

Counters are process-local and reset when a process restarts. Each replica has an independent counter set.

The counters use relaxed atomics because they are operational telemetry, not billing, authorization, audit, or transactional accounting. A concurrently sampled aggregate line can therefore be a near-current operational snapshot rather than a globally atomic transaction.

For fleet-level monitoring, central log aggregation must group these events by deployment metadata already supplied by the runtime/logging platform, rather than adding application-level provider or identity labels.

No public metrics endpoint and no new runtime metrics dependency are introduced by this contract.

## Rollout monitoring

During an identity rollout, watch at least:

1. successful authentication throughput relative to the pre-rollout baseline;
2. changes in the `unauthorized` ratio relative to the same traffic pattern;
3. any sustained `dependency_unavailable` or `internal_error` observations;
4. JWKS `unavailable` and `invalid_response` warnings;
5. JWKS `stale_fallback` usage and whether it clears after upstream recovery;
6. unexpected growth in `unknown_kid` or `invalid_signature` refresh triggers during signing-key cutover;
7. browser login/refresh/logout and API-proxy acceptance checks from the rollout plan.

A universal percentage threshold is intentionally not hard-coded here. Traffic shape differs by environment, and invalid unauthenticated traffic may be normal. Establish an environment baseline and alert on material deviation during rollout.

A rise in `unauthorized` can indicate token profile, audience, issuer, algorithm/type policy, credential transport, or expiry problems, but the aggregate event deliberately does not expose which token or user triggered it. Use access-controlled server diagnostics and the provider-neutral preflight tools for investigation.

`dependency_unavailable`, `unavailable`, and `invalid_response` are higher-severity during rollout because they indicate that authentication could not complete through a healthy verification-key path rather than simply rejecting an invalid credential.

An isolated `unknown_kid` or `invalid_signature` trigger can be expected around a controlled key rotation. Sustained growth after the overlap should be treated as a rollout signal and correlated with the signing-key runbook without logging the concrete key identifier.

## Failure behavior

Observability is passive. Authentication outcome recording and JWKS refresh recording both return the original `AppResult` unchanged.

Telemetry must never:

- convert an authentication or JWKS failure into success;
- change the public HTTP error category;
- retry authentication beyond the existing verifier behavior;
- bypass JWT/JWKS verification;
- widen cache freshness, stale-if-error, or forced-refresh cooldown windows;
- create a dependency on the log pipeline for request success.

If logging is filtered or unavailable, authentication semantics remain unchanged.

## Production acceptance

Repository tests prove that:

- authentication and JWKS taxonomies are fixed and do not derive labels from error text;
- aggregate counters track the fixed categories;
- observing an `AppResult` preserves the original result;
- normal authenticated request extraction passes through the authentication observation hook;
- normal, unknown-`kid`, invalid-signature, stale-fallback, throttled, unavailable, and invalid-response JWKS paths are classified without dynamic upstream details;
- format and Clippy gates cover the implementation.

They do **not** prove that a deployed log collector, dashboard, or alerting route is working.

Before production cutover:

1. confirm authentication and JWKS events arrive in the intended private operational log pipeline;
2. verify dashboards/queries distinguish only the documented fixed categories;
3. confirm no token, subject, `jti`, raw `kid`, issuer/JWKS URL, provider name, or internal identity implementation detail appears in sampled events;
4. establish the pre-cutover authentication and JWKS baseline;
5. exercise an invalid-token case and confirm `unauthorized` changes without exposing credential detail;
6. exercise controlled JWKS unavailability in staging and confirm the expected `stale_fallback` or `unavailable` outcome;
7. exercise a signing-key overlap so `unknown_kid`/`invalid_signature` recovery can be observed without revealing the concrete key;
8. verify alert routing for sustained dependency/JWKS/internal failures;
9. retain the real-token, browser-flow, and signing-key overlap/rollback checks required by issue #13.

Successful repository CI is necessary application evidence, not a substitute for this deployment rehearsal.
