# Identity authentication observability

## Purpose

This runbook defines the provider-neutral authentication telemetry contract for `memo_server` during identity rollout and steady-state operation.

It complements `docs/authentication.md`. The application must make authentication regressions visible without leaking bearer credentials, user identity, signing-key identifiers, provider details, or internal topology.

## Runtime event

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

## Privacy and cardinality boundary

The aggregate event MUST NOT contain:

- raw access tokens or refresh/session credentials;
- authorization codes or browser session cookies;
- JWT payloads or decoded claims;
- `sub`, user UUIDs, email addresses, or other account identifiers;
- `jti`;
- `kid`;
- `client_id`;
- issuer/provider/product names;
- internal hostnames, repositories, database identifiers, or topology;
- raw upstream/provider error payloads.

The four result categories are intentionally fixed. Do not add dynamic error strings, HTTP paths, user identifiers, key identifiers, or issuer names as labels.

## Aggregation semantics

Counters are process-local and reset when a process restarts. Each replica has an independent counter set.

The counters use relaxed atomics because they are operational telemetry, not billing, authorization, audit, or transactional accounting. A concurrently sampled aggregate line can therefore be a near-current operational snapshot rather than a globally atomic transaction.

For fleet-level monitoring, central log aggregation must group these events by deployment metadata already supplied by the runtime/logging platform, rather than adding application-level provider or identity labels.

No public metrics endpoint and no new runtime metrics dependency are introduced by this contract.

## Rollout monitoring

During an identity rollout, watch at least:

1. successful authentication throughput relative to the pre-rollout baseline;
2. changes in the `unauthorized` ratio relative to the same traffic pattern;
3. any sustained `dependency_unavailable` observations;
4. any sustained `internal_error` observations;
5. the existing generic JWKS refresh/stale-cache warnings from the verifier;
6. browser login/refresh/logout and API-proxy acceptance checks from the rollout plan.

A universal percentage threshold is intentionally not hard-coded here. Traffic shape differs by environment, and invalid unauthenticated traffic may be normal. Establish an environment baseline and alert on material deviation during rollout.

A rise in `unauthorized` can indicate token profile, audience, issuer, algorithm/type policy, credential transport, or expiry problems, but the aggregate event deliberately does not expose which token or user triggered it. Use access-controlled server diagnostics and the provider-neutral preflight tools for investigation.

`dependency_unavailable` is higher-severity during rollout because it means authentication could not complete through the verification dependency path rather than simply rejecting an invalid credential.

## JWKS visibility

The verifier already emits generic server-side diagnostics for JWKS fetch failure, stale-cache fallback, forced-refresh throttling, and invalid key-set data. Those messages remain provider-neutral and must not include key material or bearer tokens.

This change does **not** claim to export dedicated `auth_jwks_refresh_total` or cache-age metrics. If the deployment later requires first-class metrics for those signals, add them as a separate bounded observability contract with the same low-cardinality/privacy rules rather than inferring them from token contents.

## Failure behavior

Observability is passive. Recording an outcome returns the original `AppResult` unchanged.

Therefore telemetry must never:

- convert an authentication failure into success;
- change the public HTTP error category;
- retry authentication;
- bypass JWT/JWKS verification;
- create a dependency on the log pipeline for request success.

If logging is filtered or unavailable, authentication semantics remain unchanged.

## Production acceptance

Repository tests prove that:

- the result taxonomy is fixed and does not derive labels from error text;
- aggregate counters track the four categories;
- observing an `AppResult` preserves the original result;
- normal authenticated request extraction passes through the observation hook;
- format and Clippy gates cover the implementation.

They do **not** prove that a deployed log collector, dashboard, or alerting route is working.

Before production cutover:

1. confirm aggregate events arrive in the intended private operational log pipeline;
2. verify dashboards/queries distinguish the four fixed categories;
3. confirm no token, subject, `jti`, `kid`, provider name, or internal identity implementation detail appears in sampled events;
4. establish the pre-cutover success/unauthorized baseline;
5. exercise an invalid-token case and confirm `unauthorized` changes without exposing credential detail;
6. exercise a controlled verification-dependency failure in staging and confirm `dependency_unavailable` is visible;
7. verify alert routing for sustained dependency/internal failures;
8. retain the real-token, browser-flow, signing-key overlap/rollback, and JWKS checks required by issue #13.

Successful repository CI is necessary application evidence, not a substitute for this deployment rehearsal.
