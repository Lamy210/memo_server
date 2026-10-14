# Browser authentication operations

## Scope

This document records deployment constraints for the provider-neutral SvelteKit browser authentication boundary. It complements `docs/authentication.md`; it does not change the OAuth/OIDC/JWT contract used by `memo_server`.

## Browser session model

The BFF keeps access/refresh state in the existing AES-256-GCM sealed `Secure` + `HttpOnly` browser cookie. Browser JavaScript does not receive access or refresh credentials.

Two session payload generations are supported:

- legacy V1, with access token, access-token expiry, and refresh token;
- V2, which additionally contains `schemaVersion: 2` and fixed `sessionExpiresAt`.

`sessionExpiresAt` is established at login and is never extended by refresh. Shared refresh coordination accepts only V2 sessions so replay-attempt state has a bounded retention horizon.

## Coordination modes

### Shared coordination disabled

When `IDENTITY_REFRESH_COORDINATION_URL` is absent, refresh uses the existing process-local single-flight map.

This mode supports only **one active SvelteKit BFF replica for browser-session traffic**. Sticky routing is not a correctness guarantee because failover, deployment replacement, or routing changes can move concurrent requests between replicas.

V1 and V2 sessions can be handled in this mode. A refreshed V2 session keeps its original `sessionExpiresAt`.

### Shared coordination enabled

Set both private frontend values:

```text
IDENTITY_REFRESH_COORDINATION_URL=<valkey://, valkeys://, redis://, or rediss:// URL>
IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS=<900..86400>
```

When enabled:

- login creates V2 sessions;
- remaining V1 browser sessions are cleared and require a one-time re-login;
- access tokens outside the 30-second early-refresh window do not contact Valkey;
- refreshes inside the window use Valkey-backed cross-replica coordination;
- the application never falls back to process-local-only refresh if shared coordination is unavailable.

The coordination service is a correctness dependency during refresh, not a disposable cache.

## Coordination invariant

For one old sealed session generation S0:

> Once an upstream refresh attempt has started, S0's old refresh credential is never attempted again.

The coordinator persists an attempt marker before the leader can call the identity token endpoint. A short lease indicates expected leader liveness but lease expiry does not authorize another redemption. If an attempt remains without an active lease or authenticated result, the session is indeterminate and must re-authenticate.

Current timing contract:

- leader lease: 30 seconds;
- follower wait deadline: 25 seconds;
- successful result handoff: 120 seconds;
- poll interval: 100–200 ms with jitter;
- attempt replay guard: remaining V2 session lifetime plus 60-second cleanup margin.

The 30-second lease accounts for discovery plus token requests, each of which has an independent 10-second HTTP deadline, plus processing margin.

## Shared result security

Valkey never receives plaintext access tokens, refresh tokens, replayable browser-cookie values, client secrets, or subject identifiers.

The BFF derives three coordination-only keys from `IDENTITY_SESSION_KEY` with HKDF-SHA-256:

- `memo_server/identity-refresh/v1/key-id` for the keyed coordination identifier;
- `memo_server/identity-refresh/v1/result-encryption` for AES-256-GCM result-handoff encryption;
- `memo_server/identity-refresh/v1/result-mac` for the context-binding HMAC.

The coordination identifier is a keyed HMAC of the old sealed cookie. Before a refreshed sealed session S1 is written to Valkey, S1 is encrypted again with the coordination-layer AES-256-GCM key using a fresh 96-bit nonce and `v1\0<coordination-id>` as additional authenticated data. Valkey stores only the nonce, ciphertext, and a MAC bound to the exact coordination generation; it does not store the S1 browser-cookie value itself.

Followers verify the HMAC before AES-GCM decryption. A copied, modified, malformed, cross-generation, or decrypt-failing result causes fail-closed session handling. A cryptographically valid handoff is also rejected if its access token has already expired by the time a follower receives it.

See `docs/superpowers/specs/2026-10-14-distributed-refresh-coordination-handoff-encryption-amendment.md` for the corrective security amendment that supersedes the original plaintext-envelope representation.

## Valkey operational requirements

