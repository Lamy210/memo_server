# Refresh Coordination Observability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add passive, provider-neutral observability for distributed browser refresh coordination and document the real-environment canary evidence required before multi-active BFF rollout.

**Architecture:** Keep terminal outcome ownership in `restoreWithSharedCoordination()` and add one server-only recorder module with fixed counters and redacted logging. The recorder accepts only a fixed outcome enum, is process-local and passive, and does not alter Valkey/OIDC/session behavior. Existing refresh coordination CI remains the focused gate.

**Tech Stack:** TypeScript, SvelteKit 2, Vitest 2, pnpm 9.15.9, Node.js 24, existing Valkey 9.1.2 coordination tests.

**Spec:** `docs/superpowers/specs/2026-10-15-refresh-coordination-observability-design.md`

## Global Constraints

- The outcome taxonomy is exactly: `leader_success`, `follower_success`, `unavailable_continue`, `unavailable_fail_closed`, `leader_lost`, `wait_timeout`, `invalid_result`, `session_expired`, `upstream_failure`, `publish_failure`.
- Immediate WARN outcomes are exactly: `unavailable_continue`, `unavailable_fail_closed`, `leader_lost`, `wait_timeout`, `invalid_result`, `upstream_failure`, `publish_failure`.
- Aggregate INFO is cumulative and emitted when `total % 50 === 0`; counters are never reset after emission.
- Telemetry must never include credentials, sealed sessions/cookies, coordination IDs, owner tokens, user identifiers, issuer/provider details, Valkey connection details, topology identifiers, HTTP details, or raw exception/error text.
- Telemetry is passive: recorder/logging failures must not change authentication, refresh, cookie, coordination, retry, TTL, or fail-closed semantics.
- No new runtime telemetry dependency, public metrics endpoint, production identity acceptance flag, or production topology change.
- Shared coordination remains required for multi-active correctness; process-local fallback remains forbidden when shared coordination is enabled.
- Keep the fixed V2 `sessionExpiresAt` behavior from PR #178; refresh never extends it.

## Review Focus

1. **Boundary-time precedence:** when `sessionExpiresAt === decisionNow`, record `session_expired` even if the access token is also expired; tests in Task 2 pin exact equality.
2. **Follower validation precedence:** if the original V2 session expires while waiting and the handed-off result is also invalid, record only `session_expired`; tests in Task 2 pin exactly-once behavior.
3. **Upstream failure after time advances:** if discovery/token refresh throws after the fixed session lifetime has elapsed, preserve the spec precedence and record `upstream_failure`, not `session_expired`; Task 2 adds this regression.
4. **Logger failure at aggregate boundary:** on the 50th abnormal outcome, a throwing WARN logger must not prevent the aggregate INFO attempt or escape to the caller; Task 1 tests warning and aggregate calls independently.
5. **No accidental measurement outside shared coordination:** live-token, V1 migration, and coordination-disabled process-local refresh paths must record nothing; Task 2 keeps explicit zero-call assertions.

---

### Task 1: Fixed refresh coordination recorder

**Files:**
- Create: `frontend/src/lib/server/identity/refreshObservability.ts`
- Create: `frontend/src/lib/server/identity/refreshObservability.test.ts`

**Interfaces:**
- Consumes: `console.info(message)` and `console.warn(message)` only as the default server logger surface.
- Produces:
  - `export type RefreshCoordinationOutcome = 'leader_success' | 'follower_success' | 'unavailable_continue' | 'unavailable_fail_closed' | 'leader_lost' | 'wait_timeout' | 'invalid_result' | 'session_expired' | 'upstream_failure' | 'publish_failure'`
  - `export function recordRefreshCoordinationOutcome(outcome: RefreshCoordinationOutcome): void`
- Production callers cannot attach context; no second argument or metadata object is exposed.

- [ ] **Step 1: Write RED tests for the fixed type and per-outcome counters**

