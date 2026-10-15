# Topology-Aware Production Identity Acceptance Design

**Date:** 2026-10-15  
**Status:** review-required  
**Scope:** provider-neutral production identity acceptance semantics for single-active and multi-active browser BFF topologies  
**Related:** `scripts/run_identity_acceptance.py`, `scripts/test_run_identity_acceptance.py`, `docs/identity-production-acceptance.md`, `docs/browser-auth-operations.md`, `.github/workflows/identity-production-acceptance.yml`, issue #2, issue #10, issue #13, PR #180

## Goal

Make `scripts.run_identity_acceptance` explicitly topology-aware so `identity_acceptance.ready=true` has a precise production meaning for both single-active and multi-active SvelteKit BFF deployments.

The current acceptance runner already aggregates provider-neutral production evidence for:

- real browser login/callback/refresh/logout behavior;
- signing-key rotation and rollback;
- authentication/JWKS monitoring;
- issuer/discovery/JWKS preflight;
- canonical Rust resource-server verification of a real access token.

PR #180 added refresh-coordination observability and a concrete two-active-replica canary contract, while `docs/browser-auth-operations.md` continues to require one active BFF replica for browser-session traffic until that real-environment canary succeeds.

The missing contract is that the current runner does not know whether the operator is accepting a single-active or multi-active browser topology. As a result, `identity_acceptance.ready=true` does not currently distinguish:

- production identity readiness while intentionally remaining single-active; from
- production identity readiness for a multi-active browser-session topology after refresh-coordination canary validation.

The target design makes topology intent explicit and requires multi-active-only evidence without adding provider-specific configuration or turning the acceptance runner into a deployment controller.

## Non-goals

This change does not:

- run a two-replica canary automatically;
- provision or configure Valkey;
- deploy or configure an identity provider;
- register production OAuth/OIDC clients;
- obtain, persist, refresh, or expose production credentials;
- alter the refresh-coordination protocol, lease/TTL values, cryptography, replay guard, or result handoff;
- change production replica counts or routing;
- call Kubernetes, ECS, Nomad, or another deployment API;
- create dashboards or alert rules;
- collect or persist canary evidence files automatically;
- add provider/vendor-specific identity claims or SDKs;
- expose topology identifiers, instance IDs, hostnames, Valkey details, credentials, or raw error data;
- make `identity_acceptance.ready=true` trigger an automatic deployment or traffic shift;
- close issue #10 or issue #13 solely because the repository-side gate exists.

## Current state

### Existing production acceptance runner

`scripts.run_identity_acceptance` currently accepts a standards-only issuer/JWKS/audience/signature/token-type policy and three operator evidence flags:

```text
--browser-flow-validated
--rotation-rollback-validated
--monitoring-validated
```

The current fail-closed order is:

1. operator evidence completeness;
2. issuer/discovery/JWKS preflight;
3. real access-token verification through the canonical Rust verifier.

The runner emits stable machine-readable booleans and one fixed failure phase. It deliberately suppresses child verifier output and does not emit issuer/JWKS URLs, audience values, token data, identity claims, subjects, `kid`, or provider-specific diagnostics.

### Current multi-replica restriction

`docs/browser-auth-operations.md` defines shared Valkey-backed refresh coordination and its real-environment deployment sequence.

The application supports the multi-replica coordination protocol, but production browser-session traffic must remain on one active BFF replica until a controlled two-active-replica canary succeeds.

That canary already requires:

- exactly one upstream refresh-token redemption for one old V2 session generation;
- at least one `leader_success` observation;
- at least one `follower_success` observation;
- zero abnormal outcomes during the controlled healthy handoff rehearsal;
- coordination-outage behavior without process-local refresh fallback;
- fixed browser-session lifetime behavior;
- refresh telemetry privacy validation;
- deployed monitoring-path visibility;
- no unexplained login-rate or unauthorized-rate regression.

### Contract gap

The repository therefore has two valid production states:

```text
single-active identity acceptance
multi-active identity acceptance
```

but the current runner reports only:

```text
identity_acceptance.ready=true|false
```

without saying which topology the evidence supports.

## Chosen architecture

Extend the existing production identity acceptance runner rather than introducing a second top-level readiness command.

The runner remains the single provider-neutral aggregation gate owned by this repository, but the operator must explicitly declare the intended browser topology.

New required CLI input:

```text
--browser-topology single-active|multi-active
```

New multi-active-only evidence flag:

```text
--refresh-coordination-canary-validated
```

No default topology is provided.

This is intentionally a breaking CLI change because silent inference of `single-active` would preserve the ambiguity this design is intended to remove.

## Browser topology model

