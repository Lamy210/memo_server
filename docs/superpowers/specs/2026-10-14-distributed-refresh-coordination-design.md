# Distributed Browser Refresh Coordination Design

**Date:** 2026-10-14  
**Status:** review-required  
**Scope:** SvelteKit BFF browser-session refresh coordination  
**Related:** `docs/browser-auth-operations.md`, `docs/authentication.md`, issue #10, issue #13

## Goal

Remove the current one-active-BFF-replica correctness constraint for browser-session traffic without changing the provider-neutral OAuth/OIDC/JWT boundary or exposing access/refresh credentials to browser JavaScript.

Today, `frontend/src/hooks.server.ts` deduplicates near-expiry refresh work with a process-local `Map` keyed by the sealed session cookie. This prevents duplicate redemption only inside one SvelteKit process. Two replicas can receive the same sealed session concurrently and redeem the same rotating refresh token at the same time. With strict refresh-token rotation, one redemption may succeed while another receives `invalid_grant`, producing divergent session state.

The target design adds Valkey-backed shared refresh coordination while preserving the sealed cookie as the browser-facing session artifact.

Success means:

- multiple active SvelteKit BFF replicas can process the same browser session safely;
- for one old sealed session generation, at most one upstream refresh attempt is initiated while coordination state is intact;
- a successful refresh can be handed to concurrent follower replicas without putting plaintext token material in Valkey;
- an ambiguous refresh outcome is never retried with the same old refresh credential;
- a Valkey outage cannot trigger uncoordinated refresh;
- a still-valid access token can be used safely during a transient coordination outage;
- expired or indeterminate sessions fail closed;
- the backend JWT/JWKS verification and provider-neutral identity contracts remain unchanged.

## Non-goals

This change does not:

- move memo authorization into the frontend;
- replace the backend JWT/JWKS verifier;
- depend on provider-specific refresh-token grace/reuse behavior;
- introduce a public session API;
- store plaintext access tokens or refresh tokens in Valkey;
- make Valkey the source of memo-domain data;
- change `AUTH_MODE=development` behavior;
- make browser authentication available when the identity provider itself is unavailable.

## Current constraints

The current production browser session is an AES-256-GCM sealed `Secure` + `HttpOnly` cookie containing:

- `accessToken`;
- `accessTokenExpiresAt`;
- `refreshToken`.

`restoreIdentitySession()` refreshes inside a 30-second early-refresh window. The current process-local `refreshFlights` map correctly deduplicates only requests handled by the same process.

The OIDC client already bounds discovery/token requests with a 10-second HTTP deadline. Shared coordination must therefore use a lease longer than 10 seconds and must treat lease loss after an upstream request may have started as an ambiguous outcome rather than permission to retry.

`docs/browser-auth-operations.md` currently permits only one active BFF replica for browser-session traffic. That restriction is removed only after shared coordination is enabled and its multi-replica acceptance tests pass.

## Chosen architecture

Use **sealed-cookie session state plus Valkey-backed refresh coordination**.

```text
Browser
  |
  | __Host-schnee_session = sealed session S0
  v
Any SvelteKit BFF replica
  |
  | restoreIdentitySession(S0)
  v
Shared refresh coordinator
  |
  +--> derive coordination id from S0 with a server-only derived key
  |
  +--> Valkey state for {coordination-id}
  |      - persistent attempt marker
  |      - short leader lease
  |      - short authenticated sealed-result handoff
  |
  +--> exactly one leader may call the OIDC token endpoint
  |         |
  |         +--> success -> sealed session S1 -> authenticated handoff
  |         |
  |         +--> failure/ambiguity -> S0 is never redeemed again
  |
  +--> followers wait for S1 or fail closed
  |
  v
Browser receives S1 cookie
```

The application will not implement Redlock across independent Valkey primaries. Production uses one logically authoritative coordination service with its own HA/failover topology. Application correctness assumes that this service does not concurrently accept conflicting writes for the same coordination key.

## Why not a full server-side session store