In `refreshObservability.test.ts`, dynamically import a fresh module per test group with `vi.resetModules()` so module-local counters start at zero. Assert all ten outcomes increment `total` and exactly their own named count. Use `expectTypeOf(recordRefreshCoordinationOutcome).toEqualTypeOf<(outcome: RefreshCoordinationOutcome) => void>()` to pin the one-argument production API.

- [ ] **Step 2: Write RED tests for WARN/aggregate behavior and passive logging failure**

Assert:
- totals 1..49 emit no INFO;
- total 50 emits exactly one fixed-format cumulative INFO line with fields in spec order;
- the seven WARN outcomes each emit exactly one fixed `identity refresh coordination warning: outcome=<outcome>` line;
- `leader_success`, `follower_success`, and `session_expired` emit no per-attempt WARN;
- a throwing `console.warn` does not escape;
- on the 50th abnormal outcome, a throwing WARN still allows the INFO aggregate attempt;
- a throwing `console.info` does not escape.

- [ ] **Step 3: Run the focused test and verify RED**

Run:
```bash
cd frontend
pnpm exec vitest run src/lib/server/identity/refreshObservability.test.ts
```
Expected: FAIL because `refreshObservability.ts` or its exported recorder does not exist yet.

- [ ] **Step 4: Implement the minimal recorder**

In `refreshObservability.ts`:
- initialize module-local cumulative numeric counters for `total` plus the ten outcomes;
- increment synchronously on every record;
- invoke WARN and INFO through separate internal `try/catch` boundaries so one logger failure cannot suppress the other;
- construct only the two fixed log formats from the enum and counters;
- do not export counters, logger configuration, reset helpers, or dynamic metadata APIs.

- [ ] **Step 5: Run recorder tests and type/lint checks**

Run:
```bash
cd frontend
pnpm exec vitest run src/lib/server/identity/refreshObservability.test.ts
pnpm check
pnpm lint
```
Expected: all commands PASS.

- [ ] **Step 6: Commit Task 1**

```bash
git add frontend/src/lib/server/identity/refreshObservability.ts \
        frontend/src/lib/server/identity/refreshObservability.test.ts
git commit -m "feat: add refresh coordination observability"
```

### Task 2: Record exactly one terminal outcome in browser-session restore

**Files:**
- Modify: `frontend/src/hooks.server.ts`
- Modify: `frontend/src/hooks.server.test.ts`

**Interfaces:**
- Consumes: `RefreshCoordinationOutcome` and `recordRefreshCoordinationOutcome(outcome)` from Task 1.
- Produces: optional test seam on `restoreIdentitySession()` input:
  - `refreshOutcomeRecorder?: (outcome: RefreshCoordinationOutcome) => void`
- `restoreWithSharedCoordination()` receives a required recorder function so all shared terminal decisions have one owner.

- [ ] **Step 1: Add RED integration assertions to existing successful/failure cases**

Reuse existing `hooks.server.test.ts` setups and inject `const recordOutcome = vi.fn()` through `refreshOutcomeRecorder`. Pin exactly one call for:
- leader success -> `leader_success`;
- follower result adoption -> `follower_success`;
- unavailable with both fixed session and token valid -> `unavailable_continue`;
- unavailable with live fixed session but expired token -> `unavailable_fail_closed`;
- publish failure -> `publish_failure`;
- identity discovery/token refresh failure -> `upstream_failure`.

- [ ] **Step 2: Add RED tests for indeterminate reasons and invalid follower results**

Add cases using the existing fake coordinator:
- `{ kind: 'indeterminate', reason: 'leader_lost' }` -> exactly `leader_lost`;
- `{ kind: 'indeterminate', reason: 'wait_timeout' }` -> exactly `wait_timeout`;
- `{ kind: 'indeterminate', reason: 'invalid_result' }` -> exactly `invalid_result`;
- `{ kind: 'result' }` containing a decryptable but invalid V2 result (changed fixed `sessionExpiresAt` or expired handed-off access token) -> exactly `invalid_result`.

