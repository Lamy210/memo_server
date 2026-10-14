# Distributed Browser Refresh Coordination Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow multiple active SvelteKit BFF replicas to refresh one rotating browser refresh credential without duplicate redemption, while preserving the provider-neutral identity boundary and fail-closed behavior.

**Architecture:** Keep the encrypted browser cookie as session state and introduce Valkey only around refresh coordination. A bounded V2 session lifetime makes replay guards collectable; HKDF/HMAC-derived coordination identifiers and result binding isolate coordination cryptography from cookie encryption; atomic Valkey scripts persist an attempt before the upstream refresh and never permit the old refresh credential to be retried after an ambiguous outcome.

**Tech Stack:** SvelteKit 2, Svelte 5, TypeScript, Web Crypto, Vitest, Playwright, Valkey 9.1.2, `iovalkey`, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-10-14-distributed-refresh-coordination-design.md`

## Global Constraints

- Preserve the existing backend JWT/JWKS contract and memo-domain APIs.
- `IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS` is required in shared-coordination mode and must be an integer in `900..=86400`.
- Shared coordination is enabled only when `IDENTITY_REFRESH_COORDINATION_URL` is present.
- Accepted coordination URL schemes are `valkey://`, `valkeys://`, `redis://`, and `rediss://`.
- Session V2 has fixed `sessionExpiresAt`; refresh never extends it.
- Attempt replay guard TTL is `max(1000 ms, sessionExpiresAt - now) + 60000 ms`.
- Leader lease TTL is exactly `15000 ms`.
- Result handoff TTL is exactly `120000 ms`.
- Follower wait is bounded to `12000 ms` and never authorizes an upstream retry.
- Once an attempt marker exists for S0, S0's refresh credential is never redeemed again.
- Coordination-enabled mode must never fall back to process-local-only refresh.
- Valkey must never receive plaintext access tokens, refresh tokens, client secrets, raw browser cookies, or subject identifiers.
- Coordination identifiers use HMAC-SHA-256 over the sealed session with an HKDF-SHA-256-derived key.
- Result handoff MACs are bound to the exact coordination identifier and verified in constant time.
- `valkeys://` or `rediss://` is required when Valkey is not on a trusted private network; application code does not infer network trust.
- The current one-active-replica operational restriction remains until a deployment canary has verified the shared path.

## File Structure

- Modify `frontend/src/lib/server/identity/config.ts` — parse optional shared-coordination configuration and bounded browser-session lifetime.
- Modify `frontend/src/lib/server/identity/config.test.ts` — configuration contract regressions.
- Modify `frontend/src/lib/server/identity/session.ts` — V1/V2 session types, V2 expiry validation, compatibility behavior.
- Modify `frontend/src/lib/server/identity/session.test.ts` — session schema/expiry regressions.
- Create `frontend/src/lib/server/identity/refreshCoordinationCrypto.ts` — HKDF/HMAC domain separation, coordination IDs, authenticated result envelopes.
- Create `frontend/src/lib/server/identity/refreshCoordinationCrypto.test.ts` — cryptographic boundary regressions.
- Create `frontend/src/lib/server/identity/valkeyRefreshCoordinationStore.ts` — `iovalkey` connection and atomic Lua scripts only.
- Create `frontend/src/lib/server/identity/valkeyRefreshCoordinationStore.integration.test.ts` — real-Valkey atomicity and replay-guard tests.
- Create `frontend/src/lib/server/identity/refreshCoordinator.ts` — provider-neutral refresh coordination state machine, follower wait, fixed outcomes.
- Create `frontend/src/lib/server/identity/refreshCoordinator.test.ts` — deterministic state-machine tests with a fake store/clock/sleeper.
- Modify `frontend/src/lib/server/identity/browserAuth.ts` — create V2 sessions when bounded-session configuration is present.
- Modify `frontend/src/routes/auth/authRoutes.test.ts` — V2 login compatibility regressions.
- Modify `frontend/src/hooks.server.ts` — integrate shared coordinator without changing non-refresh request behavior.
- Modify `frontend/src/hooks.server.test.ts` — outage, expired-token, legacy-session, result-handoff behavior.
- Modify `frontend/src/hooks.server.concurrent.test.ts` — preserve process-local disabled-mode singleflight and add coordination-enabled orchestration coverage where useful.
- Create `frontend/src/lib/server/identity/refreshCoordinator.multiReplica.integration.test.ts` — two independent coordinator instances against real Valkey and a strict single-use refresh stub.
- Modify `frontend/package.json` — add `iovalkey` and a dedicated integration-test command.
- Modify `frontend/pnpm-lock.yaml` — lock the resolved `iovalkey` dependency.
- Create `.github/workflows/identity-refresh-coordination.yml` — dedicated Valkey 9.1.2 regression gate.
- Modify `docs/browser-auth-operations.md` — document coordination-enabled deployment/canary/rollback while retaining the one-active-replica restriction until canary acceptance.
- Modify `README.md` only if private frontend configuration documentation is kept there after implementation review.

