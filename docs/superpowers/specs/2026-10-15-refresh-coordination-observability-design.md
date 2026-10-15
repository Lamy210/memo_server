# Refresh Coordination Observability Design

**Date:** 2026-10-15  
**Status:** review-required  
**Scope:** SvelteKit BFF shared refresh coordination observability and multi-replica canary acceptance  
**Related:** `docs/browser-auth-operations.md`, `docs/identity-auth-observability.md`, `docs/authentication.md`, issue #2, issue #10, issue #13, PR #178

## Goal

Make the shared browser-session refresh coordination introduced by PR #178 operationally observable without changing its authentication, refresh, cryptographic, or failure semantics.

PR #178 added Valkey-backed coordination so multiple SvelteKit BFF replicas can safely handle the same rotating refresh-token generation. The implementation deliberately keeps production browser-session traffic on one active BFF replica until a real deployment canary validates the coordination service, failure behavior, and concurrent handoff.

The current runtime already distinguishes the outcomes required for that canary, but `restoreWithSharedCoordination()` does not expose a stable, low-cardinality operational signal for them. Operators therefore cannot reliably distinguish a successful leader refresh, a follower handoff, a Valkey outage, an indeterminate attempt, a rejected shared result, or a post-refresh publish failure from deployment logs alone.

The target design adds passive provider-neutral refresh-coordination telemetry that:

- records exactly one terminal outcome for each shared-coordination attempt;
- uses a fixed low-cardinality taxonomy;
- emits immediate warnings only for operationally actionable failure outcomes;
- emits cumulative process-local aggregates every 50 shared-coordination attempts;
- never logs credentials, browser-session artifacts, coordination identifiers, user identifiers, identity-provider details, Valkey connection details, or raw upstream errors;
- never changes authentication or refresh decisions when telemetry or logging fails;
- provides explicit multi-replica canary acceptance criteria before the one-active-BFF production restriction can be removed.

## Non-goals

This change does not:

- change the refresh coordination algorithm from PR #178;
- change leader election, follower waiting, replay protection, lease duration, result TTL, or attempt TTL;
- change the V2 browser-session schema or fixed `sessionExpiresAt` semantics;
- change the result-handoff encryption or MAC design;
- add a public metrics endpoint;
- add Prometheus, OpenTelemetry, or another runtime telemetry dependency;
- create a dashboard, alert, or log collector automatically;
- change production topology to multiple active BFF replicas;
- deploy or configure Valkey;
- deploy or configure the production identity issuer;
- add provider-specific identity or refresh-token behavior to the application contract;
- add a separate login-rate telemetry subsystem;
- add new flags to the production identity acceptance runner;
- make process-local counters authoritative for billing, audit, authorization, or distributed correctness.

## Current state

### Shared refresh coordination

`frontend/src/hooks.server.ts` currently centralizes the shared coordination decision after a V2 browser session enters the near-expiry refresh window.

The terminal branches include:

- Valkey/coordination unavailable;
- indeterminate follower state;
- valid follower result;
- invalid or expired follower result;
- leader refresh success;
- fixed session lifetime expiry while the leader is refreshing;
- successful upstream refresh followed by publish failure;
- discovery/token refresh failure.

`frontend/src/lib/server/identity/refreshCoordinator.ts` already exposes fixed indeterminate reasons:

```ts
type RefreshCoordinationDecision =
  | { kind: 'leader'; owner: string }
  | { kind: 'result'; sealedSession: string }
  | { kind: 'indeterminate'; reason: 'leader_lost' | 'wait_timeout' | 'invalid_result' }
  | { kind: 'unavailable' };
```

The coordinator therefore already contains enough semantic information to produce useful low-cardinality operational outcomes without inspecting raw Valkey errors or provider responses.

### Existing authentication observability pattern

The Rust backend already uses fixed provider-neutral authentication and JWKS refresh taxonomies, process-local cumulative counters, and redacted aggregate logging. Dynamic error text and identity/key/provider values are deliberately excluded from telemetry dimensions.

The frontend refresh observability design follows the same principles but remains a separate server-only module because the SvelteKit runtime and Rust backend have different execution contexts.

## Chosen architecture

Add one server-only module:

```text
frontend/src/lib/server/identity/refreshObservability.ts
```

Its public production API is intentionally narrow:

```ts
export type RefreshCoordinationOutcome =
  | 'leader_success'
  | 'follower_success'
  | 'unavailable_continue'
  | 'unavailable_fail_closed'
  | 'leader_lost'
  | 'wait_timeout'
  | 'invalid_result'
  | 'session_expired'
  | 'upstream_failure'
  | 'publish_failure';

export function recordRefreshCoordinationOutcome(
  outcome: RefreshCoordinationOutcome
): void;
```

`restoreWithSharedCoordination()` remains the single owner that converts one completed shared-coordination request into one terminal telemetry outcome.

The Valkey store, cryptographic helpers, and `RefreshCoordinator` do not emit this terminal telemetry themselves. That avoids double counting when one request traverses several internal layers before reaching a final authentication decision.

```text
Valkey store / crypto
        |
        v
RefreshCoordinator
        |
        v
restoreWithSharedCoordination
        |
        +--> final auth/session decision
        |
        +--> exactly one terminal outcome record
```

## Fixed outcome taxonomy

The taxonomy is closed to these ten values:

| Outcome | Meaning | Immediate warning |
| --- | --- | --- |
| `leader_success` | This replica became leader, refreshed upstream successfully, published the handoff, and authenticated with the refreshed session. | no |
| `follower_success` | This replica adopted a valid authenticated shared result and authenticated with it. | no |
| `unavailable_continue` | Shared coordination was unavailable, but both the current V2 session lifetime and access token remained valid after the coordination attempt, so the current request continued without refresh or cookie rewrite. | yes |
| `unavailable_fail_closed` | Shared coordination was unavailable while the V2 session lifetime remained valid, but the access token was no longer valid, so the session was cleared rather than refreshing without coordination. | yes |
| `leader_lost` | A persisted attempt no longer had a usable active leader/result state; the old refresh credential is not retried. | yes |
| `wait_timeout` | A follower reached the bounded 25-second deadline without obtaining a valid result. | yes |
| `invalid_result` | A coordination result or decrypted shared session failed the fixed validation contract, including cryptographic/result validation, generation/session-lifetime mismatch, or an already-expired handed-off access token. | yes |
| `session_expired` | The original fixed V2 `sessionExpiresAt` elapsed while shared coordination/leader work was in progress, so the session was cleared without extending its lifetime. | no |
| `upstream_failure` | The elected leader failed during identity discovery or refresh-token redemption and retained the no-retry invariant. | yes |
| `publish_failure` | Upstream refresh succeeded but the leader could not publish the authenticated shared result, so the session failed closed and the old generation is not retried. | yes |

The implementation must not derive new outcome strings from exceptions, HTTP statuses, Valkey responses, OIDC errors, URLs, or provider payloads.

## Classification precedence

Some failure paths can satisfy more than one time-based condition by the time the request finishes. Classification is therefore deterministic.

### Coordination unavailable

After `acquire()` returns `unavailable`, evaluate at decision time:

1. if `sessionExpiresAt <= decisionNow`, record `session_expired` and clear the session;
2. otherwise, if `accessTokenExpiresAt <= decisionNow`, record `unavailable_fail_closed` and clear the session;
3. otherwise record `unavailable_continue` and authenticate the current request with the existing access token.

This preserves the fixed V2 session-lifetime invariant and distinguishes a normal session-lifetime boundary from an infrastructure outage that makes an access token unusable.

### Indeterminate coordinator decision

Map the coordinator's fixed reason directly:

- `leader_lost` -> `leader_lost`;
- `wait_timeout` -> `wait_timeout`;
- `invalid_result` -> `invalid_result`.

Do not inspect raw store or cryptographic error details.

### Shared follower result

Before accepting a follower result:

1. if the original V2 `sessionExpiresAt <= decisionNow`, record `session_expired` and clear the session;
2. otherwise, if the handed-off sealed session cannot be opened, is not V2, changes `sessionExpiresAt`, or has `accessTokenExpiresAt <= decisionNow`, record `invalid_result` and clear the session;
3. otherwise record `follower_success`, set the refreshed sealed cookie, and authenticate.

### Leader path

For an elected leader:

- discovery/token refresh exception -> `upstream_failure`;
- upstream refresh succeeds but original fixed session lifetime has elapsed before the new session is accepted -> `session_expired`;
- refreshed session is created but `publishSuccess()` returns false -> `publish_failure`;
- publish succeeds and the refreshed session is installed -> `leader_success`.

If an upstream exception occurs after the session lifetime also happens to elapse, classify the terminal cause as `upstream_failure`. This prevents a real identity dependency failure from being hidden by a coincident time boundary.

## Exactly-once recording boundary