A multi-replica deployment requires all of the following:

- no eviction of attempt/lease/result keys before their TTL;
- bounded connection and command timeouts;
- appropriate authentication and ACLs;
- private-network placement or TLS (`valkeys://` / `rediss://`) when the network is not trusted;
- HA/persistence selected consistently with the deployment SLO;
- one logical write authority for a coordination key at a time.

An eviction policy that can delete these keys under memory pressure is unsupported for multi-replica correctness. Deleting attempt state early can defeat the no-retry invariant.

The application does not implement Redlock across independent primaries and does not claim correctness for conflicting multi-primary writes.

## Failure behavior

### Valkey unavailable while the access token is still valid

If `accessTokenExpiresAt > now`, the current request may continue with that still-valid access token. The BFF does not refresh or rewrite the cookie. A later request can retry coordination.

### Valkey unavailable after the access token has expired

If `accessTokenExpiresAt <= now`, the BFF clears the session and requires login. It never refreshes without shared coordination.

### Leader/upstream ambiguity

If the leader crashes, the token endpoint times out, result publication fails, or another ambiguous outcome occurs after an attempt is persisted:

- the attempt marker is retained;
- the old refresh credential is not retried;
- followers do not promote themselves to a new leader;
- the affected browser session fails closed and requires login unless a valid authenticated result was already published.

### Invalid or stale shared result

A malformed envelope, HMAC failure, AES-GCM authentication failure, cross-generation result, invalid/decrypt-failing sealed session, changed `sessionExpiresAt`, or handoff whose `accessTokenExpiresAt <= now` is rejected. The browser session is cleared and the old refresh credential is not retried.

## Verification coverage

The repository includes deterministic tests for:

- two independent coordinators processing the same near-expiry V2 session against real Valkey;
- strict single-use refresh semantics with exactly one upstream redemption;
- follower adoption of the refreshed sealed session;
- result-handoff ciphertext not exposing the replayable browser-cookie value;
- fresh handoff encryption nonces and independent HKDF encryption/MAC domains;
- rejection of an authenticated but already-expired handoff access token;
- leader loss with the attempt marker retained;
- publish failure after an upstream success;
- cross-generation result substitution;
- result expiry while the replay guard remains;
- Valkey outage with valid versus expired access tokens;
- the 30-second lease / 25-second follower deadline timing envelope;
- no regression of the existing process-local single-flight path when shared coordination is disabled.

The dedicated `Identity refresh coordination` GitHub Actions workflow runs the handoff-security contracts, the real-Valkey integration suite, TypeScript/Svelte checks, and linting.

## Deployment sequence

1. Deploy the code while leaving shared coordination disabled; continue with one active BFF replica.
2. Provision Valkey with the no-eviction, network, ACL/TLS, timeout, HA, and monitoring requirements above.
3. Enable `IDENTITY_REFRESH_COORDINATION_URL` and `IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS` on one replica. Existing V1 sessions will re-login into bounded V2 sessions.
4. Exercise normal refresh, Valkey outage, identity-provider failure, and indeterminate-session behavior.
5. Run a two-replica canary with concurrent near-expiry requests. Confirm exactly one upstream refresh redemption and successful follower handoff.
6. Monitor refresh failures, indeterminate sessions, coordination-unavailable outcomes, and login-rate changes before increasing traffic.

## Current production topology restriction

The implementation and CI coverage are necessary but not sufficient to declare a real environment ready for horizontal browser-session traffic.

**Keep browser-session BFF traffic on one active replica until the real deployment canary in the previous section succeeds.**

Only after that acceptance should the deployment policy be changed to permit multiple active BFF replicas.

## Rollback

If shared coordination behaves unexpectedly:

1. reduce browser-session traffic to one active BFF replica;
2. remove or disable `IDENTITY_REFRESH_COORDINATION_URL`;
3. retain V2 sessions; they remain valid in the single-replica path;
4. return to process-local single-flight refresh;
5. leave backend JWT/JWKS configuration unchanged.

Do not disable shared coordination while multiple active replicas continue serving browser-session refresh traffic.