## Review Focus

1. **Access token expires during a Valkey outage after the request begins:** the request may use the token only if it was still valid at the authorization handoff point; never refresh without coordination.
2. **Leader lease expires just before a successful upstream response:** followers fail closed; the leader may publish only while attempt ownership still matches; no follower becomes a second leader.
3. **Malformed or attacker-written result envelope:** MAC failure, wrong coordination generation, malformed JSON, or invalid sealed session clears/fails the session path and never retries S0.
4. **Legacy V1 cookie reaches a coordination-enabled replica:** it is cleared without contacting Valkey or the upstream token endpoint.
5. **Valkey command hangs or connection cannot be established:** bounded client timeouts produce `unavailable`; a still-valid access token may continue for that request, while an expired token is rejected.

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
- `completeLogin()` emits V2 when `browserSessionMaxAgeSeconds` is configured; otherwise it preserves V1 behavior.

- [ ] **Step 1: Write failing configuration tests**

Add tests named:

```ts
it('requires bounded browser session age when refresh coordination is enabled', ...)
it.each(['899', '86401', '900.5', 'abc'])('rejects invalid browser session age %s', ...)
it.each(['http://valkey.test', 'https://valkey.test', 'file:///tmp/valkey'])('rejects unsupported coordination URL %s', ...)
it.each(['valkey://valkey:6379', 'valkeys://valkey.example.test:6379', 'redis://valkey:6379', 'rediss://valkey.example.test:6379'])('accepts supported coordination URL %s', ...)
```

Assert that a coordination URL without `IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS` throws `IdentityConfigurationError`, and that the accepted age boundaries are exactly 900 and 86400 seconds.

- [ ] **Step 2: Run configuration tests and verify RED**

Run: `cd frontend && pnpm test:unit -- --run src/lib/server/identity/config.test.ts`

Expected: FAIL because coordination/session-age fields are not parsed yet.

- [ ] **Step 3: Implement configuration parsing**

Extend `IdentityConfig` with:

```ts
refreshCoordinationUrl?: URL;
browserSessionMaxAgeSeconds?: number;
```

Parse only the four allowed schemes. Do not expose the URL in `IdentityConfigurationError`. Require the bounded age only when coordination URL is configured.

- [ ] **Step 4: Run configuration tests and verify GREEN**

Run: `cd frontend && pnpm test:unit -- --run src/lib/server/identity/config.test.ts`

Expected: PASS.

- [ ] **Step 5: Write failing V2 session tests**

Add tests named:

```ts
it('round-trips a bounded v2 identity session before expiry', ...)
it('rejects a v2 identity session at sessionExpiresAt', ...)
it('continues to open legacy v1 sessions', ...)
it('rejects malformed v2 lifetime metadata', ...)
```

Use exact expiry semantics: `sessionExpiresAt > now` is valid; `sessionExpiresAt <= now` is invalid.

- [ ] **Step 6: Run V2 session tests and verify RED**

Run: `cd frontend && pnpm test:unit -- --run src/lib/server/identity/session.test.ts`

Expected: FAIL because V2 schema/expiry behavior does not exist.

- [ ] **Step 7: Implement V1/V2 session parsing and expiry**

Add the types and type guard from the Interfaces block. Keep existing AES-256-GCM cookie format/version unchanged; only the JSON payload schema evolves.

- [ ] **Step 8: Run V2 session tests and verify GREEN**

