# JWKS incident triage and recovery

This runbook complements [authentication](authentication.md), [key rotation](identity-key-rotation-rollout.md), and [observability](identity-auth-observability.md). It covers operational decisions when a deployed verifier cannot obtain or use the issuer's public keys. It does **not** change verifier behavior, authorize bypassing authentication, or specify a particular identity provider.

## Immediate safety rules

- Keep `AUTH_MODE=jwt` in production; never switch to `development` to restore access.
- Never disable JWT signature, issuer, audience, lifetime, or algorithm validation.
- Never expand JWKS stale-cache windows or token lifetime as an emergency workaround.
- Never copy tokens, cookies, authorization codes, raw `kid`, private signing keys, or JWKS URLs into tickets, screenshots, or public logs.
- If key compromise is suspected, use the security incident process rather than ordinary graceful rotation. Stateless JWT verification cannot promise immediate revocation of an already issued token.

## Incident decision matrix

| Observation | Possible cause (not proof) | Safe first action | Do not do |
| --- | --- | --- | --- |
| `scheduled/unavailable` and authentication dependency failures rise | JWKS endpoint outage, TLS/DNS/egress failure, request/body timeout | Freeze key rollout; check private network and issuer health; preserve existing keys | Add untrusted HTTP fallback or follow redirects |
| `scheduled/invalid_response` | Malformed/oversized JWKS, duplicate `kid`, incompatible key metadata | Validate published **public** JWKS against the documented shape and limits; restore last known-good published set | Bypass parser or loosen limits |
| Persistent `unknown_kid` after rollout | Issuing before publishing, propagation split, or wrong issuer metadata | Halt new-key issuance where safely possible; republish overlapping public keys; verify both token generations across replicas | Immediately delete the old key |
| Persistent `invalid_signature` | Wrong signing material for an existing `kid`, bad rotation, or malicious/invalid traffic | Check issuer publishing and signing configuration through authorized channels; ensure distinct `kid` per key | Reuse a `kid` for a different key |
| `stale_fallback` during outage | Cached valid key set within bounded fallback horizon | Restore JWKS delivery promptly; monitor for transition to `success` or dependency failure | Assume fallback will continue indefinitely |
| `throttled` with failures on one replica | Forced-refresh cooldown plus divergent per-process cache state | Verify issuer publication and replica convergence after cooldown; compare fleet aggregates | Restart all replicas at once to flush caches |
| `unauthorized` rises without JWKS warnings | Invalid audience/issuer/token profile, expiry, or caller behavior | Re-run access-token preflight and inspect deployment policy without exposing claims | Treat all 401s as a JWKS outage |

These are hypotheses for triage, not one-to-one mappings from telemetry categories to root cause.

## Recovery procedure

1. Record incident start, affected environment, deployed revision, change/rotation timeline, and aggregate counts. Use platform metadata for replica grouping rather than adding identifiers to application log labels.
2. Freeze key retirement, issuer algorithm changes, and nonessential auth deployments. Preserve the overlap of public verification keys for every token that may remain valid.
3. Establish whether the outage is network/transport (`unavailable`), malformed publication (`invalid_response`), or policy/issuance mismatch (sustained `unknown_kid`/`invalid_signature`). The categories alone do not prove a root cause.
4. Restore a correct HTTPS JWKS response with a valid, non-ambiguous public key set. Keep redirects disabled and existing response-size/key-count constraints.
5. Confirm `success` refresh outcomes on all intended replicas using controlled staging requests and private telemetry. Test old and new, unexpired token generations if rotation is in progress.
6. Re-run the documented issuer/JWKS preflight, Rust access-token preflight, and browser login/refresh/logout acceptance workflow.
7. Verify that the original dependency failures stop, that normal 401 behavior remains for invalid tokens, and that logs do not expose sensitive values.
8. Resume rollout only after the configured maximum token lifetime plus skew and deployment margin is accounted for; obtain explicit operational approval before retiring a key.

## Multi-replica verification

Per-process caches and forced-refresh cooldown can temporarily produce different answers across replicas. A successful request to one replica does **not** establish fleet convergence. Test every reachable staging replica, or a documented representative topology when direct addressing is unavailable. Include an old valid token, new valid token, expired token, wrong-audience token, and a request without credentials. Expected results are successful verification only for the two valid tokens **during an authorized overlap**, and rejection of the others.

In production, prefer controlled probes that do not write memos. Do not introduce a public endpoint exposing verifier cache internals.

## Tabletop / fault-injection acceptance

| Scenario | Expected invariant |
| --- | --- |
| JWKS 503 after a healthy cache | Bounded stale-if-error behavior; fail-closed beyond horizon |
| JWKS response stalls after sending partial JSON | Whole-body deadline applies; incomplete data never replaces a good cache |
| Two replicas see different valid snapshots during overlap | Previously valid keys continue to work within policy; replicas converge after recovery |
| Unknown `kid` while refresh is throttled | No unbounded request fan-out; no unlimited use of stale keys |
| Duplicate `kid` or malformed metadata | Reject ambiguous JWKS rather than choosing an arbitrary key |
| Rollback of new issuance | Keep both public keys until previously issued new tokens can no longer be valid |
| Log collector down | Authentication result remains unchanged |

Repository regressions cover several of these cases but are not a substitute for deployed staging acceptance. Capture the exact test revision, scenario outcome, and rollback decision in the release record.

## Rollback triggers

Stop rollout and investigate if authentication dependency failures persist after the expected JWKS propagation interval, if any replica accepts an invalid token, if legitimate valid tokens cannot be verified after healthy propagation, or if sensitive authentication data appears in logs. Security verification failures take precedence over availability; restoring service must not change token acceptance policy.
