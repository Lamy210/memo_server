# Identity signing-key rotation and rollback

## Purpose

This runbook defines the provider-neutral signing-key rollout contract for `memo_server` access tokens.

It complements `docs/authentication.md` and the identity rollout/access-token preflights. The goal is to preserve verification for every access token that can still be valid while an issuer adds a new signing key, changes issuance, rolls back issuance, or retires an old verification key.

The concrete identity implementation is outside this contract. No provider SDK, provider-specific claim, private signing key, or provider-specific control plane is required by `memo_server`.

## Safety invariant

A verification key MUST remain published while an access token signed by that key can still be valid.

Stopping issuance with a key does not make already issued tokens disappear. Therefore neither forward cutover nor rollback permits immediate verification-key retirement.

For a key that has stopped signing new access tokens, retain its public verification key for at least:

```text
AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS
+ memo_server verifier clock skew (currently 30 seconds)
+ deployment / propagation / monitoring safety margin
```

The deployment margin is intentionally operational rather than hard-coded in `memo_server`: it must cover issuer publication propagation, replica rollout skew, monitoring delay, and any other environment-specific convergence time.

## Forward rotation

Assume `old` is the current signing key and `new` is the replacement. They must use distinct `kid` values.

1. Publish `old + new` in JWKS before issuing any token with `new`.
2. Confirm the intended `memo_server` signature policy accepts the new algorithm/key type.
3. Keep issuing with `old` until JWKS overlap is observable from the deployment environment.
4. Start issuing new access tokens with `new` while continuing to publish `old + new`.
5. Validate both an unexpired `old` token and a `new` token through the real resource-server verification path.
6. Keep both verification keys published for the complete old-token validity horizon plus clock-skew and deployment margin.
7. Retire `old` only when no valid token signed by `old` can remain and rollout telemetry is healthy.

For an RS256 -> ES384 migration, the algorithm-policy stages in `docs/authentication.md` still apply. Key overlap and algorithm overlap are separate requirements; satisfying only one is insufficient.

## Rollback during overlap

If the new issuer behavior must be rolled back:

1. Stop issuing with `new` and resume issuance with `old`.
2. Continue publishing `old + new`.
3. Validate a freshly issued `old` token.
4. Continue accepting already issued, unexpired `new` tokens. Their validity is not revoked merely because issuance rolled back.
5. Keep `new` published until the last possible `new` token has passed its validity horizon plus clock-skew and deployment margin.
6. Only then may `new` be removed from JWKS.

Removing `new` immediately after switching issuance back to `old` is an invalid rollback: it can turn otherwise valid in-flight access tokens into authentication failures.

If an emergency requires immediate invalidation rather than graceful rollback, treat that as an explicit revocation/security incident procedure. The normal stateless JWT/JWKS contract intentionally does not provide per-token introspection or instant fleet-wide revocation.

## Cache behavior

`memo_server` maintains a bounded per-process JWKS cache. An unknown `kid` can trigger a forced refresh, refreshes are serialized and throttled, and stale cached keys can be used for a bounded period when a normal refresh fails.

Consequences for operations:

- publish a new key before first use; do not depend on the first request racing a publication event;
- do not use cache expiry as a substitute for a correct overlap window;
- removing a key upstream does not imply every replica immediately forgets a previously cached copy;
- a throttled forced refresh may reuse a cached snapshot only while it remains within the same bounded stale-if-error horizon used for scheduled-refresh failures; once this horizon has passed, verification fails closed rather than reviving an arbitrarily old key set;
- retirement checks therefore concern the token-validity horizon and fleet convergence, not an instantaneous global rejection point;
- rollback should be safe even if different replicas temporarily observe different valid JWKS snapshots during normal propagation, because both keys remain published throughout the overlap.

## Rotation telemetry

`docs/identity-auth-observability.md` defines the fixed JWKS refresh event used during rollout. Operators should correlate the following provider-neutral signals with the rotation timeline:

- `trigger=scheduled`: normal initial/cache-expiry refresh;
- `trigger=unknown_kid`: the current snapshot did not contain the token's key and a bounded forced refresh was requested;
- `trigger=invalid_signature`: signature verification requested a bounded forced refresh;
- `outcome=success`: the refreshed, validated key set replaced the cache;
- `outcome=stale_fallback`: a normal refresh failed but an eligible stale snapshot kept verification available;
- `outcome=throttled`: a forced refresh was suppressed by cooldown;
- `outcome=unavailable`: the JWKS dependency could not supply a usable response;
- `outcome=invalid_response`: the returned data failed size/shape/key-set validation.