Run: `cd frontend && pnpm test:unit -- --run src/lib/server/identity/session.test.ts`

Expected: PASS.

- [ ] **Step 9: Write failing login-route tests for V2 creation**

Assert:

```ts
// coordination/session-age config present
openSession(cookie, key, now) -> {
  schemaVersion: 2,
  sessionExpiresAt: now + maxAgeSeconds * 1000,
  ...
}

// session-age config absent
openSession(cookie, key, now) -> legacy V1 shape
```

- [ ] **Step 10: Run auth-route tests and verify RED**

Run: `cd frontend && pnpm test:unit -- --run src/routes/auth/authRoutes.test.ts`

Expected: FAIL on the new V2 assertion only.

- [ ] **Step 11: Implement conditional V2 creation in `completeLogin()`**

Do not extend `sessionExpiresAt` on future refreshes; this task only establishes the original login deadline.

- [ ] **Step 12: Re-run focused tests**

Run:

```bash
cd frontend
pnpm test:unit -- --run src/lib/server/identity/config.test.ts src/lib/server/identity/session.test.ts src/routes/auth/authRoutes.test.ts
```

Expected: PASS.

- [ ] **Step 13: Commit Task 1**

```bash
git add frontend/src/lib/server/identity/config.ts \
  frontend/src/lib/server/identity/config.test.ts \
  frontend/src/lib/server/identity/session.ts \
  frontend/src/lib/server/identity/session.test.ts \
  frontend/src/lib/server/identity/browserAuth.ts \
  frontend/src/routes/auth/authRoutes.test.ts
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

Add tests that assert:

```ts
expect(await deriveCoordinationId('S0', key)).toMatch(/^[0-9a-f]{64}$/);
expect(await deriveCoordinationId('S0', key)).not.toContain('S0');
expect(await deriveCoordinationId('S0', key)).not.toBe(await deriveCoordinationId('S1', key));
expect(await verifyRefreshResultEnvelope(id0, envelopeForId0, key)).toBe('S1');
expect(await verifyRefreshResultEnvelope(id1, envelopeForId0, key)).toBeNull();
expect(await verifyRefreshResultEnvelope(id0, tamperedEnvelope, key)).toBeNull();
```

Also assert that the key-id and result-MAC derived outputs differ for the same root session key.

- [ ] **Step 2: Run crypto tests and verify RED**

Run: `cd frontend && pnpm test:unit -- --run src/lib/server/identity/refreshCoordinationCrypto.test.ts`

Expected: FAIL because the module does not exist.

- [ ] **Step 3: Implement the three crypto functions**

Use Web Crypto HKDF/HMAC only. Serialize the result envelope as versioned JSON. Bind the MAC input to `"v1\0" + coordinationId + "\0" + sealedSession`. Use constant-time MAC verification through Web Crypto `verify` rather than string equality.

- [ ] **Step 4: Run crypto tests and verify GREEN**

Run: `cd frontend && pnpm test:unit -- --run src/lib/server/identity/refreshCoordinationCrypto.test.ts`

Expected: PASS.

- [ ] **Step 5: Commit Task 2**

```bash
git add frontend/src/lib/server/identity/refreshCoordinationCrypto.ts \
  frontend/src/lib/server/identity/refreshCoordinationCrypto.test.ts
git commit -m "feat: bind refresh coordination cryptographically"
```

### Task 3: Add the atomic Valkey coordination store

**Files:**
- Modify: `frontend/package.json`
- Modify: `frontend/pnpm-lock.yaml`
- Create: `frontend/src/lib/server/identity/valkeyRefreshCoordinationStore.ts`
- Create: `frontend/src/lib/server/identity/valkeyRefreshCoordinationStore.integration.test.ts`

**Interfaces:**
- Consumes coordination IDs and authenticated envelope strings from Task 2.
- Produces:

```ts
type StoreAcquireResult =
  | { kind: 'leader' }
  | { kind: 'wait' }
  | { kind: 'result'; envelope: string }
  | { kind: 'indeterminate' };

