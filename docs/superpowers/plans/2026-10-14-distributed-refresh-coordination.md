# Distributed Browser Refresh Coordination Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow multiple active SvelteKit BFF replicas to refresh one rotating browser refresh credential without duplicate redemption, while preserving the provider-neutral identity boundary and fail-closed behavior.

**Architecture:** Keep the encrypted browser cookie as session state and use Valkey only for refresh coordination. A bounded V2 session lifetime makes replay guards collectable; HKDF/HMAC domain separation binds coordination state to one sealed-session generation; atomic Valkey scripts persist an attempt before upstream refresh and never permit the old refresh credential to be retried after an ambiguous outcome.

**Tech Stack:** SvelteKit 2, Svelte 5, TypeScript, Web Crypto, Vitest, Playwright, Valkey 9.1.2, `iovalkey`, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-10-14-distributed-refresh-coordination-design.md` plus the superseding timing amendment `docs/superpowers/specs/2026-10-14-distributed-refresh-coordination-timeout-amendment.md`.

## Global Constraints

- Preserve the existing backend JWT/JWKS contract and memo-domain APIs.
- Shared coordination is enabled only when `IDENTITY_REFRESH_COORDINATION_URL` is present.
- `IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS` is required in shared-coordination mode and must be an integer in `900..=86400`.
- Accepted coordination URL schemes are `valkey://`, `valkeys://`, `redis://`, and `rediss://`.
- Session V2 has fixed `sessionExpiresAt`; refresh never extends it.
- Attempt replay guard TTL is `max(1000 ms, sessionExpiresAt - now) + 60000 ms`.
- Leader lease TTL is exactly `30000 ms`.
- Result handoff TTL is exactly `120000 ms`.
- Follower wait deadline is exactly `25000 ms`; follower timeout never grants leadership.
- Identity discovery and token requests each retain their existing 10000 ms HTTP deadline.
- Once an attempt marker exists for S0, S0's refresh credential is never redeemed again.
- Coordination-enabled mode must never fall back to process-local-only refresh.
- Valkey must never receive plaintext access tokens, refresh tokens, client secrets, raw browser cookies, or subject identifiers.
- Coordination identifiers use HMAC-SHA-256 over the sealed session with an HKDF-SHA-256-derived key.
- Result handoff MACs are bound to the exact coordination identifier and verified in constant time.
- The current one-active-replica operational restriction remains until a real deployment canary has verified the shared path.

## File Structure

- Modify `frontend/src/lib/server/identity/config.ts` and `.test.ts` — shared-coordination config and bounded session lifetime.
- Modify `frontend/src/lib/server/identity/session.ts` and `.test.ts` — V1/V2 session types and V2 expiry.
- Create `frontend/src/lib/server/identity/refreshCoordinationCrypto.ts` and `.test.ts` — HKDF/HMAC domain separation and result-envelope authentication.
- Create `frontend/src/lib/server/identity/valkeyRefreshCoordinationStore.ts` and `.integration.test.ts` — `iovalkey` connection plus atomic Lua scripts.
- Create `frontend/src/lib/server/identity/refreshCoordinator.ts` and `.test.ts` — refresh state machine and bounded follower wait.
- Modify `frontend/src/lib/server/identity/browserAuth.ts` and `frontend/src/routes/auth/authRoutes.test.ts` — V2 creation at login.
- Modify `frontend/src/hooks.server.ts`, `hooks.server.test.ts`, and `hooks.server.concurrent.test.ts` — enabled/disabled orchestration and failure semantics.
- Create `frontend/src/lib/server/identity/refreshCoordinator.multiReplica.integration.test.ts` — two independent coordinators against real Valkey with strict single-use refresh behavior.
- Modify `frontend/package.json` and `frontend/pnpm-lock.yaml` — add `iovalkey` and integration-test script.
- Create `.github/workflows/identity-refresh-coordination.yml` — dedicated real-Valkey gate.
- Modify `docs/browser-auth-operations.md` — deployment/canary/rollback constraints without prematurely removing the one-replica restriction.
- Modify `README.md` only if the final implementation exposes the new private frontend configuration there.

## Review Focus