One invocation of `restoreWithSharedCoordination()` that actually reaches shared coordination records zero or one terminal outcome until a terminal branch is reached, and every terminal shared-coordination branch records exactly one outcome.

No outcome is recorded for:

- requests with no production browser session;
- invalid/tampered sessions rejected before shared coordination;
- legacy V1 sessions cleared before shared coordination;
- V2 access tokens outside the 30-second refresh window;
- coordination-disabled process-local refresh;
- helper/store/crypto operations that are not themselves terminal browser-auth decisions.

`completeFailure()` never records an additional outcome. It remains cleanup only. A cleanup failure must not create a second telemetry event or replace the primary terminal classification.

## Counter and aggregate semantics

The observability module keeps module-local cumulative integer counters:

```ts
type RefreshCoordinationCounters = {
  total: number;
  leader_success: number;
  follower_success: number;
  unavailable_continue: number;
  unavailable_fail_closed: number;
  leader_lost: number;
  wait_timeout: number;
  invalid_result: number;
  session_expired: number;
  upstream_failure: number;
  publish_failure: number;
};
```

Requirements:

- increment `total` and exactly one outcome counter synchronously for each recorded terminal outcome;
- counters are cumulative for the process lifetime and reset on process restart;
- emit one aggregate INFO line when `total % 50 === 0`;
- do not reset counters after an aggregate emission;
- do not persist counters to Valkey or another store;
- do not use counters for authentication or refresh decisions;
- do not attempt cross-replica atomic aggregation inside the application.

Fleet-level aggregation is owned by the deployment logging platform using runtime metadata already attached outside the application. The application must not add pod names, instance IDs, hostnames, topology IDs, or deployment identifiers to its own refresh-outcome dimensions.

## Logging contract

### Aggregate INFO event

Every 50 recorded shared-coordination outcomes, emit one fixed-format cumulative aggregate event:

```text
identity refresh coordination aggregate: total=<n> leader_success=<n> follower_success=<n> unavailable_continue=<n> unavailable_fail_closed=<n> leader_lost=<n> wait_timeout=<n> invalid_result=<n> session_expired=<n> upstream_failure=<n> publish_failure=<n>
```

Field names and order are fixed.

### Immediate WARN event

For operationally actionable abnormal outcomes, emit one fixed warning:

```text
identity refresh coordination warning: outcome=<fixed-outcome>
```

Immediate WARN outcomes are exactly:

- `unavailable_continue`;
- `unavailable_fail_closed`;
- `leader_lost`;
- `wait_timeout`;
- `invalid_result`;
- `upstream_failure`;
- `publish_failure`.

`leader_success`, `follower_success`, and `session_expired` do not emit per-attempt warnings. They remain visible through aggregate counters. A normal bounded session expiry is not independently an infrastructure incident.

### Privacy boundary

Refresh-coordination telemetry MUST NOT contain:

- access tokens;
- refresh tokens;
- browser session cookies or sealed-session values;
- login transaction cookies or authorization codes;
- coordination IDs;
- leader owner tokens;
- session ciphertext/nonces/MACs;
- `sub`, user UUID, email, or account identifiers;
- `jti`, `kid`, `client_id`;
- issuer, authorization, token, discovery, or JWKS URLs;
- Valkey/Redis URLs, hosts, usernames, ACL names, or topology;
- provider/vendor/product names;
- raw exception text;
- raw HTTP status/body/header values;
- raw Valkey response/error text.

The production recording API accepts only the fixed enum. It has no parameter through which callers can attach dynamic context.

## Passive failure semantics

Observability is strictly passive.

`recordRefreshCoordinationOutcome()` must not throw into the authentication path. Logging failures are contained inside the observability module.

Telemetry must never:

- convert a failed refresh into success;
- convert success into failure;
- retry Valkey access;
- retry identity discovery or token refresh;
- alter lease/result/attempt TTLs;
- rewrite session cookies;
- alter `locals.accessToken`;
- extend `sessionExpiresAt`;
- alter the old-generation no-retry invariant;
- create a dependency on the log collector or dashboard.

If stdout/stderr collection is broken or the logger implementation throws, the browser-auth decision remains exactly the same as if telemetry had not been present.

## Implementation boundary

Expected production changes:

```text
frontend/src/lib/server/identity/refreshObservability.ts
frontend/src/hooks.server.ts
docs/browser-auth-operations.md
.github/workflows/identity-refresh-coordination.yml
```

Expected test changes:

```text
frontend/src/lib/server/identity/refreshObservability.test.ts
frontend/src/hooks.server.test.ts
```