Represent topology as a closed enum in Python:

```python
class BrowserTopology(Enum):
    SINGLE_ACTIVE = "single_active"
    MULTI_ACTIVE = "multi_active"
```

CLI spelling uses hyphens for operator ergonomics:

```text
single-active
multi-active
```

Rendered machine-readable output uses underscore values:

```text
single_active
multi_active
```

The accepted set is closed. Arbitrary topology names are invalid usage.

## Evidence model

Extend `AcceptanceEvidence` with:

```text
browser_flow_validated
rotation_rollback_validated
monitoring_validated
refresh_coordination_canary_validated
```

Evidence completeness is topology-dependent.

### Single-active

Required operator evidence:

```text
browser_flow_validated
rotation_rollback_validated
monitoring_validated
```

Refresh-coordination canary evidence is not required because the deployment remains constrained to one active BFF replica for browser-session traffic.

### Multi-active

Required operator evidence:

```text
browser_flow_validated
rotation_rollback_validated
monitoring_validated
refresh_coordination_canary_validated
```

The additional flag represents real candidate-environment evidence that the canary defined in `docs/browser-auth-operations.md` succeeded.

The runner does not independently verify that the operator attestation is truthful. It prevents an incomplete evidence set from being mistaken for topology readiness.

## CLI validity matrix

The CLI contract is fixed as follows:

| Browser topology | Canary flag | Result |
| --- | --- | --- |
| `single-active` | absent | valid |
| `single-active` | present | usage failure |
| `multi-active` | absent | operator evidence failure |
| `multi-active` | present | valid |
| omitted | either | usage failure |
| any unknown value | either | usage failure |

`single-active + --refresh-coordination-canary-validated` is rejected instead of silently ignored. A canary attestation that is irrelevant to the declared topology should not be accepted as meaningful evidence.

## Failure ordering

Preserve the existing fail-closed phases and extend only the preconditions.

The order becomes:

1. CLI syntax and topology/canary consistency;
2. topology-dependent operator evidence completeness;
3. issuer/discovery/JWKS preflight;
4. real access-token verification.

No new failure phase is required for the canary.

Missing multi-active canary evidence is still an operator-evidence failure:

```text
identity_acceptance.failure_phase=operator_evidence
```

This keeps failure taxonomy stable and correctly models the canary as an externally established production evidence requirement.

A failure at steps 1 or 2 prevents issuer/JWKS network activity and prevents the real access token from being consumed by the verifier.

## `ready=true` semantics

`identity_acceptance.ready=true` means:

> The provider-neutral production identity acceptance evidence owned by this repository is complete for the explicitly declared browser topology.

### Single-active readiness

For `single-active`, `ready=true` means all of the following passed:

- real browser-flow evidence;
- signing-key rotation and rollback evidence;
- authentication/JWKS monitoring evidence;
- issuer/discovery/JWKS preflight;
- real access-token verification using the intended production policy.

It does **not** authorize multi-active browser-session traffic.

### Multi-active readiness

For `multi-active`, `ready=true` means the single-active evidence above passed and the operator additionally attested that the real-environment refresh-coordination canary succeeded.

It means the repository-level identity acceptance contract has been satisfied for a multi-active browser topology. It still does not change replica count, traffic routing, or deployment policy automatically.

## Machine-readable output contract

Add two stable output fields before the existing evidence booleans:

```text
identity_acceptance.browser_topology=<single_active|multi_active|not_run>
identity_acceptance.refresh_coordination_canary=<true|false|not_required|not_run>
```

Existing fields remain:

```text
identity_acceptance.browser_flow=<true|false|not_run>
identity_acceptance.rotation_rollback=<true|false|not_run>
identity_acceptance.monitoring=<true|false|not_run>
identity_acceptance.issuer_preflight=<true|false|not_run>
identity_acceptance.access_token=<true|false|not_run>
identity_acceptance.ready=<true|false>
```

A failing result continues to append exactly one stable failure phase.

### Successful single-active example

```text
identity_acceptance.browser_topology=single_active
identity_acceptance.refresh_coordination_canary=not_required
identity_acceptance.browser_flow=true
identity_acceptance.rotation_rollback=true
identity_acceptance.monitoring=true
identity_acceptance.issuer_preflight=true
identity_acceptance.access_token=true
identity_acceptance.ready=true
```

### Successful multi-active example

```text
identity_acceptance.browser_topology=multi_active
identity_acceptance.refresh_coordination_canary=true
identity_acceptance.browser_flow=true
identity_acceptance.rotation_rollback=true
identity_acceptance.monitoring=true
identity_acceptance.issuer_preflight=true
identity_acceptance.access_token=true
identity_acceptance.ready=true
```