interface RefreshCoordinationStore {
  acquire(input: {
    coordinationId: string;
    owner: string;
    attemptTtlMs: number;
    leaseTtlMs: number;
  }): Promise<StoreAcquireResult>;
  publishSuccess(input: {
    coordinationId: string;
    owner: string;
    envelope: string;
    resultTtlMs: number;
  }): Promise<boolean>;
  completeFailure(input: { coordinationId: string; owner: string }): Promise<void>;
  close(): Promise<void>;
}

function createValkeyRefreshCoordinationStore(url: URL): RefreshCoordinationStore;
```

- Connection/command behavior is bounded: `connectTimeout: 1000`, `commandTimeout: 1000`, `maxRetriesPerRequest: 1`, `enableOfflineQueue: false`, and lazy connection.

- [ ] **Step 1: Add `iovalkey` through pnpm**

Run: `cd frontend && pnpm add iovalkey`

Expected: `package.json` and `pnpm-lock.yaml` change; no other dependency update is intentionally introduced.

- [ ] **Step 2: Write failing real-Valkey acquire/publish tests**

Require `VALKEY_TEST_URL`; skip locally only when the variable is absent. Add tests for:

```ts
leader on first acquire
wait on second acquire while lease exists
result after publishSuccess
indeterminate after lease loss with attempt retained
publishSuccess false for wrong owner
completeFailure removes only the matching lease and retains attempt
```

- [ ] **Step 3: Start Valkey 9.1.2 and verify RED**

Run:

```bash
docker run -d --rm --name memo-refresh-valkey -p 16379:6379 valkey/valkey:9.1.2-alpine
cd frontend
VALKEY_TEST_URL=valkey://127.0.0.1:16379 pnpm test:unit -- --run src/lib/server/identity/valkeyRefreshCoordinationStore.integration.test.ts
```

Expected: FAIL because the store module does not exist.

- [ ] **Step 4: Implement atomic acquire/publish/failure scripts**

Key names are exactly:

```text
memo:identity:refresh:v1:{<coordinationId>}:attempt
memo:identity:refresh:v1:{<coordinationId>}:lease
memo:identity:refresh:v1:{<coordinationId>}:result
```

Acquire atomically checks result, attempt, and lease before writing attempt+lease. Publish verifies `attempt == owner` before writing result and conditionally removing the matching lease. Failure never removes attempt.

- [ ] **Step 5: Run real-Valkey integration tests and verify GREEN**

Run the Step 3 test command again.

Expected: PASS.

- [ ] **Step 6: Verify timeout/unavailable behavior**

Add a test using an unreachable local port and assert the store call rejects within a bounded interval rather than waiting indefinitely. Do not assert internal `iovalkey` error text.

- [ ] **Step 7: Re-run integration tests and close Valkey**

Run:

```bash
cd frontend
VALKEY_TEST_URL=valkey://127.0.0.1:16379 pnpm test:unit -- --run src/lib/server/identity/valkeyRefreshCoordinationStore.integration.test.ts
docker rm -f memo-refresh-valkey
```

Expected: PASS.

- [ ] **Step 8: Commit Task 3**

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
- Consumes `RefreshCoordinationStore` from Task 3 and crypto helpers from Task 2.
- Produces:

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
}): RefreshCoordinator;
```

- Constants: lease 15000 ms, result TTL 120000 ms, follower deadline 12000 ms, poll delay `100 + floor(random() * 101)` ms.

- [ ] **Step 1: Write failing deterministic state-machine tests**

Use an in-memory fake `RefreshCoordinationStore`. Test:

```ts
first acquire returns leader with a random owner
wait eventually returns verified result
wait + lease loss returns indeterminate/leader_lost
wait deadline returns indeterminate/wait_timeout
invalid/cross-generation result returns indeterminate/invalid_result
store exception returns unavailable
publishSuccess MAC-binds S1 and forwards exact 120000 ms TTL
completeFailure never requests attempt deletion
```

- [ ] **Step 2: Run coordinator tests and verify RED**

Run: `cd frontend && pnpm test:unit -- --run src/lib/server/identity/refreshCoordinator.test.ts`

Expected: FAIL because the coordinator does not exist.

- [ ] **Step 3: Implement coordinator decision logic**

Generate owner with `crypto.getRandomValues(new Uint8Array(16))`. `acquire()` owns follower polling; callers never see raw `wait`. Convert store/client exceptions to `{ kind: 'unavailable' }` without including dynamic error text.