1. **Serial slow identity requests:** discovery can consume nearly 10 seconds and token refresh another nearly 10 seconds; a healthy leader must remain leased long enough to publish the result.
2. **Leader lease loss after redemption may have started:** no follower may become leader; the persistent attempt marker forces indeterminate/fail-closed behavior.
3. **Malformed or attacker-written result envelope:** wrong MAC, wrong coordination generation, malformed JSON, or invalid sealed session must never trigger an S0 retry.
4. **Legacy V1 cookie on a coordination-enabled replica:** clear it before Valkey/upstream token calls and require re-login.
5. **Valkey connection/command stall:** bounded failure must become `unavailable`; a still-valid access token may continue only without refresh or cookie mutation.

---

### Task 1: Add bounded session V2 and coordination configuration

**Files:**
- Modify: `frontend/src/lib/server/identity/config.ts`
- Modify: `frontend/src/lib/server/identity/config.test.ts`
- Modify: `frontend/src/lib/server/identity/session.ts`
- Modify: `frontend/src/lib/server/identity/session.test.ts`
- Modify: `frontend/src/lib/server/identity/browserAuth.ts`
- Modify: `frontend/src/routes/auth/authRoutes.test.ts`

**Interfaces:**
- Produces `IdentityConfig.refreshCoordinationUrl?: URL`.
- Produces `IdentityConfig.browserSessionMaxAgeSeconds?: number`.
- Produces `IdentitySessionV1`, `IdentitySessionV2`, and `IdentitySession = IdentitySessionV1 | IdentitySessionV2`.
- Produces `isIdentitySessionV2(session: IdentitySession): session is IdentitySessionV2`.
- Changes `openSession(value: string, key: Uint8Array, now?: number): Promise<IdentitySession | null>` so V2 expiry is checked at open time.
- `completeLogin()` emits V2 when bounded-session configuration is present; otherwise it preserves V1 behavior.

- [ ] **Step 1: Write failing configuration tests**

Add tests named:

```ts
it('requires bounded browser session age when refresh coordination is enabled', ...)
it.each(['899', '86401', '900.5', 'abc'])('rejects invalid browser session age %s', ...)
it.each(['http://valkey.test', 'https://valkey.test', 'file:///tmp/valkey'])('rejects unsupported coordination URL %s', ...)
it.each(['valkey://valkey:6379', 'valkeys://valkey.example.test:6379', 'redis://valkey:6379', 'rediss://valkey.example.test:6379'])('accepts supported coordination URL %s', ...)
```

Assert accepted age boundaries are exactly 900 and 86400 seconds.

- [ ] **Step 2: Run the configuration tests and verify RED**

Run: `cd frontend && pnpm test:unit -- --run src/lib/server/identity/config.test.ts`

Expected: FAIL because coordination/session-age fields are not implemented.

- [ ] **Step 3: Implement configuration parsing**

Extend `IdentityConfig` with the two optional fields. Parse only the four allowed URL schemes. Do not expose the raw URL in `IdentityConfigurationError`. Require the bounded age whenever coordination URL is present.

- [ ] **Step 4: Run configuration tests and verify GREEN**

Run the Step 2 command. Expected: PASS.

- [ ] **Step 5: Write failing V2 session tests**

Add:

```ts
it('round-trips a bounded v2 identity session before expiry', ...)
it('rejects a v2 identity session at sessionExpiresAt', ...)
it('continues to open a legacy v1 session', ...)
it('rejects malformed v2 lifetime metadata', ...)
```

Exact rule: `sessionExpiresAt > now` is valid; `sessionExpiresAt <= now` is invalid.

- [ ] **Step 6: Run V2 tests and verify RED**

Run: `cd frontend && pnpm test:unit -- --run src/lib/server/identity/session.test.ts`

Expected: FAIL because V2 does not exist.

- [ ] **Step 7: Implement V1/V2 parsing and expiry**

Keep the current AES-256-GCM cookie envelope version unchanged; only the JSON session payload evolves.

- [ ] **Step 8: Add login-route RED/GREEN coverage**

Assert that a configured bounded session produces:

```ts
{
  schemaVersion: 2,
  sessionExpiresAt: now + maxAgeSeconds * 1000,
  accessToken,
  accessTokenExpiresAt,
  refreshToken
}
```

and that absent bounded-session configuration preserves the legacy V1 shape.

Run: `cd frontend && pnpm test:unit -- --run src/routes/auth/authRoutes.test.ts`

Implement only the minimal conditional V2 creation needed to pass.

- [ ] **Step 9: Run all Task 1 focused tests**