Keep the existing cookie/auth/fetch assertions in every case.

- [ ] **Step 3: Add RED precedence and zero-measurement regressions**

Pin:
- unavailable at exact `sessionExpiresAt === decisionNow` -> exactly `session_expired`;
- follower wait ends after original session expiry even when result is otherwise invalid -> exactly `session_expired`;
- elected leader's successful upstream call finishing after the fixed lifetime -> exactly `session_expired`;
- discovery/token exception after time has advanced past fixed lifetime -> exactly `upstream_failure`;
- live V2 token outside refresh window -> recorder not called;
- legacy V1 cleared before shared coordination -> recorder not called;
- coordination-disabled process-local refresh -> recorder not called.

- [ ] **Step 4: Run hooks tests and verify RED**

Run:
```bash
cd frontend
pnpm exec vitest run src/hooks.server.test.ts
```
Expected: FAIL because `refreshOutcomeRecorder` and terminal recording are not implemented.

- [ ] **Step 5: Implement recorder injection and terminal classification**

Update `IdentitySessionRestoreInput` with the optional recorder seam. Import the production recorder and use it as the default only when `restoreIdentitySession()` enters shared coordination.

Update `restoreWithSharedCoordination()` terminal branches with these exact rules:
- unavailable: `sessionExpiresAt <= decisionNow` -> `session_expired`; else `accessTokenExpiresAt <= decisionNow` -> `unavailable_fail_closed`; else `unavailable_continue`;
- indeterminate: record `decision.reason` directly;
- follower result: original session expiry first -> `session_expired`; otherwise invalid opened/result session -> `invalid_result`; otherwise `follower_success`;
- leader: upstream discovery/token exception -> `upstream_failure`; successful upstream work after fixed lifetime -> `session_expired`; failed publication -> `publish_failure`; installed refreshed session -> `leader_success`.

Do not add recording to process-local refresh, V1 migration, invalid-cookie/config handling, or live-token fast path.

- [ ] **Step 6: Run hook, recorder, concurrency, and handoff regressions**

Run:
```bash
cd frontend
pnpm exec vitest run \
  src/hooks.server.test.ts \
  src/hooks.server.concurrent.test.ts \
  src/lib/server/identity/refreshObservability.test.ts \
  src/lib/server/identity/refreshCoordinationCrypto.test.ts \
  src/lib/server/identity/refreshResultAcceptance.test.ts
pnpm check
pnpm lint
```
Expected: all commands PASS.

- [ ] **Step 7: Commit Task 2**

```bash
git add frontend/src/hooks.server.ts frontend/src/hooks.server.test.ts
git commit -m "feat: observe refresh coordination outcomes"
```

### Task 3: Wire focused CI and production canary runbook

**Files:**
- Modify: `.github/workflows/identity-refresh-coordination.yml`
- Modify: `docs/browser-auth-operations.md`

**Interfaces:**
- Consumes: `refreshObservability.test.ts` and the ten fixed outcome names from Tasks 1-2.
- Produces: focused CI coverage and the operator acceptance contract; no runtime API.

- [ ] **Step 1: Extend the focused workflow test command**

In `.github/workflows/identity-refresh-coordination.yml`, include `src/lib/server/identity/refreshObservability.test.ts` in the existing `Run refresh handoff security contracts` Vitest invocation. Keep Valkey 9.1.2 integration, `pnpm check`, and `pnpm lint` unchanged.

- [ ] **Step 2: Update `browser-auth-operations.md` with the telemetry contract**

Document:
- the ten fixed outcomes and the seven immediate WARN outcomes;
- aggregate INFO every 50 shared-coordination attempts;
- process-local/cumulative semantics;
- privacy/cardinality exclusions;
- passive failure behavior;
- no outcome on non-shared paths.

- [ ] **Step 3: Make the two-replica canary release gate explicit**

