# Distributed Browser Refresh Coordination Design

**Date:** 2026-10-14  
**Status:** review-required  
**Scope:** SvelteKit BFF browser-session refresh coordination  
**Related:** `docs/browser-auth-operations.md`, `docs/authentication.md`, issue #10, issue #13

## Goal

Remove the current one-active-BFF-replica correctness constraint for browser-session traffic without changing the provider-neutral OAuth/OIDC/JWT boundary or exposing access/refresh credentials to browser JavaScript.

Today, `frontend/src/hooks.server.ts` deduplicates near-expiry refresh work with a process-local `Map` keyed by the sealed session cookie. This prevents duplicate redemption only inside one SvelteKit process. Two replicas can receive the same sealed session concurrently and redeem the same rotating refresh token at the same time. With strict refresh-token rotation, one redemption may succeed while another receives `invalid_grant`, causing divergent session handling.

The target design adds shared Valkey coordination while preserving the sealed cookie as the browser-facing session artifact.

Success means:

- multiple active SvelteKit BFF replicas may process the same browser session safely;
- for one old sealed session generation, at most one upstream refresh attempt is initiated;
- a successful refresh can be handed off to concurrent follower replicas without exposing raw token material to Valkey;
- an ambiguous refresh outcome is never retried with the same old refresh credential;
- a Valkey outage cannot cause duplicate refresh redemption;
- already-valid access tokens remain usable when safe during a transient coordination outage;
- expired or indeterminate sessions fail closed;
- existing backend JWT/JWKS verification and provider-neutral identity contracts remain unchanged.

## Non-goals

This change does not:

- move memo authorization into the frontend;
- replace the backend JWT/JWKS verifier;
- add provider-specific refresh-token behavior;
- depend on a provider grace/reuse window;
- introduce a public session API;
- store raw access tokens or raw refresh tokens in Valkey;
- make Valkey the source of memo-domain data;
- change the current development `AUTH_MODE=development` path;
- make browser authentication available when the identity provider itself is unavailable.

## Existing behavior and constraints

The current production browser session is an AES-256-GCM sealed `Secure` + `HttpOnly` cookie containing:

- `accessToken`;
- `accessTokenExpiresAt`;
- `refreshToken`.

`restoreIdentitySession()` refreshes when the access token is within the 30-second refresh window. A process-local `refreshFlights` map deduplicates concurrent requests carrying the exact same sealed cookie.

The existing OIDC client already bounds discovery and token endpoint requests with a 10-second HTTP deadline. The new shared coordination must therefore use a lease longer than that network deadline and must treat a lease loss after an upstream request may have started as an ambiguous outcome rather than permission to retry.

`docs/browser-auth-operations.md` currently requires one active BFF replica because the process-local map does not coordinate across replicas. This design removes that restriction only when shared coordination is explicitly enabled and healthy.

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
  +--> derive digest = SHA-256(S0)
  |
  +--> Valkey state for {digest}
  |      - attempt marker
  |      - short lease
  |      - optional sealed-success handoff
  |
  +--> exactly one leader may call the OIDC token endpoint
  |         |
  |         +--> success -> sealed session S1 -> publish encrypted handoff
  |         |
  |         +--> failure/ambiguity -> no retry of S0 refresh credential
  |
  +--> followers wait for S1 or fail closed
  |
  v