A full server-side session store would also solve cross-replica refresh but would make the shared store a hard dependency for every authenticated request and would replace the current browser-session architecture with opaque server-side session state.

That larger migration is unnecessary for this problem. Shared coordination is consulted only near refresh or when resolving an already-started refresh generation. Requests with comfortably valid access tokens remain on the existing sealed-cookie path.

## Why not rely on provider refresh-token grace

Provider-specific refresh-token reuse/grace semantics are outside the stable application contract. Correctness must hold for the strict case where the refresh token is single-use and a second redemption fails.

## Bounded V2 session schema

The current payload has no bounded logical session lifetime. A replay marker cannot be garbage-collected safely if an old sealed cookie can be accepted indefinitely.

Coordination therefore introduces:

```ts
type IdentitySessionV2 = {
  schemaVersion: 2;
  accessToken: string;
  accessTokenExpiresAt: number;
  refreshToken: string;
  sessionExpiresAt: number;
};
```

Requirements:

- `sessionExpiresAt` is fixed at login;
- refresh never extends `sessionExpiresAt`;
- `openSession()` rejects V2 sessions after that deadline;
- shared coordination accepts only V2 sessions;
- legacy sessions are cleared and require re-login when shared coordination is enabled;
- coordination-disabled single-replica mode may continue to accept legacy sessions during migration;
- V2 sessions remain valid if the deployment later rolls back to single-replica mode.

The cookie can remain a browser-session cookie without `Max-Age`; the application deadline is authoritative.

### Session lifetime configuration

Add private configuration:

```text
IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS
```

Rules:

- required when shared coordination is enabled;
- integer in `900..=86400` seconds;
- no silent production default in coordination-enabled mode;
- login sets `sessionExpiresAt = now + configured lifetime`.

This is an application session bound, not a claim about the upstream refresh-token lifetime.

## Coordination configuration

Add private configuration:

```text
IDENTITY_REFRESH_COORDINATION_URL
IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS
```

`IDENTITY_REFRESH_COORDINATION_URL` accepts `valkey://`, `valkeys://`, and compatible `redis://` / `rediss://` URLs supported by the selected client.

Modes:

1. **Disabled:** coordination URL absent. Preserve process-local single-flight and the one-active-replica production restriction.
2. **Enabled:** coordination URL present. V2 bounded sessions and shared coordination are mandatory for refresh; multi-replica browser-session traffic is permitted after acceptance testing.

Enabled mode must never silently fall back to process-local-only refresh when Valkey is unavailable.

## Valkey client selection

Use `iovalkey` for the first implementation.

Rationale:

- maintained under `valkey-io`;
- TypeScript implementation with declarations;
- package metadata supports Node.js `>=18.12.0`, including this repository's Node 24 runtime;
- supports Valkey URLs, TLS, Lua scripting, Cluster, and Sentinel;
- no native runtime dependency is added to `node:24-alpine`;
- the required primitive is a small set of string operations plus atomic scripts.

Use the stable version resolved by pnpm at implementation time and commit the lockfile. The design does not require a Valkey server upgrade; current Valkey 9.1.2 is sufficient.

References:

- <https://github.com/valkey-io/iovalkey>
- <https://valkey.io/commands/set/>
- <https://valkey.io/topics/distlock/>

## Cryptographic domain separation

Valkey is a shared coordination dependency and may contain attacker-controlled or corrupted values after an operational incident. A sealed session is authenticated, but without additional binding a valid sealed session from one coordination key could be copied into another key.

Do not use `IDENTITY_SESSION_KEY` directly as an HMAC key. Derive coordination-only keys with HKDF-SHA-256 from the existing uniformly random 32-byte session key.

Use distinct HKDF `info` values:

```text
memo_server/identity-refresh/v1/key-id
memo_server/identity-refresh/v1/result-mac
```

The derived keys never leave the SvelteKit server process.

### Coordination identifier

For old sealed session `S0`:

```text
coordination_id = hex(HMAC-SHA-256(key_id_key, S0))
```

This avoids putting the raw cookie or an unkeyed cookie fingerprint in Valkey key names.