```bash
cd frontend
pnpm test:unit -- --run src/lib/server/identity/config.test.ts src/lib/server/identity/session.test.ts src/routes/auth/authRoutes.test.ts
```

Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add frontend/src/lib/server/identity/config.ts frontend/src/lib/server/identity/config.test.ts \
  frontend/src/lib/server/identity/session.ts frontend/src/lib/server/identity/session.test.ts \
  frontend/src/lib/server/identity/browserAuth.ts frontend/src/routes/auth/authRoutes.test.ts
git commit -m "feat: add bounded browser identity sessions"
```

### Task 2: Add coordination cryptographic domain separation

**Files:**
- Create: `frontend/src/lib/server/identity/refreshCoordinationCrypto.ts`
- Create: `frontend/src/lib/server/identity/refreshCoordinationCrypto.test.ts`

**Interfaces:**
- Produces `deriveCoordinationId(sealedSession: string, sessionKey: Uint8Array): Promise<string>`.
- Produces `createRefreshResultEnvelope(coordinationId: string, sealedSession: string, sessionKey: Uint8Array): Promise<string>`.
- Produces `verifyRefreshResultEnvelope(coordinationId: string, envelope: string, sessionKey: Uint8Array): Promise<string | null>`.
- HKDF-SHA-256 `info` values are exactly `memo_server/identity-refresh/v1/key-id` and `memo_server/identity-refresh/v1/result-mac`.

- [ ] **Step 1: Write failing crypto contract tests**

Assert:

```ts
coordinationId matches /^[0-9a-f]{64}$/
coordinationId does not expose S0
S0 and S1 produce different IDs
a valid envelope verifies only under its original coordination ID
tampered/malformed envelopes return null
key-id and result-MAC derived keys are domain-separated
```

- [ ] **Step 2: Run tests and verify RED**

Run: `cd frontend && pnpm test:unit -- --run src/lib/server/identity/refreshCoordinationCrypto.test.ts`

Expected: module-not-found/failing contract.

- [ ] **Step 3: Implement the three crypto functions**

Use Web Crypto HKDF/HMAC only. MAC input is exactly `"v1\0" + coordinationId + "\0" + sealedSession`. Verify with Web Crypto `verify`, not string equality.

- [ ] **Step 4: Run tests and verify GREEN**

Run the Step 2 command. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/server/identity/refreshCoordinationCrypto.ts frontend/src/lib/server/identity/refreshCoordinationCrypto.test.ts
git commit -m "feat: bind refresh coordination cryptographically"
```

### Task 3: Add the atomic Valkey coordination store

**Files:**
- Modify: `frontend/package.json`
- Modify: `frontend/pnpm-lock.yaml`
- Create: `frontend/src/lib/server/identity/valkeyRefreshCoordinationStore.ts`
- Create: `frontend/src/lib/server/identity/valkeyRefreshCoordinationStore.integration.test.ts`

**Interfaces:**

```ts
type StoreAcquireResult =
  | { kind: 'leader' }
  | { kind: 'wait' }
  | { kind: 'result'; envelope: string }
  | { kind: 'indeterminate' };

interface RefreshCoordinationStore {
  acquire(input: { coordinationId: string; owner: string; attemptTtlMs: number; leaseTtlMs: number }): Promise<StoreAcquireResult>;
  publishSuccess(input: { coordinationId: string; owner: string; envelope: string; resultTtlMs: number }): Promise<boolean>;
  completeFailure(input: { coordinationId: string; owner: string }): Promise<void>;
  close(): Promise<void>;
}

function createValkeyRefreshCoordinationStore(url: URL): RefreshCoordinationStore;
```

Client options are bounded: `connectTimeout: 1000`, `commandTimeout: 1000`, `maxRetriesPerRequest: 1`, `enableOfflineQueue: false`, and lazy connection.

- [ ] **Step 1: Add `iovalkey`**

Run: `cd frontend && pnpm add iovalkey`

Expected: only `package.json` and lockfile dependency resolution change.

- [ ] **Step 2: Write failing real-Valkey tests**

With `VALKEY_TEST_URL` set, cover:

```ts
first acquire -> leader
second acquire while lease exists -> wait
publishSuccess -> result
lease disappears while attempt remains -> indeterminate
wrong owner cannot publish
completeFailure removes only matching lease and retains attempt
```

- [ ] **Step 3: Start Valkey and verify RED**

```bash
docker run -d --rm --name memo-refresh-valkey -p 16379:6379 valkey/valkey:9.1.2-alpine
cd frontend
VALKEY_TEST_URL=valkey://127.0.0.1:16379 pnpm test:unit -- --run src/lib/server/identity/valkeyRefreshCoordinationStore.integration.test.ts
```