Keep the one-active-BFF production restriction. State that it may only be reconsidered after a real two-active-replica canary demonstrates:
- one upstream refresh redemption for concurrent requests sharing one near-expiry V2 generation;
- at least one observed `leader_success` and `follower_success` in the controlled handoff exercise;
- zero `leader_lost`, `wait_timeout`, `invalid_result`, `upstream_failure`, and `publish_failure` during that controlled success rehearsal;
- outage exercises produce expected `unavailable_continue` / `unavailable_fail_closed` without process-local refresh fallback;
- fixed lifetime exercise produces `session_expired` without lifetime extension;
- sampled operational logs contain none of the prohibited credential/identity/topology/raw-error values;
- deployed log collection/alerting can distinguish the fixed categories;
- no material unexplained login-rate or unauthorized-rate regression compared with the environment baseline.

Clarify that zero abnormal outcomes is a controlled-canary acceptance condition, not a permanent production SLO.

- [ ] **Step 4: Run documentation-sensitive focused checks**

Run:
```bash
cd frontend
pnpm exec vitest run src/lib/server/identity/refreshObservability.test.ts
pnpm check
pnpm lint
```
Expected: PASS.

- [ ] **Step 5: Commit Task 3**

```bash
git add .github/workflows/identity-refresh-coordination.yml docs/browser-auth-operations.md
git commit -m "docs: define refresh coordination canary signals"
```

### Task 4: Full verification, review, and issue-tracking handoff

**Files:**
- Review all changes in the feature branch.
- No production topology or deployment file should change.

**Interfaces:**
- Consumes: Tasks 1-3.
- Produces: merge-ready implementation PR and post-merge issue updates.

- [ ] **Step 1: Run the complete frontend/local contract suite available without external deployment credentials**

Run:
```bash
cd frontend
pnpm exec vitest run \
  src/lib/server/identity/refreshObservability.test.ts \
  src/hooks.server.test.ts \
  src/hooks.server.concurrent.test.ts \
  src/lib/server/identity/refreshCoordinationCrypto.test.ts \
  src/lib/server/identity/refreshResultAcceptance.test.ts
pnpm check
pnpm lint
pnpm build
```
Expected: PASS.

- [ ] **Step 2: Run real-Valkey integration when Valkey is available**

Run with the repository's existing Valkey 9.1.2 test setup:
```bash
cd frontend
VALKEY_TEST_URL=valkey://127.0.0.1:6379 pnpm test:identity-refresh-integration
```
Expected: PASS with strict single-use refresh and follower convergence unchanged.

- [ ] **Step 3: Review the branch against the design spec**

Verify:
- exactly ten fixed outcomes, no dynamic label/context surface;
- exactly one terminal record per shared-coordination path;
- zero records outside shared coordination;
- logging failures cannot change auth behavior;
- no credential/provider/topology leakage in log construction;
- no refresh coordination semantic/TTL/crypto changes;
- no production topology change;
- docs and workflow match the implemented taxonomy exactly.

Any Critical/Important finding must be fixed with a regression test before proceeding.

- [ ] **Step 4: Open the implementation PR and require exact-head CI**

PR title:
```text
feat: add refresh coordination observability
```

Require successful exact-head runs for at least:
- `Identity refresh coordination`;
- `Provider-neutral auth boundary`;
- `Provider-neutral auth E2E`;
- repository `CI`.

Do not treat repository CI as evidence that the real multi-replica production canary has succeeded.

- [ ] **Step 5: After merge, update tracking without closing environment-dependent work**

- Issue #10: replace the stale future-decision wording with: distributed coordination is implemented by #178, observability is implemented by this PR, and real multi-replica canary acceptance remains required before topology expansion.
- Issue #13: keep open; real issuer/client/token/rotation/monitoring acceptance remains environment-dependent.
- Issue #2: record refresh coordination + observability as completed repository work while keeping production identity readiness incomplete.

Do not close #10, #13, or #2 solely because this implementation merges.