### Authenticated result envelope

Valkey stores a result envelope containing the already-encrypted sealed session plus a context-binding MAC:

```ts
type RefreshResultEnvelope = {
  version: 1;
  sealedSession: string;
  mac: string;
};
```

Compute:

```text
mac = HMAC-SHA-256(
  result_mac_key,
  "v1\0" || coordination_id || "\0" || sealedSession
)
```

Followers verify this MAC in constant time before opening `sealedSession`.

This prevents a Valkey writer that does not know the session key from forging a handoff or swapping a valid result between coordination generations. It does not protect against deletion of coordination keys; key retention/integrity remains part of the Valkey operational trust boundary.

## Valkey key design

Use a Cluster-compatible hash tag so keys for one generation share a slot:

```text
memo:identity:refresh:v1:{<coordination_id>}:attempt
memo:identity:refresh:v1:{<coordination_id>}:lease
memo:identity:refresh:v1:{<coordination_id>}:result
```

Values:

- `attempt`: cryptographically random owner token, at least 128 bits;
- `lease`: the same owner token with short TTL;
- `result`: authenticated `RefreshResultEnvelope`.

No Valkey key/value contains plaintext issuer credentials, client secrets, access tokens, refresh tokens, subject IDs, authorization codes, or raw browser cookies.

## TTLs

### Attempt replay guard

Set attempt TTL to:

```text
max(1 second, sessionExpiresAt - now) + 60 seconds
```

The old refresh credential therefore cannot be retried while the old session is logically acceptable.

### Lease

Use **15 seconds**.

The identity HTTP deadline is 10 seconds; the remaining margin covers scheduling and result publication. Lease expiry is never permission to retry because the attempt marker remains.

### Result

Keep the authenticated sealed result for **120 seconds**.

After result expiry, S0 still cannot become leader again because the attempt marker remains. A very stale S0 therefore causes re-login rather than a second redemption.

## Atomic operations

Use small Lua scripts through `iovalkey`. All touched keys share one hash slot.

### Acquire

Atomically:

1. if `result` exists, return `RESULT` and its envelope;
2. else if `attempt` and `lease` exist, return `WAIT`;
3. else if `attempt` exists and `lease` does not, return `INDETERMINATE`;
4. else set `attempt=<owner>` with replay-guard TTL and `lease=<owner>` with 15-second TTL, then return `LEADER`.

The attempt marker exists before the leader is allowed to start the upstream token request.

### Publish success

The leader may publish only while `attempt == owner`.

Atomically:

1. verify attempt ownership;
2. store the authenticated result envelope for 120 seconds;
3. delete `lease` only if it still contains the same owner;
4. leave `attempt` until its replay-guard TTL expires.

If attempt ownership has expired, the leader must not publish into a new generation and must treat its local state as indeterminate.

### Complete failure

On upstream refresh error, HTTP timeout, parse failure, sealing failure, MAC/envelope construction failure, or uncertain completion:

- never delete `attempt`;
- delete `lease` only if owned by this leader;
- never retry S0's refresh credential;
- clear the browser session for the failed request.

## Refresh state machine

```text
VALID_ACCESS_TOKEN
    |
    | enters 30s early-refresh window
    v
NEEDS_REFRESH
    |
    | shared acquire
    +-------------------------------+
    |                               |
    v                               v
LEADER                            FOLLOWER
    |                               |
    | attempt already persisted      | RESULT -> verify MAC -> open S1
    |                               | WAIT -> bounded polling
    v                               | INDETERMINATE -> clear session
UPSTREAM_REFRESH                    |
    |                               |
    +--> success -> seal S1 ---------+
    |               MAC-bind result
    |               publish result
    |               set cookie
    |
    +--> any failure / ambiguity
                    |
                    v
              CLEAR SESSION
```

## Follower behavior

A follower receiving `WAIT` polls only this generation's keys.

Requirements:

- total wait limit: **12 seconds**;
- use short jittered intervals, not a tight loop;
- `RESULT`: verify result MAC, open/validate S1, then adopt it;
- lease disappears with no result: `INDETERMINATE` immediately;
- wait deadline reached while lease exists: fail closed for this request and never call the upstream token endpoint;
- follower timeout does not cancel the leader.

A MAC failure or invalid/decrypt-failing S1 is treated as indeterminate and never causes an S0 retry.

## Valkey outage behavior

### Token still valid

When coordination is unavailable but `accessTokenExpiresAt > now`:

- do not call the refresh endpoint;
- use the still-valid token for the current request;
- do not rewrite the cookie;
- emit a fixed coordination-unavailable outcome;
- allow a later request to try coordination again.

No refresh credential is consumed, so this does not weaken exactly-once refresh behavior.

### Token expired

When coordination is unavailable and the access token is expired:

- do not call refresh;
- clear the session;
- expose unauthenticated state;
- require re-login.

### Indeterminate generation

If `attempt` exists without a usable result and without an active lease, clear the session immediately. Do not use the old token merely because it has a few seconds remaining.

## Security properties

The implementation must preserve all of the following:

- browser JavaScript never receives access/refresh credentials;
- Valkey never stores plaintext access/refresh credentials;
- raw sealed cookies never appear in Valkey keys or logs;
- coordination IDs use a server-keyed HMAC, not plain hashing;
- result handoffs are MAC-bound to the exact coordination generation;
- MAC checks use constant-time comparison;
- owner tokens use cryptographically secure randomness;
- attempt state is persisted before upstream redemption starts;
- request failure never deletes the attempt marker;
- an observed attempt marker can never lead to retry of the same old refresh credential;
- browser errors remain generic/provider-neutral;
- logs never include coordination ID, owner token, sealed result, token values, or Valkey URI credentials;
- `valkeys://`/`rediss://` is required outside a trusted private network.

## Valkey operational requirements

Multi-replica correctness treats Valkey coordination state as correctness state, not disposable cache state.

Required production properties:

- no eviction of attempt/lease/result keys before TTL;
- bounded connect/command timeouts;
- appropriate ACL/authentication;
- private network or TLS;
- HA/persistence consistent with deployment SLOs;
- one logical write authority per key generation.

A memory policy that may evict these keys before TTL is unsupported for multi-replica correctness.

Valkey is inside the replay-marker integrity trust boundary: an attacker able to delete attempt state can defeat the no-retry invariant. The result MAC limits value forgery/substitution but cannot protect against malicious deletion.

## Component boundaries

### `session.ts`

Owns:

- V1/V2 parsing;
- `sessionExpiresAt` validation;
- sealing/opening;
- compatibility rules.

It does not know Valkey commands.

### `refreshCoordinator.ts` (new)

Owns:

- HKDF/HMAC coordination key derivation;
- Valkey key derivation;
- result-envelope MAC creation/verification;
- Valkey client boundary;
- acquire/publish/failure scripts;
- bounded follower waiting;
- fixed coordination outcomes.

Suggested caller-facing shape:

```ts
type RefreshCoordinationDecision =
  | { kind: 'leader'; owner: string }
  | { kind: 'result'; sealedSession: string }
  | { kind: 'indeterminate' }
  | { kind: 'unavailable' };

interface RefreshCoordinator {
  acquire(input: {
    sealedSession: string;
    replayGuardMs: number;
  }): Promise<RefreshCoordinationDecision>;

  publishSuccess(input: {
    sealedSession: string;
    owner: string;
    refreshedSealedSession: string;
  }): Promise<boolean>;

  completeFailure(input: {
    sealedSession: string;
    owner: string;
  }): Promise<void>;
}
```

The exact internal names may differ. Callers must not depend directly on Valkey commands or key formats.

### `hooks.server.ts`

Owns orchestration only:

- open current session;
- decide whether refresh is needed;
- call coordinator;
- leader performs OIDC refresh;
- follower consumes verified result;
- set/clear cookie and locals.

The existing process-local map remains only in coordination-disabled mode or as a local optimization that cannot bypass shared acquire.

### `config.ts`