Expected: FAIL because store implementation is absent.

- [ ] **Step 4: Implement the atomic scripts**

Keys are exactly:

```text
memo:identity:refresh:v1:{<coordinationId>}:attempt
memo:identity:refresh:v1:{<coordinationId>}:lease
memo:identity:refresh:v1:{<coordinationId>}:result
```

Acquire atomically checks result/attempt/lease before writing attempt+lease. Publish verifies `attempt == owner`. Failure never deletes attempt.

- [ ] **Step 5: Add unavailable/timeout regression**

Use an unreachable local port and assert bounded rejection; never assert dynamic client error text.

- [ ] **Step 6: Run the real-Valkey suite and verify GREEN**

Repeat Step 3. Expected: PASS. Then `docker rm -f memo-refresh-valkey`.

- [ ] **Step 7: Commit**

```bash
git add frontend/package.json frontend/pnpm-lock.yaml \
  frontend/src/lib/server/identity/valkeyRefreshCoordinationStore.ts \
  frontend/src/lib/server/identity/valkeyRefreshCoordinationStore.integration.test.ts
git commit -m "feat: add atomic Valkey refresh coordination store"
```

### Task 4: Implement the refresh coordinator state machine

**Files:**
- Create: `frontend/src/lib/server/identity/refreshCoordinator.ts`
- Create: `frontend/src/lib/server/identity/refreshCoordinator.test.ts`

**Interfaces:**

```ts
type RefreshCoordinationDecision =
  | { kind: 'leader'; owner: string }
  | { kind: 'result'; sealedSession: string }
  | { kind: 'indeterminate'; reason: 'leader_lost' | 'wait_timeout' | 'invalid_result' }
  | { kind: 'unavailable' };

interface RefreshCoordinator {
  acquire(input: { sealedSession: string; replayGuardMs: number }): Promise<RefreshCoordinationDecision>;
  publishSuccess(input: { sealedSession: string; owner: string; refreshedSealedSession: string }): Promise<boolean>;
  completeFailure(input: { sealedSession: string; owner: string }): Promise<void>;
  close(): Promise<void>;
}

function createRefreshCoordinator(input: {
  store: RefreshCoordinationStore;
  sessionKey: Uint8Array;
  sleep?: (ms: number) => Promise<void>;
  random?: () => number;
  now?: () => number;
}): RefreshCoordinator;
```

Constants: lease `30000 ms`, result TTL `120000 ms`, follower deadline `25000 ms`, poll delay `100 + floor(random() * 101)` ms.

- [ ] **Step 1: Write failing state-machine tests**

Use an in-memory fake store/clock/sleeper. Cover:

```ts
first acquire returns leader
WAIT then result returns verified S1
lease loss returns leader_lost
25000 ms wait deadline returns wait_timeout and never leadership
invalid/cross-generation result returns invalid_result
store exception returns unavailable
publishSuccess uses exactly 120000 ms result TTL
```

- [ ] **Step 2: Add timing-amendment regressions**

Create deterministic tests proving:

```ts
leader still considered active after 15001 ms
follower can receive result after >20000 ms and <25000 ms
follower timeout at 25000 ms never becomes leader
lease loss at/after 30000 ms becomes indeterminate while attempt remains
```

No production/test constant may retain the superseded `15000` lease or `12000` follower deadline.

- [ ] **Step 3: Run tests and verify RED**

Run: `cd frontend && pnpm test:unit -- --run src/lib/server/identity/refreshCoordinator.test.ts`

Expected: FAIL because coordinator implementation is absent.

- [ ] **Step 4: Implement coordinator logic**

Generate a 16-byte cryptographically random owner. `acquire()` owns polling; callers never see raw `wait`. Store/client exceptions become `{ kind: 'unavailable' }` without dynamic error details.

- [ ] **Step 5: Run tests and verify GREEN**