Browser receives S1 cookie
```

The application will not implement Redlock across multiple independent Valkey primaries. The production requirement is one logically authoritative Valkey coordination service with its own HA/replication/failover architecture. The application-level invariant is fail-closed coordination; it does not claim linearizability across an arbitrarily partitioned multi-primary deployment.

## Why not a full server-side session store

A full server-side session store would also solve cross-replica refresh but would change the browser/session architecture substantially: the cookie would become an opaque session identifier, every authenticated request would require shared-store access, server-side token lifecycle and garbage collection would become mandatory, and the store would become a hard dependency even for requests whose access token is still valid.

That broader change is not required for the current problem. Shared refresh coordination keeps normal authenticated requests on the existing sealed-cookie path and touches Valkey only when a token approaches expiry or when coordination state for that old session generation must be consulted.

## Why not rely on provider refresh-token grace

Provider-specific refresh-token reuse/grace behavior is explicitly outside the stable application contract. Depending on such behavior would make correctness vary by identity implementation and would weaken the provider-neutral boundary.

The design therefore assumes the strict case: a refresh credential may be single-use and a second redemption may fail.

## Session schema evolution

The current session payload has no bounded logical session lifetime. A persistent replay/attempt marker cannot be safely garbage-collected if an old sealed cookie can be presented indefinitely.

Shared coordination therefore introduces a new bounded session schema:

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

- `sessionExpiresAt` is fixed at login and is not extended by token refresh;
- `openSession()` rejects V2 sessions after `sessionExpiresAt`;
- distributed coordination is enabled only for V2 sessions;
- legacy sessions without `schemaVersion: 2` are cleared and require re-login when distributed coordination mode is enabled;
- single-replica mode may continue to accept the legacy schema during the migration window;
- the deployment that enables multi-replica coordination must document the expected one-time re-login for any remaining legacy browser session.

The session cookie may remain a browser-session cookie without `Max-Age`; `sessionExpiresAt` is the authoritative application lifetime. This bounds replay-guard retention even if a browser continues to send an old cookie.

### Browser session lifetime

Add private configuration:

```text
IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS
```

Rules:

- required when distributed coordination is enabled;
- positive integer;
- bounded to an operationally reasonable range, proposed 900 seconds through 86400 seconds;
- default is not silently inferred in production coordination mode;
- login sets `sessionExpiresAt = now + configured lifetime`;
- refresh never extends this deadline.

This is an application session bound, not an upstream refresh-token lifetime claim.

## Coordination configuration

Add private frontend configuration:

```text
IDENTITY_REFRESH_COORDINATION_URL
IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS
```

`IDENTITY_REFRESH_COORDINATION_URL` accepts `valkey://`, `valkeys://`, and the existing compatible `redis://` / `rediss://` schemes supported by the selected client.

Modes:

1. **Coordination disabled**: URL absent. Keep the current process-local single-flight behavior and retain the documented one-active-replica production constraint.
2. **Coordination enabled**: URL present. V2 bounded sessions and shared Valkey coordination are mandatory for refresh. Multi-replica browser-session traffic is permitted only in this mode.

The application must never silently fall back from enabled shared coordination to process-local refresh when Valkey is unavailable. Such a fallback would recreate the exact cross-replica race this design is intended to remove.

## Valkey client selection

Use `iovalkey` for the first implementation.

Rationale:

- it is maintained under the `valkey-io` organization;
- it is TypeScript-based and provides official type declarations;
- its package metadata supports Node.js `>=18.12.0`, which includes the repository's Node 24 runtime;
- it supports Valkey URLs, TLS, Lua scripting, Cluster, and Sentinel;
- it adds no native runtime requirement to the current `node:24-alpine` frontend image;
- the required coordination primitive is a small set of string operations and atomic scripts, so the larger native GLIDE stack is not required for this change.

The implementation must use the stable package version resolved by pnpm at implementation time and commit the resulting lockfile. The design does not require a Valkey server upgrade; the current Valkey 9.1.2 deployment target is sufficient.

References:

- <https://github.com/valkey-io/iovalkey>
- <https://valkey.io/commands/set/>
- <https://valkey.io/topics/distlock/>

## Coordination key design

For an old sealed session `S0`:

```text
digest = hex(SHA-256(S0))
```

The raw sealed cookie is never placed in a key name.

Use a Valkey Cluster-compatible hash tag so all keys for one old session generation occupy the same slot:

```text
memo:identity:refresh:v1:{<digest>}:attempt
memo:identity:refresh:v1:{<digest>}:lease
memo:identity:refresh:v1:{<digest>}:result
```

Values:

- `attempt`: random 128-bit-or-stronger owner token. Its existence means an upstream redemption may already have been attempted for this old session generation.
- `lease`: the same owner token with a short TTL. Its existence tells followers that the leader is expected to still be working.
- `result`: the new **sealed session string** only. It contains encrypted/authenticated session state, not plaintext access or refresh credentials.

No key or value contains raw issuer credentials, client secrets, access tokens, refresh tokens, subject identifiers, authorization codes, or raw cookies.

## TTLs

### Attempt replay guard

The attempt marker TTL is derived from the old session's remaining logical lifetime:

```text
max(1 second, sessionExpiresAt - now) + 60-second cleanup margin
```

This ensures the application never retries the old refresh credential while the old sealed session could still be accepted as logically valid.

### Lease

Use a 15-second lease.

Reasoning:

- the existing identity HTTP deadline is 10 seconds;
- 15 seconds leaves bounded scheduling/serialization margin;
- lease expiry is not permission to retry because the attempt marker remains.

### Result handoff

Keep the sealed success result for 120 seconds.

This is long enough for ordinary concurrent/follower requests to adopt the new cookie without retaining every old-to-new mapping for the full browser session lifetime. After result expiry, an old S0 cookie still cannot refresh again because its attempt marker remains; it fails closed and requires re-login.

## Atomic operations

Use small server-side Lua scripts through `iovalkey` so acquisition/publication decisions are atomic and all touched keys share one Cluster hash slot.

### Acquire script

Given `attempt`, `lease`, and `result` keys:

1. if `result` exists, return `RESULT` plus the sealed result;
2. if `attempt` exists and `lease` exists, return `WAIT`;
3. if `attempt` exists and `lease` does not exist, return `INDETERMINATE`;
4. otherwise set `attempt=<owner>` with replay-guard TTL and `lease=<owner>` with 15-second TTL, then return `LEADER`.

The attempt and lease writes happen atomically.

### Publish-success script

The leader may publish only while the `attempt` marker still contains its owner token.

The script:

1. checks `attempt == owner`;
2. stores `result=<sealed S1>` with 120-second TTL;
3. deletes `lease` only when it still equals the same owner;
4. keeps `attempt` until the replay-guard TTL expires.

A leader whose attempt ownership has expired must not publish a success result into a new coordination generation.

### Failure completion

On any upstream refresh error, network timeout, response parse failure, sealing failure, or uncertain completion:

- do not delete the attempt marker;
- remove the lease only when owned by the current leader;
- do not retry the same old refresh credential;
- clear the browser session for that request.

This deliberately converts ambiguous refresh state into re-authentication instead of duplicate redemption.

## Refresh state machine

```text
VALID_ACCESS_TOKEN
    |
    | enters 30s refresh window
    v
NEEDS_REFRESH
    |
    | acquire shared coordination
    +-------------------------------+
    |                               |
    v                               v
LEADER                            FOLLOWER
    |                               |
    | mark attempt before            | RESULT -> adopt S1
    | upstream request               | WAIT   -> bounded poll
    v                               | INDETERMINATE -> clear session
UPSTREAM_REFRESH                    |
    |                               |
    +--> success -> seal S1 ---------+
    |               publish result
    |               set cookie
    |
    +--> any failure/ambiguity
                    |
                    v
              CLEAR SESSION
```

## Follower waiting behavior

Followers that receive `WAIT` perform bounded polling for the result.

Requirements:

- total follower wait is capped below the normal frontend request budget;
- proposed maximum is 12 seconds;
- use short jittered intervals rather than a tight loop;
- each poll reads only the coordination keys for the same digest;
- if `result` appears, validate/decrypt it with the normal session code before use;
- if the lease disappears without a result, return `INDETERMINATE` immediately;
- if the wait deadline is reached while the lease still exists, fail closed for that request and do not call the upstream token endpoint.

A timed-out follower does not cancel the leader's refresh.

## Valkey outage behavior

Coordination-enabled mode distinguishes whether the current access token is still valid.

### Access token still valid

If coordination is unavailable while the access token is inside the early-refresh window but `accessTokenExpiresAt > now`:

- do not call the upstream refresh endpoint;
- continue the current request with the still-valid existing access token;
- do not rewrite the session cookie;
- emit a low-cardinality coordination-unavailable outcome;
- allow a later request to retry coordination.

This is safe because no refresh credential is consumed.

### Access token expired

If coordination is unavailable and the access token is already expired:

- do not call the upstream refresh endpoint;
- clear the session cookie;
- expose unauthenticated state;
- require re-login.

This preserves fail-closed behavior and avoids uncoordinated refresh.

### Indeterminate attempt

If an attempt marker exists without a usable result and without an active lease, clear the session immediately. Do not temporarily reuse the old access token even if it has a few seconds remaining. Once an ambiguous redemption may have happened, the old refresh state is not a safe basis for continued session mutation.

## Security properties

The design must preserve these properties:

- browser JavaScript never receives access or refresh credentials;
- Valkey never stores plaintext access or refresh credentials;
- the old sealed cookie is never used directly as a Valkey key;
- coordination identifiers are SHA-256 digests of already-random authenticated ciphertext;
- owner tokens are generated with cryptographically secure randomness;
- upstream refresh is started only after the attempt marker exists;
- no code path deletes the attempt marker merely because a request failed;
- no code path retries an old refresh credential after an attempt marker has been observed;
- public/browser error responses remain generic and provider-neutral;
- internal logs never include the sealed session, digest, tokens, client secret, or result ciphertext;
- Valkey TLS (`valkeys://`/`rediss://`) is required when the coordination network is not a trusted private network.

## Valkey operational requirements

Multi-replica correctness depends on the shared coordination store being operated as a correctness dependency, not as an evictable cache.

Required production properties:

- no eviction of coordination keys before their TTL;
- bounded command/connect timeouts;
- authentication/ACL appropriate to the deployment;
- private-network placement or TLS;
- persistence/HA chosen consistently with the deployment's availability objectives;
- one logical write authority for a coordination key at a time.

The application must document that a configuration allowing volatile-key eviction under memory pressure is unsupported for multi-replica refresh correctness.

Valkey failover may still create an ambiguous external-refresh window. The application response remains fail closed: it must prefer re-login over replaying an old refresh token.

## Component boundaries

### `session.ts`

Owns:

- V1/V2 session parsing;
- V2 `sessionExpiresAt` validation;
- sealing/opening session ciphertext;
- schema compatibility rules.

It does not know about Valkey.

### `refreshCoordinator.ts` (new)

Owns:

- digest/key derivation;
- Valkey client boundary;
- acquire/publish/failure scripts;
- follower wait logic;
- fixed outcome types.

Suggested public interface:

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

The exact internal class/function names may vary, but callers must not depend on Valkey commands or key names directly.

### `hooks.server.ts`

Owns orchestration only:

- open current session;
- decide whether refresh is required;
- call the coordinator;
- leader performs OIDC refresh;
- followers consume a published sealed result;
- set/clear cookies and locals.

The existing process-local map remains only for coordination-disabled mode and may also be used as an optimization inside one process in front of the shared coordinator, provided it cannot bypass shared correctness.

### `config.ts`

Owns validation for:

- optional coordination URL;
- required browser session max age when coordination is enabled;
- URL scheme restrictions;
- numeric bounds.

No raw coordination URL is emitted to public error responses.

## Observability

Add low-cardinality internal counters/events with fixed labels only.

Suggested outcomes:

```text
identity_refresh_coordination_outcome=
  leader
  follower_result
  follower_wait_timeout
  indeterminate
  unavailable_valid_token_fallback
  unavailable_expired_session
  upstream_refresh_failed
  publish_failed
  success
```

Do not log:

- digest values;
- owner values;
- sealed result values;
- token values;
- subject/client identifiers;
- Valkey URI credentials.

The existing identity/JWKS observability privacy model remains the reference for output hygiene.

## Failure modes