- [ ] **Step 4: Run coordinator tests and verify GREEN**

Run: `cd frontend && pnpm test:unit -- --run src/lib/server/identity/refreshCoordinator.test.ts`

Expected: PASS.

- [ ] **Step 5: Commit Task 4**

```bash
git add frontend/src/lib/server/identity/refreshCoordinator.ts \
  frontend/src/lib/server/identity/refreshCoordinator.test.ts
git commit -m "feat: add distributed refresh coordinator state machine"
```

### Task 5: Integrate shared coordination into the SvelteKit session hook

**Files:**
- Modify: `frontend/src/hooks.server.ts`
- Modify: `frontend/src/hooks.server.test.ts`
- Modify: `frontend/src/hooks.server.concurrent.test.ts`

**Interfaces:**
- Consumes Task 1 `IdentitySessionV2` and config, and Task 4 `RefreshCoordinator`.
- Add an injectable optional coordinator to `restoreIdentitySession()` for deterministic tests; production path lazily creates one shared process client from `refreshCoordinationUrl`.
- Disabled mode preserves the current process-local `refreshFlights` behavior.

- [ ] **Step 1: Write failing hook tests for enabled-mode admission**

Add tests:

```ts
it('clears a legacy v1 session before shared coordination', ...)
it('does not contact coordination when the v2 access token is outside the refresh window', ...)
it('preserves sessionExpiresAt after a leader refresh', ...)
```

Assert legacy V1 enabled-mode requests make zero Valkey/coordinator and zero token-endpoint calls.

- [ ] **Step 2: Run focused hook tests and verify RED**

Run: `cd frontend && pnpm test:unit -- --run src/hooks.server.test.ts src/hooks.server.concurrent.test.ts`

Expected: FAIL on the new enabled-mode cases.

- [ ] **Step 3: Implement enabled/disabled mode selection and V2 admission**

Keep normal non-refresh requests independent of Valkey. In enabled mode, only V2 sessions are accepted.

- [ ] **Step 4: Write failing leader/result tests**

Add tests for:

```ts
leader -> exactly one upstream refresh -> publish -> set S1 cookie -> authenticated locals
result -> no upstream refresh -> set verified S1 cookie -> authenticated locals
publish false after upstream success -> clear session and locals
upstream timeout/error -> completeFailure -> clear session and locals
```

Assert refreshed V2 keeps the original `sessionExpiresAt` exactly.

- [ ] **Step 5: Run tests and verify RED**

Run the Step 2 command.

Expected: new leader/result cases FAIL.

- [ ] **Step 6: Implement leader/result/failure orchestration**

The upstream token endpoint is called only for `leader`. Never retry after `publishSuccess()` failure or after any upstream error.

- [ ] **Step 7: Write failing Valkey-unavailable boundary tests**

Test both exact cases:

```ts
accessTokenExpiresAt > now -> keep current token for this request, no cookie rewrite, no refresh call
accessTokenExpiresAt <= now -> clear session, no refresh call
```

Also cover Review Focus #1 by using `accessTokenExpiresAt = now + 1` and asserting the token is accepted only because it is valid at the handoff check.

- [ ] **Step 8: Implement unavailable behavior and verify GREEN**

Run: `cd frontend && pnpm test:unit -- --run src/hooks.server.test.ts src/hooks.server.concurrent.test.ts`

Expected: PASS.

- [ ] **Step 9: Prove disabled mode remains unchanged**

Keep the existing test `singleflights requests carrying the same sealed refresh session`; assert its token/discovery call counts remain unchanged when no coordination URL/coordinator is configured.

- [ ] **Step 10: Run all identity/frontend unit tests**

Run: `cd frontend && pnpm test:unit -- --run`

Expected: PASS.

- [ ] **Step 11: Commit Task 5**

```bash
git add frontend/src/hooks.server.ts frontend/src/hooks.server.test.ts frontend/src/hooks.server.concurrent.test.ts
git commit -m "feat: coordinate browser refresh across replicas"
```

### Task 6: Prove multi-replica correctness against real Valkey