### Multi-active evidence failure

```text
identity_acceptance.browser_topology=multi_active
identity_acceptance.refresh_coordination_canary=false
identity_acceptance.browser_flow=true
identity_acceptance.rotation_rollback=true
identity_acceptance.monitoring=true
identity_acceptance.issuer_preflight=not_run
identity_acceptance.access_token=not_run
identity_acceptance.ready=false
identity_acceptance.failure_phase=operator_evidence
```

### Topology parse failure

If topology cannot be parsed, neither topology nor canary semantics are trusted:

```text
identity_acceptance.browser_topology=not_run
identity_acceptance.refresh_coordination_canary=not_run
identity_acceptance.browser_flow=not_run
identity_acceptance.rotation_rollback=not_run
identity_acceptance.monitoring=not_run
identity_acceptance.issuer_preflight=not_run
identity_acceptance.access_token=not_run
identity_acceptance.ready=false
identity_acceptance.failure_phase=usage
```

### Single-active with irrelevant canary flag

The topology itself is known, but the combination is invalid usage:

```text
identity_acceptance.browser_topology=single_active
identity_acceptance.refresh_coordination_canary=not_required
identity_acceptance.ready=false
identity_acceptance.failure_phase=usage
```

Other evidence fields may remain `not_run` because the invalid combination is rejected before evidence evaluation.

## Monitoring evidence separation

Do not broaden `--monitoring-validated` to silently absorb refresh-coordination canary evidence.

`--monitoring-validated` continues to mean the candidate deployment exposes the existing production authentication/JWKS observability contract, including:

- authentication success/failure aggregates;
- JWKS scheduled/forced refresh outcomes;
- stale fallback and throttling where applicable;
- dependency failure categories;
- operational alert/on-call visibility;
- absence of prohibited credential or identity values.

`--refresh-coordination-canary-validated` specifically represents the multi-active browser refresh contract, including:

- two-active-replica rehearsal;
- exactly-one upstream redemption;
- leader/follower handoff observations;
- coordination-outage behavior;
- fixed session-lifetime behavior;
- refresh-coordination telemetry privacy;
- refresh-coordination monitoring-path visibility.

Keeping these attestations separate avoids redefining an existing flag and preserves clear operational ownership.

## Security and privacy boundary

The topology-aware extension must preserve the current acceptance runner privacy guarantees.

The runner must not emit:

- access tokens or refresh tokens;
- browser cookies or sealed session state;
- authorization codes;
- `sub`, user UUID, email, `jti`, `kid`, or other identity identifiers;
- issuer, JWKS, discovery, authorization, or token endpoint URLs;
- audience values;
- Valkey/Redis URLs, hosts, usernames, credentials, or topology details;
- BFF instance IDs, hostnames, pod names, task IDs, or replica identifiers;
- provider/vendor/product names;
- raw exception text;
- raw preflight, verifier, OIDC, or coordination error payloads.

The new rendered values are only closed topology/status enums.

A usage failure caused by malformed topology input must not echo the rejected user-supplied value.

## Migration contract

The CLI change is intentionally explicit.

### Existing invocation

Current operator commands omit topology:

```bash
python3 -m scripts.run_identity_acceptance \
  ... \
  --browser-flow-validated \
  --rotation-rollback-validated \
  --monitoring-validated
```

After this change, that command fails with `failure_phase=usage`.

### Single-active migration

Existing production/staging commands must add:

```text
--browser-topology single-active
```

The candidate environment can then continue to use the existing acceptance evidence while the deployment remains one active browser-session BFF replica.

### Multi-active migration

The controlled migration sequence is:

1. update operator commands to explicit `single-active` and confirm acceptance still passes;
2. keep browser-session traffic on one active BFF replica;
3. enable and validate shared refresh coordination on one replica;
4. execute the controlled two-active-replica canary from `docs/browser-auth-operations.md`;
5. review canary and monitoring evidence;
6. re-run production acceptance with `--browser-topology multi-active --refresh-coordination-canary-validated`;
7. require `identity_acceptance.ready=true` for `browser_topology=multi_active`;
8. only then may the external deployment layer change policy to permit sustained multi-active browser-session traffic.

The repository runner does not perform step 8.

## Documentation changes

### `docs/identity-production-acceptance.md`

This is the canonical documentation for the topology-aware runner.

Update it to document:

- required `--browser-topology` input;
- topology-dependent evidence rules;
- multi-active canary attestation semantics;
- machine-readable output fields;
- fail-closed ordering;
- intentional CLI migration;
- exact meaning of `ready=true` for each topology;
- explicit warning that single-active readiness is not multi-active readiness;
- explicit warning that the runner does not deploy or shift traffic.