| Failure | Required behavior |
| --- | --- |
| Two replicas receive S0 concurrently | one leader refreshes; follower adopts S1 |
| Leader crashes before upstream call | attempt remains; after lease loss session becomes indeterminate and re-login is required |
| Leader crashes during/after upstream redemption | attempt remains; no retry of S0; re-login unless S1 was published |
| Token endpoint times out | ambiguous; keep attempt marker; clear session |
| Valkey unavailable, token still valid | use current token for current request only; no refresh |
| Valkey unavailable, token expired | clear session; no refresh |
| Result cannot be decrypted/validated | clear session; do not retry old refresh |
| Publish fails after successful upstream refresh | treat as ambiguous; clear session; no retry old refresh |
| Lease expires while leader still running | followers may fail closed; leader may publish only while attempt ownership still matches |
| Attempt TTL expires after session lifetime | old session is already invalid by `sessionExpiresAt`; no replay path remains |
| Valkey eviction removes attempt early | unsupported production configuration; correctness no longer guaranteed |

## Testing strategy

Tests must prove failure-detection capability, not only code coverage.

### Unit tests

Cover:

- V2 session lifetime validation;
- legacy-session behavior in enabled/disabled coordination modes;
- key derivation never includes raw sealed cookie text;
- fixed TTL calculations;
- configuration scheme/range validation;
- failure classification without sensitive details.

### Real-Valkey integration tests

Use an actual Valkey 9.1.2 container.

Required regressions:

1. **Two independent coordinator instances, same S0**  
   Assert the upstream refresh stub is called exactly once and both logical replicas obtain the same refreshed access-token state.

2. **Leader crash before result publication**  
   Create attempt + lease, simulate upstream redemption, remove/expire lease without result, then assert a second coordinator returns `indeterminate` and never invokes upstream refresh.

3. **Follower waits then consumes result**  
   A follower observing `WAIT` must receive S1 after leader publication.

4. **Result expires but replay guard remains**  
   After result TTL expiry, the same S0 must not become leader again; it must fail closed.

5. **Coordination unavailable with valid token**  
   Current valid access token is used without calling upstream refresh.

6. **Coordination unavailable with expired token**  
   Session is cleared and upstream refresh is not called.

7. **Publish failure after upstream success**  
   No second redemption is attempted with S0.

8. **Session expiry bounds replay state**  
   Once V2 `sessionExpiresAt` has passed, S0 is rejected before refresh coordination.

### Browser E2E

Extend the provider-neutral browser E2E with a deterministic multi-replica refresh scenario using two logical SvelteKit request contexts against one Valkey instance and a strict-rotation identity fixture.

The fixture must reject reuse of the same refresh token. The test passes only when:

- exactly one refresh redemption occurs;
- both concurrent requests converge on the refreshed session where result handoff is available;
- browser-readable surfaces still contain no access/refresh credentials;
- no expired token is forwarded after refresh failure.

## CI design

Add a dedicated lightweight workflow/job for shared refresh coordination rather than relying only on mocked frontend unit tests.

The job must:

1. start Valkey 9.1.2;
2. install frontend dependencies with the repository's pinned pnpm toolchain;
3. run the dedicated coordination integration tests;
4. run relevant TypeScript/Svelte checks;
5. fail if the real-Valkey regressions fail.

The normal frontend/unit/build jobs remain unchanged except for the new dependency and source files. Full-stack smoke continues to protect the existing single-replica development path.

Before merging the implementation PR, required verification is:

- `pnpm check`;
- `pnpm lint`;
- `pnpm test:unit -- --run`;
- dedicated real-Valkey coordination integration tests;
- provider-neutral auth E2E;
- `pnpm build`;
- container-definition validation/build;
- full CI green.

## Deployment and migration

Rollout is staged.

### Stage 1: code deployed, coordination disabled

- introduce V2-capable session parsing and coordinator code;
- keep `IDENTITY_REFRESH_COORDINATION_URL` absent;
- retain one-active-replica policy;
- verify no regression in current browser auth.

### Stage 2: coordination service validation

- deploy production Valkey coordination endpoint with no-eviction policy;
- validate connectivity, ACL/TLS, command timeout, and failure telemetry;
- do not horizontally scale yet.