The concrete `kid`, issuer, JWKS URL, token, subject, and raw upstream error are deliberately absent. Diagnose a rotation from the fixed event sequence plus access-controlled deployment context, not by adding those values to application logs.

A short-lived `unknown_kid` or `invalid_signature` followed by `success` can be expected during controlled convergence. Sustained `unknown_kid`, repeated `invalid_signature`, `stale_fallback`, `unavailable`, or `invalid_response` after the expected propagation interval is a rollout warning. Stop advancing key retirement until the cause is understood and the fleet is healthy.

## Repository regression harness

`backend/tests/identity_key_rotation.rs` exercises the canonical verifier source rather than implementing a second JWT validator.

The deterministic test fixtures contain only public JWK material and pre-signed non-production tokens. No production/staging private signing key or credential belongs in the repository.

The HTTP-backed regression additionally exercises the canonical verifier's real JWKS fetching, caching and cryptographic verification against a loopback fixture: initial key publication, unknown-kid discovery of overlapping keys, temporary HTTP 503 errors, malformed JSON, rejection after the bounded stale horizon, recovery, and forced-refresh cooldown without extra HTTP requests.

The fixture injects an HTTP client **only in the test**; the production JWKS client remains HTTPS-only with redirects disabled. The HTTP fixture does not prove certificate validation or the behavior of a real deployed issuer.

The multi-replica regression checks two verifier instances with independent JWKS caches during a staged rotation: one instance learns the new key while the other sees an HTTP 503, continues accepting the still-valid old key, rejects the unknown new key during forced-refresh cooldown, then converges after the issuer recovers. Another HTTP fixture deliberately stalls after a partial response body to verify that the JWKS request deadline covers streaming and that incomplete key sets are never cached.

These tests exercise an in-process, deterministic loopback fixture. They do not substitute for staging multi-replica rollout, external HTTPS/TLS trust-chain validation, real network partition testing, or operational evidence from the production issuer.

The regression covers:

1. `old` only: an `old` token verifies;
2. overlap `old + new`: both tokens verify;
3. issuer rollback to `old` while overlap remains: both the newly preferred `old` token and previously issued `new` token continue to verify;
4. post-horizon rollback completion: a snapshot containing only `old` accepts `old` and rejects `new` fail-closed.

The static fixture lifetime is intentionally longer than deployable policy so the checked-in signed tokens do not expire as the test ages. It is constructed directly at the verifier boundary and does not change the deployment configuration contract; normal `AppConfig` validation still constrains `AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS`.

## What CI proves

The repository test proves that the checked-in verifier logic preserves the intended overlap/rollback behavior for controlled JWKS snapshots and real cryptographic signature verification.

It does **not** prove that a concrete deployed issuer:

- actually publishes both keys before cutover;
- uses the intended `kid` values and algorithms;
- preserves overlap for the required real-world duration;
- issues production-profile claims correctly;
- propagates JWKS changes consistently across its edge/cache layers;
- completes browser login/refresh/logout correctly;
- delivers the application telemetry through the deployed log collector/dashboard/alert path.

Those remain deployment acceptance work.

## Production rehearsal

Before production traffic switches:

1. run the public issuer/JWKS rollout preflight;
2. issue a real staging token with the production profile and validate it through the Rust access-token preflight;
3. confirm the private operations pipeline receives the fixed authentication/JWKS events without credential, key identifier, issuer, or JWKS URL leakage;
4. publish the new verification key while the old key remains present;
5. validate real tokens signed by both keys during overlap and observe healthy refresh outcomes;
6. switch staging issuance to the new key and exercise browser login/refresh/logout;
7. rehearse rollback by returning issuance to the old key while keeping both verification keys published;
8. validate that previously issued new-key tokens still work during the rollback overlap and that JWKS telemetry stays healthy;
9. after the new-token validity horizon plus safety margin, rehearse retiring the new key and confirm expected fleet convergence;
10. verify alert routing and the rollback decision path, then repeat the approved sequence for production.

A successful repository test is necessary evidence for the application behavior, but it is not a substitute for this real issuer rehearsal.
