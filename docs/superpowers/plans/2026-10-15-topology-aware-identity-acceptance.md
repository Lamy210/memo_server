# Topology-Aware Production Identity Acceptance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the production identity acceptance runner explicitly distinguish single-active from multi-active browser BFF readiness and require real refresh-coordination canary evidence before reporting multi-active readiness.

**Architecture:** Extend the existing Python acceptance runner rather than introducing a second readiness command. A closed `BrowserTopology` enum flows explicitly from CLI parsing into `run_acceptance()` and `AcceptanceResult`; topology-dependent evidence remains operator attestation, while issuer/JWKS preflight and the canonical Rust token verifier remain unchanged. Documentation and the focused GitHub Actions workflow are updated to make the topology-scoped gate operationally unambiguous.

**Tech Stack:** Python 3 `argparse` / `dataclasses` / `unittest`, existing Rust verifier, GitHub Actions YAML, Markdown runbooks.

**Spec:** `docs/superpowers/specs/2026-10-15-topology-aware-identity-acceptance-design.md`

## Global Constraints

- `--browser-topology` is required and accepts only `single-active` or `multi-active`; there is no default.
- Machine-readable topology output is only `single_active`, `multi_active`, or `not_run`.
- `--refresh-coordination-canary-validated` is valid only with `multi-active`; using it with `single-active` is a usage failure.
- Missing multi-active canary evidence is `failure_phase=operator_evidence`, not a new failure phase.
- CLI/topology/evidence failures must occur before issuer/JWKS network activity and before the real access token is consumed by the verifier.
- `--monitoring-validated` keeps its existing authentication/JWKS meaning; it does not absorb refresh-coordination canary evidence.
- `identity_acceptance.ready=true` is scoped only to the explicitly declared browser topology and never changes deployment topology automatically.
- Do not emit rejected topology input, issuer/JWKS URLs, audience, token material, identity identifiers, Valkey details, replica identifiers, provider names, or raw error payloads.
- Do not change refresh coordination, backend JWT verification behavior, Valkey semantics, deployment manifests, or provider-neutral boundaries.
- Existing stdin inheritance, sensitive environment removal, verifier output suppression, and fail-closed preflight/token behavior must remain unchanged.

## Review Focus

- **Malformed or omitted topology:** CLI must emit only fixed `usage` evidence, render topology/canary as `not_run`, and never echo attacker-controlled topology text. Covered in Task 2 CLI tests.
- **Irrelevant single-active canary attestation:** must fail as `usage`, render `single_active` + `not_required`, and perform no preflight or verifier work. Covered in Task 2 semantic-validation tests.
- **Missing multi-active canary evidence:** must fail as `operator_evidence`, render canary `false`, and perform no preflight or verifier work. Covered in Task 2 evidence-ordering tests.
- **Unexpected preflight failure after valid topology/evidence:** must remain `internal`, preserve topology-scoped rendered fields, and never leak dynamic exception text. Covered in Task 1 regression tests.
- **Credential handling regression:** new CLI fields must not create token argv/config surfaces or alter verifier stdin/environment/output suppression. Covered in Task 2 security-regression tests.

---

### Task 1: Add topology-aware acceptance/result model

**Files:**
- Modify: `scripts/run_identity_acceptance.py` — `AcceptanceEvidence`, new `BrowserTopology`, `AcceptanceResult`, `_result()`, and `run_acceptance()` signatures/rendering.
- Modify/Test: `scripts/test_run_identity_acceptance.py` — direct `run_acceptance()` and render-contract tests.

**Interfaces:**
- Consumes: existing `AcceptanceConfig`, `AcceptancePhase`, `run_preflight`, `run_verifier`.
- Produces: `BrowserTopology`; `AcceptanceEvidence(..., refresh_coordination_canary_validated: bool)`; `AcceptanceEvidence.is_complete_for(browser_topology: BrowserTopology) -> bool`; `run_acceptance(config, evidence, browser_topology, *, preflight_fn=..., verifier_fn=...) -> AcceptanceResult`; `AcceptanceResult.browser_topology: BrowserTopology | None`; `AcceptanceResult.refresh_coordination_canary_validated: bool | None`.

- [ ] **Step 1: Write failing model/render tests**

Add tests with exact assertions:

```python
def test_single_active_success_renders_topology_and_not_required_canary():
    # run_acceptance(..., BrowserTopology.SINGLE_ACTIVE)
    # assert phase COMPLETE / ready True
    # assert first two rendered lines are:
    # identity_acceptance.browser_topology=single_active
    # identity_acceptance.refresh_coordination_canary=not_required


def test_multi_active_success_requires_and_renders_true_canary():
    # evidence has refresh_coordination_canary_validated=True
    # run_acceptance(..., BrowserTopology.MULTI_ACTIVE)
    # assert ready True
    # assert rendered topology=multi_active and canary=true


def test_multi_active_missing_canary_stops_before_preflight_and_verifier():
    # evidence base three true, canary false
    # assert OPERATOR_EVIDENCE, ready False
    # assert preflight/verifier not called
    # assert canary=false, issuer_preflight/access_token=not_run


def test_unexpected_preflight_failure_keeps_topology_without_leaking_detail():
    # valid single-active evidence, exploding preflight with sensitive text
    # assert INTERNAL and sensitive text absent from render()
    # assert topology=single_active / canary=not_required
```

Update helper evidence construction so existing direct-run tests explicitly carry the new canary boolean.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
python3 -m unittest scripts.test_run_identity_acceptance -v
```

Expected: FAIL because `BrowserTopology`, the new evidence field, topology argument, and rendered fields do not exist yet.

- [ ] **Step 3: Implement the minimum topology/result model**

In `scripts/run_identity_acceptance.py`:

```python
class BrowserTopology(Enum):
    SINGLE_ACTIVE = "single_active"
    MULTI_ACTIVE = "multi_active"
```

Extend `AcceptanceEvidence` with `refresh_coordination_canary_validated: bool` and implement exactly:

```python
def is_complete_for(self, browser_topology: BrowserTopology) -> bool:
    ...
```

The method requires the existing three evidence booleans for both topologies and additionally requires `refresh_coordination_canary_validated` only for `BrowserTopology.MULTI_ACTIVE`. Do not encode `not_required` as a fake boolean.

Change `run_acceptance()` to:

```python
def run_acceptance(
    config: AcceptanceConfig,
    evidence: AcceptanceEvidence,
    browser_topology: BrowserTopology,
    *,
    preflight_fn: Callable[..., object] = run_preflight,
    verifier_fn: Callable[[AcceptanceConfig], int] = run_verifier,
) -> AcceptanceResult:
```

Extend `AcceptanceResult` with:

```python
browser_topology: BrowserTopology | None
refresh_coordination_canary_validated: bool | None
```

Render topology/canary first, with deterministic status rules from the spec:

```text
None topology -> not_run
single_active -> not_required
multi_active + true -> true
multi_active + false -> false
```

Thread topology through `_result()` rather than reading CLI/global state.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```bash
python3 -m unittest scripts.test_run_identity_acceptance -v
```

Expected: PASS for direct acceptance/model tests, with CLI-specific RED tests deferred to Task 2.

- [ ] **Step 5: Commit Task 1**

```bash
git add scripts/run_identity_acceptance.py scripts/test_run_identity_acceptance.py
git commit -m "feat: model topology-aware identity acceptance"
```

---

### Task 2: Enforce topology-aware CLI semantics and failure ordering

**Files:**
- Modify: `scripts/run_identity_acceptance.py` — `_parser()`, usage-result construction, `main()` semantic validation.
- Modify/Test: `scripts/test_run_identity_acceptance.py` — CLI matrix, ordering, privacy, and credential-handling regression tests.

**Interfaces:**
- Consumes: Task 1 `BrowserTopology`, topology-aware `AcceptanceEvidence`, `AcceptanceResult`, and `run_acceptance()`.
- Produces: CLI flags `--browser-topology single-active|multi-active` and `--refresh-coordination-canary-validated`; fixed usage/operator-evidence behavior required by the spec.

- [ ] **Step 1: Add failing CLI validity-matrix tests**

Extend imports to `from unittest.mock import Mock, patch`.

Add tests proving:

```python
def test_cli_requires_browser_topology_without_echoing_inputs():
    # otherwise-valid args omit --browser-topology
    # assert exit 2, failure_phase=usage
    # assert browser_topology=not_run, canary=not_run


def test_cli_rejects_unknown_topology_without_echoing_value():
    # --browser-topology attacker-controlled-value
    # assert exit 2 and rejected value absent from stdout+stderr


def test_cli_rejects_single_active_canary_as_usage_before_evidence_work():
    # single-active + --refresh-coordination-canary-validated
    # with patch("scripts.run_identity_acceptance.run_acceptance") as mocked:
    # assert exit 2, mocked.assert_not_called()
    # assert topology=single_active, canary=not_required
    # assert browser_flow/rotation/monitoring/preflight/token are not_run