### Stage 3: enable shared coordination on one replica

- configure `IDENTITY_REFRESH_COORDINATION_URL` and `IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS`;
- legacy sessions are cleared and users re-login into V2 sessions;
- validate successful refresh, outage fallback, and indeterminate fail-closed behavior.

### Stage 4: multi-replica canary

- add a second active BFF replica;
- generate concurrent near-expiry traffic;
- verify exactly-once logical refresh and result handoff;
- monitor coordination-unavailable, indeterminate, upstream failure, and login-rate changes.

### Stage 5: normal multi-replica operation

- remove the one-active-replica operational restriction from `docs/browser-auth-operations.md` only after canary acceptance succeeds;
- retain a documented rollback path to one replica.

## Rollback

If shared coordination causes operational problems:

1. reduce browser-session traffic to one active BFF replica;
2. disable `IDENTITY_REFRESH_COORDINATION_URL`;
3. continue using current sealed-session behavior with process-local single-flight;
4. do not run multiple active replicas in disabled mode;
5. keep backend JWT/JWKS configuration unchanged.

V2 sessions remain valid in single-replica mode, so rollback does not require token or memo-data migration.

## Compatibility

### Backend API

No change.

### Memo data

No change.

### JWT/JWKS contract

No change.

### Browser cookie

Same cookie name and security attributes. The encrypted payload gains V2 schema/lifetime fields. Enabling shared coordination intentionally invalidates legacy unbounded sessions once so replay-guard lifetime can be proven.

### Development environment

Current Compose/local development may keep shared coordination disabled initially. A dedicated integration test uses the existing Valkey service/version to exercise coordination.

## Acceptance criteria

The implementation is accepted only if all of the following are demonstrated:

- two independent BFF coordination instances cannot redeem the same old refresh credential twice;
- attempt state is persisted before the upstream refresh request begins;
- ambiguous leader loss never permits a retry with the same old credential;
- followers can consume a successful sealed result;
- no plaintext access/refresh credential is stored in Valkey;
- expired/indeterminate sessions fail closed;
- transient coordination failure does not unnecessarily reject a still-valid access token before its expiry;
- coordination-disabled mode preserves existing single-replica behavior;
- coordination-enabled mode never silently falls back to process-local-only refresh;
- tests exercise a real Valkey instance and a strict single-use refresh-token fixture;
- all existing provider-neutral auth, frontend, container, and full-stack checks remain green;
- `docs/browser-auth-operations.md` is updated only when multi-replica acceptance has actually been proven.

## Rejected alternatives

### Full server-side session store now

Rejected for this change because it expands every authenticated request's dependency surface and requires a larger session-lifecycle migration than needed to solve refresh duplication.

### Distributed lock without a persistent attempt marker

Rejected because a lease expiry after an ambiguous upstream request would allow another replica to retry the same rotating refresh token.

### Process-local lock plus sticky sessions

Rejected because failover, deployment replacement, or routing changes can move concurrent requests across replicas.

### Provider-specific refresh reuse/grace

Rejected because it violates the provider-neutral application contract and cannot provide portable correctness.

### Retry refresh after timeout

Rejected because a timeout does not prove the identity provider failed to redeem the token. Retrying could reuse a consumed credential.

## Design decision summary

The implementation will preserve the current encrypted browser cookie model and add a correctness-focused Valkey coordination layer only around refresh. The key invariant is stronger than ordinary duplicate suppression:

> Once an old sealed session generation has begun an upstream refresh attempt, that old refresh credential is never attempted again.

A short lease indicates whether a leader is likely still running. A longer attempt marker prevents replay after ambiguous completion. A short sealed-result handoff lets concurrent followers converge on the leader's refreshed session. A bounded V2 browser-session lifetime makes the replay marker safely garbage-collectable.

This trades occasional re-login under ambiguous infrastructure failure for deterministic avoidance of rotating refresh-token reuse, while leaving the backend identity contract and memo-domain architecture unchanged.