The implementation may use a narrow injectable logger/recorder dependency for tests, but the production API remains fixed and server-only.

The implementation should not move coordination state into the observability module and should not refactor unrelated authentication code.

## TDD strategy

Implementation follows RED -> GREEN -> review -> regression hardening.

### Observability module RED tests

Before implementing the module, add tests proving:

- each of the ten fixed outcomes increments exactly its own counter;
- `total` increments once per record;
- no aggregate INFO is emitted for totals 1 through 49;
- exactly one aggregate INFO is emitted at total 50;
- cumulative counts remain intact after total 50;
- warning outcomes emit one fixed WARN line;
- success outcomes and `session_expired` do not emit per-attempt WARN lines;
- logger exceptions are swallowed and do not escape to the caller;
- the public recorder accepts no dynamic context object/string alongside the fixed outcome.

Tests may expose a test-only reset/snapshot helper or construct an isolated recorder instance so test order does not depend on module-global state. Test-only access must not widen the production runtime telemetry surface.

### Hook integration RED tests

Extend `frontend/src/hooks.server.test.ts` to prove the integration records exactly one terminal outcome for:

- leader success;
- follower success;
- coordination unavailable with valid session/token -> `unavailable_continue`;
- coordination unavailable with live session but expired access token -> `unavailable_fail_closed`;
- coordination unavailable after fixed session expiry -> `session_expired`;
- `leader_lost`;
- `wait_timeout`;
- coordinator `invalid_result`;
- decrypted/opened shared result rejection -> `invalid_result`;
- fixed session expiry while elected leader work completes -> `session_expired`;
- discovery/token refresh failure -> `upstream_failure`;
- publish failure -> `publish_failure`.

Also prove the recorder is not called for:

- a live token outside the refresh window;
- a V1 session cleared before shared coordination;
- coordination-disabled process-local refresh.

Existing refresh tests should be reused rather than duplicated where their setup already covers these branches.

### Regression requirements

Existing assertions for cookie clearing, authentication state, upstream call counts, `completeFailure()`, fixed `sessionExpiresAt`, no process-local fallback, and result validation must continue to pass unchanged except for recorder injection/assertions.

Telemetry tests must not weaken behavioral assertions merely to make logging tests easier.

## CI strategy

Do not add a new workflow.

The existing `.github/workflows/identity-refresh-coordination.yml` already triggers on:

- `frontend/src/hooks.server.ts`;
- `frontend/src/hooks.server.test.ts`;
- `frontend/src/lib/server/identity/**`;
- `docs/browser-auth-operations.md`;
- the workflow itself.

Extend its focused contract step to include `refreshObservability.test.ts` while retaining:

- refresh-handoff security tests;
- real Valkey 9.1.2 multi-replica integration;
- TypeScript/Svelte checks;
- lint.

The implementation PR must also pass the repository-wide gates that cover the affected authentication surface, including:

- `Identity refresh coordination`;
- `Provider-neutral auth boundary`;
- `Provider-neutral auth E2E`;
- normal repository `CI` with frontend unit/build, browser E2E, container validation, backend checks/integrations, and full-stack smoke;
- UI/Lighthouse workflows when their path filters select the change.

## Operations runbook changes

`docs/browser-auth-operations.md` remains the canonical operations document for shared browser-refresh topology.

Add sections describing:

- the ten fixed refresh-coordination outcomes;
- which outcomes produce immediate warnings;
- the cumulative every-50 aggregate event;
- the telemetry privacy boundary;
- the fact that counters are process-local and reset on restart;
- the fact that the deployment logging platform, not the application, performs fleet-level aggregation;
- canary evidence required before allowing multiple active BFF replicas for browser-session traffic.

The production identity acceptance runner remains unchanged. Its responsibility is the provider-neutral issuer/JWKS/access-token/browser-flow/rotation/monitoring contract. Multi-replica BFF refresh topology remains a browser-auth operations concern.

## Multi-replica canary acceptance

The one-active-BFF production restriction from PR #178 remains in force until a controlled real-environment canary passes.

A successful canary requires all of the following evidence from the candidate deployment:

1. shared coordination is enabled with V2 bounded browser sessions;
2. two active BFF replicas can receive browser-session traffic;
3. concurrent near-expiry requests for the same old V2 session generation produce exactly one upstream refresh-token redemption;
4. the canary observes at least one `leader_success`;
5. the canary observes at least one `follower_success`;
6. during the controlled successful-concurrency rehearsal, `leader_lost`, `wait_timeout`, `invalid_result`, `upstream_failure`, and `publish_failure` remain zero;
7. a controlled Valkey-unavailability rehearsal produces the expected `unavailable_continue` or `unavailable_fail_closed` behavior without process-local refresh fallback;
8. a controlled fixed-session-expiry rehearsal produces `session_expired` and never extends `sessionExpiresAt`;
9. sampled refresh-coordination logs contain none of the prohibited credential, identity, provider, topology, URL, coordination, or raw-error material;
10. the intended private log collector/dashboard can distinguish the fixed categories and aggregate replicas using deployment metadata supplied outside the application;
11. the intended alert/on-call path can detect sustained warning-category failures;
12. existing authentication monitoring shows no unexplained material increase in login or unauthorized rates during/after the canary.

Exactly-one upstream redemption must be verified using approved controlled-environment evidence at the identity/token endpoint or equivalent deployment instrumentation. It is not inferred solely from the BFF's own `leader_success` counter.

The requirement that the listed abnormal outcomes remain zero applies to the controlled successful-concurrency rehearsal, not as a permanent production SLO. Real production SLO/alert thresholds remain environment-specific.

Only after this acceptance is recorded may deployment policy be changed to permit multiple active BFF replicas for browser-session traffic.

## Rollback

The rollback contract from PR #178 remains unchanged.

If shared coordination or its canary behaves unexpectedly:

1. reduce browser-session traffic to one active BFF replica;
2. disable/remove `IDENTITY_REFRESH_COORDINATION_URL`;
3. keep V2 sessions valid in the single-replica path;
4. return to process-local single-flight refresh;
5. leave backend JWT/JWKS policy unchanged.

Telemetry does not participate in rollback correctness. Missing telemetry is an operations problem, not permission to bypass the one-active-replica restriction or coordination safety invariants.

## Issue tracking updates after implementation

The implementation PR should reconcile existing issue text rather than close environment-dependent work prematurely.

### Issue #10

Update the stale remaining-work statement that multi-replica coordination still needs to be decided.

Record that:

- shared multi-replica refresh coordination was implemented by PR #178;
- runtime refresh-coordination observability is implemented by the follow-up PR;
- the real deployment canary remains required before multi-active browser-session traffic is enabled.

Issue #10 should remain open if real production browser client/configuration acceptance is still incomplete.

### Issue #13

Keep issue #13 open.

This observability change does not complete:

- real issuer deployment/configuration;
- real web-client registration/callback/logout origins;
- real production-profile access-token verification;
- real signing-key overlap/rotation/rollback rehearsal;
- real deployed authentication/JWKS monitoring acceptance.

### Issue #2

Update the production-readiness progress summary to mention:

- PR #178 shared refresh coordination;
- the follow-up refresh observability work;
- the remaining real-environment multi-replica canary restriction.

Do not mark the overall production identity work complete solely because repository CI passes.

## Security review checklist

Before merge, review the implementation specifically for:

- no credential/session/coordination identifier in any emitted string;
- no raw exception or provider/Valkey error interpolation;
- no URL/hostname/topology labels;
- fixed taxonomy only;
- exactly one terminal outcome for each shared-coordination attempt;
- no double counting from coordinator/store/cleanup layers;
- deterministic classification precedence around session/token expiry;
- logger failure cannot affect authentication;
- no change to refresh retry/no-retry invariants;
- no change to result encryption/MAC handling;
- no change to session lifetime semantics;
- no change to disabled-mode process-local behavior;
- no new production dependency.

## Acceptance criteria

Repository implementation is acceptable when all of the following are true:

- the ten-value outcome taxonomy is enforced by TypeScript;
- `restoreWithSharedCoordination()` records exactly one terminal outcome on every shared-coordination terminal path and none on out-of-scope paths;
- fixed WARN events are emitted only for the seven documented abnormal categories;
- one cumulative aggregate INFO event is emitted on every 50th recorded outcome;
- logger failure is contained and cannot change authentication/session behavior;
- no prohibited dynamic/credential/provider/topology data can be attached through the production recorder API;
- existing refresh-coordination security/correctness regressions remain green;
- real-Valkey integration remains green;
- provider-neutral auth boundary/E2E and repository-wide CI remain green;
- `docs/browser-auth-operations.md` documents the telemetry and real multi-replica canary acceptance contract;
- production continues with one active browser-session BFF replica until that real canary succeeds;
- issues #10, #13, and #2 accurately reflect the post-implementation state without claiming environment acceptance that repository CI cannot prove.