Run the Step 3 command. Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/lib/server/identity/refreshCoordinator.ts frontend/src/lib/server/identity/refreshCoordinator.test.ts
git commit -m "feat: add distributed refresh coordinator state machine"
```

### Task 5: Integrate coordination into SvelteKit session restoration

**Files:**
- Modify: `frontend/src/hooks.server.ts`
- Modify: `frontend/src/hooks.server.test.ts`
- Modify: `frontend/src/hooks.server.concurrent.test.ts`

**Interfaces:**
- Consumes Task 1 V2/config and Task 4 `RefreshCoordinator`.
- Add optional injected coordinator to `restoreIdentitySession()` for tests; production lazily creates one process-shared coordinator from the configured URL.
- Disabled mode preserves existing process-local `refreshFlights` behavior.

- [ ] **Step 1: Write failing admission tests**

Cover:

```ts
legacy V1 + enabled coordination -> cookie cleared; zero coordinator/upstream calls
V2 outside refresh window -> no Valkey access
leader refresh preserves original sessionExpiresAt
```

- [ ] **Step 2: Run hook tests and verify RED**

Run: `cd frontend && pnpm test:unit -- --run src/hooks.server.test.ts src/hooks.server.concurrent.test.ts`

Expected: new enabled-mode cases FAIL.

- [ ] **Step 3: Implement enabled/disabled admission**

Keep normal non-refresh requests independent of Valkey. Enabled mode accepts only V2 for refresh-capable session restoration.

- [ ] **Step 4: Write leader/result/failure tests**

Cover:

```ts
leader -> one upstream refresh -> publish -> set S1 -> authenticated locals
result -> no upstream refresh -> adopt S1
publish false after upstream success -> clear session
upstream timeout/error -> completeFailure -> clear session
```

Refreshed V2 retains the original `sessionExpiresAt` exactly.

- [ ] **Step 5: Implement orchestration and verify GREEN**

Only `leader` may call `discoverIdentity()` and `refreshAccessToken()`. No retry after upstream error or publish failure.

- [ ] **Step 6: Write Valkey-unavailable boundary tests**

Exact assertions:

```ts
accessTokenExpiresAt > now -> current request keeps old token; no refresh; no cookie rewrite
accessTokenExpiresAt <= now -> clear session; no refresh
```

Also test `accessTokenExpiresAt = now + 1` to pin the validity boundary.

- [ ] **Step 7: Preserve disabled-mode regression**

The existing process-local singleflight test must still pass with coordination absent and keep its existing discovery/token call counts.

- [ ] **Step 8: Run all frontend unit tests**

Run: `cd frontend && pnpm test:unit -- --run`

Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/hooks.server.ts frontend/src/hooks.server.test.ts frontend/src/hooks.server.concurrent.test.ts
git commit -m "feat: coordinate browser refresh across replicas"
```

### Task 6: Prove multi-replica correctness against real Valkey

**Files:**
- Create: `frontend/src/lib/server/identity/refreshCoordinator.multiReplica.integration.test.ts`
- Modify: `frontend/package.json`

**Interfaces:**
- Create two independent coordinator/store instances sharing only one real Valkey endpoint.
- Test refresh stub is strict single-use: any second redemption of `refresh-old` fails the test.

- [ ] **Step 1: Write the two-replica regression**

Run two concurrent refresh paths from the same V2 S0 and assert:

```ts
upstreamRedemptionCount === 1
both replicas converge on access-new
both replicas converge on refresh-new
both replicas preserve the same sessionExpiresAt
```

- [ ] **Step 2: Add failure regressions**

Cover:

```ts
attempt persisted + lease removed + no result -> second replica never calls upstream
upstream success + publish failure -> later replica never redeems S0
valid result copied to another coordination generation -> rejected by MAC
result expires while attempt remains -> S0 never becomes leader again
expired V2 -> rejected before Valkey/upstream calls
```

- [ ] **Step 3: Add package script**

```json
"test:identity-refresh-integration": "vitest run src/lib/server/identity/valkeyRefreshCoordinationStore.integration.test.ts src/lib/server/identity/refreshCoordinator.multiReplica.integration.test.ts"
```

- [ ] **Step 4: Run the complete real-Valkey suite**

```bash
docker run -d --rm --name memo-refresh-valkey -p 16379:6379 valkey/valkey:9.1.2-alpine
cd frontend
VALKEY_TEST_URL=valkey://127.0.0.1:16379 pnpm test:identity-refresh-integration
docker rm -f memo-refresh-valkey
```