Owns:

- optional coordination URL validation;
- mandatory session max age when enabled;
- URL scheme restrictions;
- numeric bounds.

No raw coordination URL is exposed in public errors.

## Observability

Add fixed low-cardinality internal outcomes:

```text
leader
follower_result
follower_wait_timeout
indeterminate
unavailable_valid_token_fallback
unavailable_expired_session
upstream_refresh_failed
publish_failed
result_authentication_failed
success
```

Do not log coordination IDs, owner values, result envelopes, tokens, subject/client identifiers, or Valkey URI credentials.

## Failure modes

| Failure | Required behavior |
| --- | --- |
| Two replicas receive S0 concurrently | one leader refreshes; followers adopt authenticated S1 |
| Leader dies before upstream call | attempt remains; after lease loss S0 becomes indeterminate |
| Leader dies during/after redemption | no S0 retry; re-login unless authenticated S1 was published |
| Token endpoint times out | ambiguous; attempt remains; clear session |
| Valkey unavailable, token valid | current request may use existing valid token; no refresh |
| Valkey unavailable, token expired | clear session; no refresh |
| Result MAC invalid | clear session; no S0 retry |
| Valid result copied from another coordination key | MAC binding rejects it |
| Publish fails after upstream success | ambiguous; clear session; no S0 retry |
| Lease expires while leader still runs | followers may fail closed; leader publishes only while attempt ownership matches |
| Attempt TTL expires | S0 is already invalid by `sessionExpiresAt` |
| Attempt key evicted/deleted early | unsupported / integrity violation; correctness not guaranteed |

## Testing strategy

Tests must demonstrate fault detection, not only coverage.

### Unit tests

Cover:

- V2 session expiry;
- legacy behavior in enabled/disabled modes;
- HKDF domain separation;
- coordination ID does not contain raw cookie text and changes with S0;
- result MAC validates only for its original coordination ID;
- cross-key result substitution fails;
- tampered result/MAC fails;
- TTL calculations;
- config scheme/range validation;
- sensitive data is absent from fixed error/outcome strings.

### Real-Valkey integration tests

Use Valkey 9.1.2.

Required regressions:

1. **Two independent coordinators, same S0**  
   Upstream refresh stub is called exactly once; both logical replicas converge on S1.

2. **Leader crash after attempt**  
   Expire/remove lease without result; second coordinator returns `indeterminate` and never calls upstream refresh.

3. **Follower result handoff**  
   Follower observing `WAIT` receives and verifies S1 after leader publication.

4. **Result expires while attempt remains**  
   Same S0 never becomes leader again; it fails closed.

5. **Cross-generation substitution**  
   Copy a valid result value to another coordination key; follower rejects it by MAC.

6. **Coordination unavailable with valid token**  
   Existing token is used and refresh is not called.

7. **Coordination unavailable with expired token**  
   Session is cleared and refresh is not called.

8. **Publish failure after upstream success**  
   No second redemption occurs with S0.

9. **Session expiry bounds replay state**  
   Expired V2 session is rejected before coordination.

### Browser/auth E2E

Extend the provider-neutral auth fixture with strict single-use refresh tokens and a deterministic multi-replica scenario using independent coordinator/client instances against one Valkey service.

Acceptance requires:

- exactly one redemption;
- concurrent requests converge where handoff is available;
- no access/refresh credential appears in browser-readable surfaces;
- no expired token is forwarded after failure.

## CI design

Add a dedicated real-Valkey coordination job/workflow.

It must:

1. start Valkey 9.1.2;
2. install dependencies with the repository pnpm toolchain;
3. run coordination integration tests;
4. run relevant TypeScript/Svelte checks;
5. fail on any real-Valkey regression.

Before implementation merge:

- `pnpm check`;
- `pnpm lint`;
- `pnpm test:unit -- --run`;
- real-Valkey coordination integration tests;
- provider-neutral auth E2E;
- `pnpm build`;
- container validation/build;
- full CI green.

## Deployment and migration

### Stage 1: code present, coordination disabled