**Files:**
- Create: `frontend/src/lib/server/identity/refreshCoordinator.multiReplica.integration.test.ts`
- Modify: `frontend/package.json`

**Interfaces:**
- Uses two independently created `RefreshCoordinator` + `RefreshCoordinationStore` instances sharing only one real Valkey service.
- Test refresh stub enforces strict single-use behavior: the second redemption of the same refresh token must fail the test even if application code would otherwise handle the provider response.

- [ ] **Step 1: Write the failing two-replica regression**

Create two independently instantiated coordinators for the same V2 S0. Start both orchestration paths concurrently. Assert:

```ts
upstreamRedemptionCount === 1
replicaA.accessToken === 'access-new'
replicaB.accessToken === 'access-new'
replicaA.refreshToken === 'refresh-new'
replicaB.refreshToken === 'refresh-new'
```

The test must fail if the implementation performs two token redemptions.

- [ ] **Step 2: Run against Valkey and confirm behavior**

Run:

```bash
docker run -d --rm --name memo-refresh-valkey -p 16379:6379 valkey/valkey:9.1.2-alpine
cd frontend
VALKEY_TEST_URL=valkey://127.0.0.1:16379 pnpm test:unit -- --run src/lib/server/identity/refreshCoordinator.multiReplica.integration.test.ts
```

Expected before final integration: FAIL if orchestration cannot share the result correctly; after Task 5 wiring it must PASS without weakening the strict stub.

- [ ] **Step 3: Add leader-crash and publish-failure regressions**

Test:

```ts
attempt persisted + lease removed + no result -> second replica never calls upstream and returns indeterminate
upstream success + publish failure -> a subsequent replica never redeems S0 again
result copied to another coordination generation -> verification rejects it
```

- [ ] **Step 4: Add bounded-session/replay-retention regression**

Create V2 S0 with `sessionExpiresAt <= now`; assert rejection occurs before Valkey or upstream calls. For a non-expired S0, expire only the 120-second result while retaining attempt and assert S0 cannot become leader again.

- [ ] **Step 5: Add package script**

Add:

```json
"test:identity-refresh-integration": "vitest run src/lib/server/identity/valkeyRefreshCoordinationStore.integration.test.ts src/lib/server/identity/refreshCoordinator.multiReplica.integration.test.ts"
```

- [ ] **Step 6: Run the complete real-Valkey suite**

Run:

```bash
cd frontend
VALKEY_TEST_URL=valkey://127.0.0.1:16379 pnpm test:identity-refresh-integration
docker rm -f memo-refresh-valkey
```

Expected: PASS.

- [ ] **Step 7: Commit Task 6**

```bash
git add frontend/src/lib/server/identity/refreshCoordinator.multiReplica.integration.test.ts frontend/package.json
git commit -m "test: prove refresh coordination across replicas"
```

### Task 7: Add CI gate, operations documentation, and final verification

**Files:**
- Create: `.github/workflows/identity-refresh-coordination.yml`
- Modify: `docs/browser-auth-operations.md`
- Modify: `README.md` only if implementation adds user-operated private env documentation there.

**Interfaces:**
- Dedicated workflow is a blocking correctness signal for implementation PRs that touch identity refresh coordination paths.
- Documentation keeps one-active-replica production traffic as the default until Stage 4 deployment canary acceptance is completed.

- [ ] **Step 1: Add the dedicated workflow**

Workflow triggers on PR/push path changes to:

```text
frontend/src/lib/server/identity/**
frontend/src/hooks.server.ts
frontend/src/hooks.server*.test.ts
frontend/package.json
frontend/pnpm-lock.yaml
.github/workflows/identity-refresh-coordination.yml
docs/browser-auth-operations.md
```

Job requirements:

```text
Node 24
pnpm 9.15.9
Valkey 9.1.2 container/service
VALKEY_TEST_URL=valkey://127.0.0.1:6379
pnpm install --frozen-lockfile
pnpm test:identity-refresh-integration
pnpm check
pnpm lint
```

- [ ] **Step 2: Validate workflow YAML and package command locally**

Run:

```bash
cd frontend
pnpm check
pnpm lint
```

Expected: PASS. Inspect the workflow for action versions consistent with existing repository workflows.

- [ ] **Step 3: Update browser-auth operations documentation**