### `docs/browser-auth-operations.md`

Do not duplicate the full acceptance-runner specification.

Add one link in the deployment sequence after successful canary review requiring the operator to run:

```text
scripts.run_identity_acceptance
--browser-topology multi-active
--refresh-coordination-canary-validated
```

and obtain topology-scoped `ready=true` before external deployment policy is changed.

## Expected implementation boundary

Expected production changes:

```text
scripts/run_identity_acceptance.py
scripts/test_run_identity_acceptance.py
docs/identity-production-acceptance.md
docs/browser-auth-operations.md
.github/workflows/identity-production-acceptance.yml
```

No frontend refresh implementation file, backend JWT verifier behavior, Valkey store implementation, or deployment manifest should change.

## TDD strategy

Implementation follows RED -> GREEN -> review -> regression hardening.

Add acceptance-runner tests before production changes.

Required RED coverage:

- omitted topology -> usage failure;
- unknown topology -> usage failure without echoing the invalid value;
- `single-active` + base three operator attestations -> ready without canary;
- `single-active` + canary flag -> usage failure;
- `multi-active` + missing canary -> `operator_evidence` failure;
- missing multi-active canary stops before preflight;
- missing multi-active canary stops before verifier;
- `multi-active` + canary + base evidence -> preflight then verifier;
- successful single-active output includes `browser_topology=single_active`;
- successful single-active output includes `refresh_coordination_canary=not_required`;
- successful multi-active output includes `browser_topology=multi_active`;
- successful multi-active output includes `refresh_coordination_canary=true`;
- usage failure before topology parsing renders topology/canary `not_run`;
- invalid single-active/canary combination renders fixed values without dynamic input;
- existing issuer/JWKS/audience/token privacy assertions remain true;
- verifier stdin inheritance and output suppression remain unchanged;
- existing failure ordering for preflight and verifier remains unchanged once operator evidence is complete.

Do not weaken existing assertions to accommodate the new fields. Update exact rendered-output expectations intentionally.

## CI strategy

Do not add a new workflow.

The existing `.github/workflows/identity-production-acceptance.yml` remains the focused contract gate.

It continues to run:

```text
scripts.test_validate_identity_rollout
scripts.test_run_identity_acceptance
backend/src/bin/verify_identity_access_token.rs contract tests
```

Add `docs/browser-auth-operations.md` to its pull-request and `main` push path filters so changes to the canary/acceptance connection trigger the focused contract.

The implementation PR must also pass relevant repository-wide gates for the exact final head, including normal `CI` and provider-neutral authentication workflows triggered by the changed paths.

## Issue updates after implementation merge

### Issue #10

Update the stale multi-replica statement.

Record that repository-side work now includes:

- provider-neutral browser auth;
- shared cross-replica refresh coordination;
- refresh-coordination observability;
- topology-aware production acceptance.

Leave the issue open because real production client/configuration and candidate-environment acceptance remain external rollout work.

### Issue #13

Leave open.

The repository-side gate does not deploy the production issuer, register the real client, acquire a real token, perform real signing-key rotation/rollback, create monitoring infrastructure, or prove operator attestations.

Add a progress note only after implementation merges.

### Issue #2

Add the topology-aware acceptance work to production-readiness progress after merge.

Do not mark overall production readiness complete solely because the repository gate exists.

## Rollback and compatibility

The runner change is an operator-interface migration, not a runtime authentication migration.

If rollout tooling has not yet been updated, the new runner safely fails with `failure_phase=usage` instead of silently assuming a topology.

Runtime browser authentication behavior is unaffected.

No browser session cookie schema, JWT contract, refresh-token behavior, Valkey state, or backend persistence migration is required.

## Acceptance criteria

The change is complete when all of the following are true:

- topology is a required closed input;
- there is no implicit default topology;
- single-active readiness does not require refresh-coordination canary evidence;
- multi-active readiness does require canary evidence;
- irrelevant canary evidence on single-active is rejected;
- incomplete multi-active evidence fails before any issuer/JWKS preflight or token verification;
- `ready=true` is always rendered alongside an explicit topology;
- the output distinguishes canary `true`, `false`, `not_required`, and `not_run` states as designed;
- malformed input is not reflected into output;
- current credential/privacy protections remain intact;
- existing preflight and canonical Rust verification remain unchanged;
- browser operations documentation links successful canary evidence to topology-aware production acceptance;
- focused identity-production-acceptance CI covers the changed contract;
- issue #10, #13, and #2 are updated after merge without prematurely closing production rollout work.