- add V2-aware parsing and coordinator implementation;
- leave coordination URL absent;
- keep one-active-replica policy;
- verify current auth behavior.

### Stage 2: coordination service validation

- deploy Valkey endpoint with no-eviction policy;
- validate connectivity, ACL/TLS, timeout behavior, and telemetry;
- do not horizontally scale yet.

### Stage 3: enable on one replica

- set coordination URL and browser session max age;
- remaining legacy sessions are cleared and users re-login into V2;
- validate normal refresh, Valkey outage fallback, and indeterminate behavior.

### Stage 4: two-replica canary

- activate a second BFF replica;
- generate concurrent near-expiry requests;
- verify one redemption and successful handoff;
- monitor indeterminate/outage/upstream-failure/login-rate changes.

### Stage 5: normal multi-replica operation

- remove the one-active-replica restriction from `docs/browser-auth-operations.md` only after canary acceptance;
- retain documented rollback to one replica.

## Rollback

If shared coordination is problematic:

1. reduce browser-session traffic to one active BFF replica;
2. remove/disable `IDENTITY_REFRESH_COORDINATION_URL`;
3. use process-local single-flight again;
4. do not run multiple active replicas while disabled;
5. leave backend JWT/JWKS configuration unchanged.

V2 sessions remain valid in single-replica mode, so rollback requires no memo-data or token-contract migration.

## Compatibility

- **Backend API:** unchanged.
- **Memo data:** unchanged.
- **JWT/JWKS contract:** unchanged.
- **Cookie:** same name and security attributes; encrypted payload gains V2 lifetime metadata.
- **Existing browser sessions:** enabling shared coordination intentionally causes one-time re-login for legacy unbounded sessions.
- **Local development:** may keep coordination disabled; real-Valkey tests exercise the new path independently.

## Acceptance criteria

Implementation is accepted only when all are proven:

- two independent BFF coordinators cannot redeem the same old refresh credential twice while coordination state is intact;
- attempt state exists before upstream redemption starts;
- ambiguous leader loss never allows retry of S0;
- followers consume only MAC-authenticated results bound to their coordination generation;
- Valkey contains no plaintext access/refresh credential;
- expired/indeterminate sessions fail closed;
- transient Valkey failure does not reject a still-valid access token unnecessarily;
- enabled mode never falls back to process-local-only refresh;
- disabled mode preserves current single-replica behavior;
- real Valkey and strict single-use refresh fixtures exercise the regression;
- existing provider-neutral auth/frontend/container/full-stack checks remain green;
- the operational one-replica restriction is removed only after real multi-replica acceptance passes.

## Rejected alternatives

### Full server-side session store now

Rejected because it expands shared-store dependency to every authenticated request and requires a broader session-lifecycle migration.

### Lock without persistent attempt marker

Rejected because lease expiry after ambiguous upstream redemption would allow a second refresh attempt.

### Plain SHA-256 coordination key and unbound sealed result

Rejected because it unnecessarily exposes a stable cookie fingerprint to the coordination store and does not prevent cross-key substitution of an otherwise valid sealed result after store compromise/corruption.

### Process-local lock plus sticky routing

Rejected because failover, deploy replacement, or routing changes can move concurrent requests between replicas.

### Provider-specific refresh reuse grace

Rejected because it violates the provider-neutral boundary.

### Retry refresh after timeout

Rejected because a timeout does not prove the provider failed to consume the refresh token.

## Design decision summary

Keep the encrypted browser cookie model and add Valkey only around refresh coordination. Derive coordination-only cryptographic keys from the session key with HKDF. Persist an attempt marker before the upstream request, use a short lease for liveness, and publish only a short MAC-bound sealed result for followers.

The core invariant is:

> Once one sealed session generation has begun an upstream refresh attempt, its old refresh credential is never attempted again.

A bounded V2 session lifetime makes the replay marker garbage-collectable. Ambiguous infrastructure failures intentionally trade session continuity for re-login rather than risking rotating refresh-token reuse. Backend identity verification and memo-domain architecture remain unchanged.