Document:

- disabled mode retains process-local singleflight and one-active-replica restriction;
- enabled mode requires V2 session lifetime, no-eviction Valkey, bounded command timeouts, private network/TLS, and canary acceptance;
- legacy sessions require one-time re-login when shared coordination is enabled;
- Valkey failure semantics for valid/expired tokens;
- ambiguous refresh requires re-login and is never retried;
- rollback is reduce to one replica, disable coordination URL, keep backend identity contract unchanged;
- do **not** remove the one-active-replica production restriction until the real deployment canary is complete.

- [ ] **Step 4: Run frontend verification**

Run:

```bash
cd frontend
pnpm check
pnpm lint
pnpm test:unit -- --run
pnpm build
```

Expected: all PASS.

- [ ] **Step 5: Run real-Valkey verification**

Run:

```bash
docker run -d --rm --name memo-refresh-valkey -p 16379:6379 valkey/valkey:9.1.2-alpine
cd frontend
VALKEY_TEST_URL=valkey://127.0.0.1:16379 pnpm test:identity-refresh-integration
docker rm -f memo-refresh-valkey
```

Expected: PASS.

- [ ] **Step 6: Run repository/container verification**

Run from repository root:

```bash
docker compose config --quiet
docker compose build backend frontend
```

Expected: PASS.

- [ ] **Step 7: Perform security/correctness review of the full diff**

Classify every finding as Confirmed / Likely / False Positive / Intentional / Unknown. In particular inspect:

- any code path that can call `refreshAccessToken()` without a persisted shared attempt in enabled mode;
- any code path that deletes attempt state after ambiguous failure;
- dynamic error/log values that could contain URL credentials, cookie values, tokens, owner values, or coordination IDs;
- expiry boundary (`<= now`) and millisecond/second conversion mistakes;
- `iovalkey` reconnection/offline-queue behavior that could exceed the intended 1-second command/connect bound;
- production paths that accidentally make Valkey a dependency outside the refresh window.

Fix only Confirmed/Likely issues after reproducing or proving them, add regression tests, then rerun Steps 4-6.

- [ ] **Step 8: Commit Task 7**

```bash
git add .github/workflows/identity-refresh-coordination.yml docs/browser-auth-operations.md README.md
git commit -m "docs: add distributed refresh rollout gate"
```

If `README.md` was not changed, omit it from `git add`.

- [ ] **Step 9: Push implementation branch and verify GitHub CI**

Required successful signals before merge:

- dedicated Identity refresh coordination workflow;
- Provider-neutral auth boundary;
- Provider-neutral auth E2E;
- normal Frontend checks/build;
- normal CI/container/full-stack checks triggered by the final diff.

Do not label a failure flaky without reproducing/history evidence.

- [ ] **Step 10: Merge decision**

Merge only when all required/relevant checks are green, review findings are resolved, the branch is conflict-free, and the one-active-replica production restriction remains documented pending deployment canary. Use the repository's preferred squash merge.

## Execution Branch and PR Strategy

1. Merge the approved docs/spec+plan PR first so implementation work can cite immutable design artifacts on `main`.
2. Create `feat/distributed-refresh-coordination` from the then-current `main`.
3. Implement Tasks 1-7 with one logical commit per task unless review feedback requires a focused follow-up commit.
4. Open one implementation PR referencing the spec and this plan; do not mix unrelated dependency upgrades or refactors.
5. Do not remove the production one-active-replica restriction in the implementation PR. That is a later operational change after a real two-replica canary.

## Final Acceptance

The implementation PR is ready to merge only if a reviewer can point to executable evidence for each invariant:

- one S0 generation causes at most one upstream redemption while coordination state is intact;
- the attempt marker exists before redemption;
- leader failure/timeout/publish failure never enables S0 retry;
- follower results are cryptographically bound to their coordination generation;
- Valkey outage never triggers uncoordinated refresh;
- still-valid tokens can survive transient coordination outage without cookie mutation;
- expired/legacy/indeterminate sessions fail closed;
- disabled mode retains current single-replica behavior;
- no credential material enters browser-readable state, Valkey plaintext, or logs;
- full existing provider-neutral auth and application CI remains green.