Expected: PASS with exactly one upstream redemption in the concurrency regression.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/server/identity/refreshCoordinator.multiReplica.integration.test.ts frontend/package.json
git commit -m "test: prove refresh coordination across replicas"
```

### Task 7: Add CI gate, operations docs, review, and merge verification

**Files:**
- Create: `.github/workflows/identity-refresh-coordination.yml`
- Modify: `docs/browser-auth-operations.md`
- Modify: `README.md` only if required for private env documentation.

**Interfaces:**
- Dedicated workflow is a correctness signal for refresh-coordination paths.
- Production one-active-replica restriction remains until a real deployment canary succeeds.

- [ ] **Step 1: Add dedicated workflow**

Trigger on identity refresh/config/session/hook/package/lockfile/workflow/operations-doc changes. Use Node 24, pnpm 9.15.9, Valkey 9.1.2, `VALKEY_TEST_URL=valkey://127.0.0.1:6379`, then run:

```bash
pnpm install --frozen-lockfile
pnpm test:identity-refresh-integration
pnpm check
pnpm lint
```

Use action versions consistent with existing workflows.

- [ ] **Step 2: Update operations documentation**

Document disabled/enabled modes, V2 re-login migration, no-eviction Valkey requirement, 30s lease / 25s follower wait, valid-vs-expired Valkey outage behavior, indeterminate re-login behavior, canary requirements, and rollback to one replica. Do not remove the current production one-replica restriction yet.

- [ ] **Step 3: Run frontend verification**

```bash
cd frontend
pnpm check
pnpm lint
pnpm test:unit -- --run
pnpm build
```

Expected: PASS.

- [ ] **Step 4: Run real-Valkey verification**

```bash
docker run -d --rm --name memo-refresh-valkey -p 16379:6379 valkey/valkey:9.1.2-alpine
cd frontend
VALKEY_TEST_URL=valkey://127.0.0.1:16379 pnpm test:identity-refresh-integration
docker rm -f memo-refresh-valkey
```

Expected: PASS.

- [ ] **Step 5: Run container verification**

From repository root:

```bash
docker compose config --quiet
docker compose build backend frontend
```

Expected: PASS.

- [ ] **Step 6: Perform full-diff code review**

Classify each finding as Confirmed / Likely / False Positive / Intentional / Unknown. Explicitly inspect:

- any enabled-mode path calling upstream refresh without persisted attempt state;
- any path deleting attempt after ambiguous failure;
- timing constants inconsistent with the 30s/25s amendment;
- second-vs-millisecond conversion errors;
- logs/errors containing raw URL credentials, cookie, token, owner, coordination ID, or result ciphertext;
- `iovalkey` retry/offline-queue behavior that violates bounded failure;
- accidental Valkey dependency outside the refresh window.

For Confirmed/Likely findings, reproduce/prove them, add regression coverage, fix, then rerun Steps 3-5.

- [ ] **Step 7: Commit**

```bash
git add .github/workflows/identity-refresh-coordination.yml docs/browser-auth-operations.md
# add README.md only if intentionally changed
git commit -m "docs: add distributed refresh rollout gate"
```

- [ ] **Step 8: Push and verify GitHub CI**

Require green relevant checks including:

- Identity refresh coordination;
- Provider-neutral auth boundary;
- Provider-neutral auth E2E;
- normal Frontend checks/build;
- container/full-stack checks triggered by the final diff.

Do not classify failures as flaky without history/reproduction evidence.

- [ ] **Step 9: Merge decision**

Merge only when all relevant checks are green, review findings are resolved, no conflict exists, and the one-active-replica production restriction remains documented. Use squash merge per repository convention.

## Execution Branch and PR Strategy

1. Merge the approved spec/plan documentation PR first.
2. Create `feat/distributed-refresh-coordination` from the then-current `main`.
3. Implement Tasks 1-7 with one logical commit per task unless a focused review-fix commit is needed.
4. Open one implementation PR referencing both the base design, timing amendment, and this plan.
5. Do not mix unrelated dependency upgrades/refactors.
6. Do not remove the production one-active-replica restriction in the implementation PR; that is a later operational change after a real two-replica canary.

## Final Acceptance

The implementation PR is ready to merge only with executable evidence that:

- one S0 generation causes at most one upstream redemption while coordination state is intact;
- attempt state exists before redemption;
- a healthy serial discovery+token refresh can run beyond 15 seconds without false lease loss;
- leader failure/timeout/publish failure never enables S0 retry;
- follower results are cryptographically bound to their coordination generation;
- Valkey outage never triggers uncoordinated refresh;
- still-valid tokens may survive transient coordination outage without session mutation;
- expired/legacy/indeterminate sessions fail closed;
- disabled mode retains current single-replica behavior;
- no credential material enters browser-readable state, Valkey plaintext, or logs;
- all existing provider-neutral auth and application CI remains green.