def test_cli_accepts_single_active_without_canary():
    # patch("scripts.run_identity_acceptance.run_acceptance")
    # make mock return a ready AcceptanceResult for SINGLE_ACTIVE
    # assert exit 0
    # assert call args contain BrowserTopology.SINGLE_ACTIVE
    # assert call evidence.refresh_coordination_canary_validated is False


def test_cli_multi_active_without_canary_is_operator_evidence_failure():
    # invoke real main() with base three attestations true, multi-active, no canary
    # assert exit 1, failure_phase=operator_evidence
    # assert topology=multi_active, canary=false


def test_cli_multi_active_with_canary_passes_true_attestation_to_run_acceptance():
    # patch("scripts.run_identity_acceptance.run_acceptance")
    # make mock return a ready AcceptanceResult for MULTI_ACTIVE
    # assert exit 0
    # assert call args contain BrowserTopology.MULTI_ACTIVE
    # assert call evidence.refresh_coordination_canary_validated is True
```

Update the existing token-argument rejection test to include `--browser-topology single-active` so the unknown `--token` remains the reason for usage failure, and prove `secret-token`, issuer, and JWKS URI are still absent from combined output.

- [ ] **Step 2: Add failing credential/failure-ordering regression tests**

Pin these existing guarantees under the new topology contract:

```python
def test_multi_active_missing_canary_never_runs_preflight_or_verifier(): ...
def test_preflight_failure_still_stops_before_verifier_with_valid_topology(): ...
def test_verifier_still_inherits_stdin_and_suppresses_output(): ...
def test_verifier_environment_still_removes_access_token_variables(): ...
```

Do not weaken current assertions on command, cwd, env, stdout/stderr suppression, or absence of explicit stdin replacement.

- [ ] **Step 3: Run tests and verify RED**

Run:

```bash
python3 -m unittest scripts.test_run_identity_acceptance -v
```

Expected: FAIL because the new CLI options and semantic combination checks are not implemented.

- [ ] **Step 4: Implement CLI parsing and semantic validation**

Add required parser input:

```text
--browser-topology single-active|multi-active
```

and boolean flag:

```text
--refresh-coordination-canary-validated
```

Keep `SafeArgumentParser.error()` fixed-text-only so omitted/unknown choices cannot echo attacker-controlled values. Convert the accepted CLI spellings to `BrowserTopology` only after parsing succeeds.

Before normal acceptance work:

- omitted/unknown topology -> parser usage result with `browser_topology=None`;
- `single-active + canary flag` -> semantic usage result with known `BrowserTopology.SINGLE_ACTIVE`, all evidence/network/token fields `not_run`;
- `multi-active + no canary` -> construct evidence with canary false and let topology-dependent `run_acceptance()` return `OPERATOR_EVIDENCE` before network/token work;
- valid single/multi combinations -> call `run_acceptance(config, evidence, browser_topology)`.

Preserve exit codes: success `0`, ordinary not-ready `1`, usage `2`.

- [ ] **Step 5: Run focused Python contracts and verify GREEN**

Run:

```bash
python3 -m unittest scripts.test_run_identity_acceptance -v
python3 -m unittest scripts.test_validate_identity_rollout -v
```

Expected: both PASS.

- [ ] **Step 6: Commit Task 2**

```bash
git add scripts/run_identity_acceptance.py scripts/test_run_identity_acceptance.py
git commit -m "feat: require explicit browser topology acceptance"
```

---

### Task 3: Connect topology-aware acceptance to runbooks and focused CI

**Files:**
- Modify: `docs/identity-production-acceptance.md`
- Modify: `docs/browser-auth-operations.md`
- Modify: `.github/workflows/identity-production-acceptance.yml`

**Interfaces:**
- Consumes: Task 2 CLI/output contract.
- Produces: canonical operator migration/runbook instructions and workflow path coverage for the browser-auth runbook that defines the canary evidence.

- [ ] **Step 1: Update the canonical production-acceptance documentation**

In `docs/identity-production-acceptance.md`, document exactly:

- required `--browser-topology single-active|multi-active` and no default;
- `--refresh-coordination-canary-validated` as multi-active-only operator evidence;
- single-active evidence = browser flow + rotation/rollback + auth/JWKS monitoring;
- multi-active evidence = the above + successful real-environment refresh-coordination canary;
- `single-active + canary` is usage failure;
- missing multi-active canary is `operator_evidence` failure;
- new machine-readable topology/canary fields and fixed ordering;
- `ready=true` is topology-scoped and never changes replicas/traffic automatically;
- old invocations must migrate by adding `--browser-topology single-active`;
- multi-active migration requires the browser runbook canary first.

Update the usage example to explicit `single-active`; add a separate short multi-active invocation showing both topology and canary flag without duplicating token/secret guidance.

- [ ] **Step 2: Connect the browser canary to the acceptance gate**

In `docs/browser-auth-operations.md`, after the existing controlled two-active-replica canary review step, require operators to run topology-aware production acceptance with:

```text
--browser-topology multi-active
--refresh-coordination-canary-validated
```

and require `identity_acceptance.ready=true` with `browser_topology=multi_active` before the external deployment layer permits sustained multi-active browser-session traffic.

Do not duplicate the full runner spec and do not remove the current one-active restriction before that gate.

- [ ] **Step 3: Extend focused workflow path coverage**

In `.github/workflows/identity-production-acceptance.yml`, add `docs/browser-auth-operations.md` to both `pull_request.paths` and `push.paths`.

Do not add a workflow or change the existing Python/Rust test commands.

- [ ] **Step 4: Verify documentation/workflow consistency**

Run:

```bash
python3 -m unittest scripts.test_validate_identity_rollout -v
python3 -m unittest scripts.test_run_identity_acceptance -v
cargo test --locked --manifest-path backend/Cargo.toml --bin verify_identity_access_token
```

Expected: PASS.

Review the docs for exact spellings:

```text
single-active / multi-active    # CLI
single_active / multi_active    # output
not_required                    # single-active canary output
operator_evidence               # missing multi-active canary
```

- [ ] **Step 5: Commit Task 3**

```bash
git add docs/identity-production-acceptance.md docs/browser-auth-operations.md .github/workflows/identity-production-acceptance.yml
git commit -m "docs: connect topology acceptance to browser canary"
```

---

### Task 4: Final verification, review, PR, and post-merge issue bookkeeping

**Files:**
- Verify: all files changed in Tasks 1-3.
- Post-merge issue updates: GitHub issues #2, #10, #13 only; do not close #10 or #13.

**Interfaces:**
- Consumes: complete implementation from Tasks 1-3.
- Produces: reviewable implementation PR with fresh exact-head evidence; after merge, accurate production-readiness tracking.

- [ ] **Step 1: Run the focused identity acceptance gate on the exact final head**

Run:

```bash
python3 -m unittest scripts.test_validate_identity_rollout -v
python3 -m unittest scripts.test_run_identity_acceptance -v
cargo test --locked --manifest-path backend/Cargo.toml --bin verify_identity_access_token
```

Expected: all PASS.

- [ ] **Step 2: Run repository-level verification appropriate to the affected surface**

At minimum require the exact implementation PR head to pass GitHub Actions for:

```text
Identity production acceptance
Provider-neutral auth boundary
Provider-neutral auth E2E
Identity refresh coordination
CI
```

If path filters cause a workflow not to run, explicitly verify why; do not claim it passed when it did not execute.

- [ ] **Step 3: Perform whole-diff code review**

Review for:

- no raw topology echo through argparse/errors;
- no new token argv/config surface;
- topology-dependent evidence cannot accidentally report multi-active ready without canary;
- single-active ready cannot be mistaken for multi-active ready in rendered output/docs;
- no changes to issuer/JWKS policy, Rust verifier semantics, refresh coordination, Valkey, or deployment manifests;
- workflow triggers include both canonical acceptance docs and browser canary runbook.

Any finding must be validated as a real defect before changing code; add a regression test first for behavioral fixes.

- [ ] **Step 4: Open/update the implementation PR with evidence**

PR summary must state:

- intentional breaking CLI requirement for `--browser-topology`;
- topology-dependent evidence semantics;
- exact failure ordering;
- TDD RED/GREEN evidence;
- production constraint that the runner does not change topology;
- verification results on the exact PR head.

- [ ] **Step 5: Merge only after exact-head gates and review are clean**

Use squash merge. Re-read PR head SHA immediately before merging and pass it as the expected head SHA.

- [ ] **Step 6: Verify post-merge main**

Confirm the relevant push workflows on the merge SHA complete successfully. A PR-only success is not enough for a post-merge completion claim.

- [ ] **Step 7: Update issue #10 without closing it**

Record that repository-side browser auth, distributed refresh coordination, refresh observability, and topology-aware acceptance are implemented. Replace the stale future-coordination note with the remaining real-environment work: production client/configuration, two-replica canary evidence, and actual deployment-policy change.

- [ ] **Step 8: Update issue #13 without closing it**

Record the new topology-aware acceptance gate while keeping real issuer/client registration, token-profile validation, rotation/rollback, monitoring, and candidate-environment evidence open.

- [ ] **Step 9: Update issue #2 without closing production readiness prematurely**

Add the topology-aware acceptance work to completed repository-side identity hardening and keep external production-rollout items explicitly outstanding.
