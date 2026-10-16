# Production identity acceptance gate

## Purpose

`scripts.run_identity_acceptance` is the final provider-neutral operator gate for the production identity boundary tracked by issue #13.

It composes two existing automated checks:

1. the issuer/discovery/JWKS preflight from `scripts.validate_identity_rollout`;
2. the canonical Rust resource-server verification path from `verify_identity_access_token`.

It also requires explicit operator attestations for rollout checks that cannot be proven from one token or one HTTP request:

- staging browser login/callback/refresh/logout behavior;
- signing-key rotation **and rollback** across the required overlap/token-validity horizon;
- authentication/JWKS monitoring visibility and alerting;
- for a declared `multi-active` browser topology only, the real-environment refresh-coordination canary defined in `browser-auth-operations.md`.

The target browser topology is mandatory. `identity_acceptance.ready=true` means the evidence owned by this repository is complete **for that explicitly declared topology**. A successful `single-active` result does not authorize multi-active browser-session traffic.

## Security boundary

A real access token remains a credential.

The acceptance runner therefore:

- never accepts an access token through argv;
- never accepts an access token through its configuration flags;
- does not read or copy stdin token bytes in Python;
- runs CLI/topology and operator-evidence checks before issuer/JWKS validation or token verification;
- passes stdin directly to the existing Rust verifier, which applies the bounded-input and zeroization behavior documented in `identity-access-token-preflight.md`;
- removes common inherited access-token environment variable names before launching the verifier;
- suppresses child verifier output and emits only stable acceptance values/failure phases;
- normalizes unexpected preflight exceptions to a fixed `internal` phase instead of echoing dynamic exception details;
- never emits rejected topology input, issuer URLs, JWKS URLs, audience values, token contents, claims, subjects, `kid`, Valkey details, replica identifiers, or provider-specific diagnostics.

Disable shell tracing before obtaining or piping a token. Never place a production token in a command argument, CI variable dump, issue, pull request, chat message, or shell history.

## Browser topology

The required option is:

```text
--browser-topology single-active|multi-active
```

There is no default. Existing operator commands must explicitly add `--browser-topology single-active` before adopting this runner version.

CLI spelling uses hyphens, while machine-readable output uses underscores:

```text
single-active -> single_active
multi-active  -> multi_active
```

The validity contract is:

| Browser topology | `--refresh-coordination-canary-validated` | Result |
| --- | --- | --- |
| `single-active` | absent | valid |
| `single-active` | present | `usage` failure |
| `multi-active` | absent | `operator_evidence` failure |
| `multi-active` | present | valid |
| omitted/unknown | either | `usage` failure |

Malformed topology input is never echoed in the stable output.

## Operator evidence

Only set evidence flags after the corresponding staging/candidate-environment evidence exists.

### `--browser-flow-validated`

Assert this only after the intended production client registration has been exercised in staging and all of the following are true:

- Authorization Code + PKCE login succeeds through the real browser/BFF boundary;
- registered callback origin/path is correct;
- refresh/session rotation succeeds;
- logout clears the local session and the intended logout/return behavior is correct;
- browser JavaScript does not receive access tokens, refresh credentials, authorization codes after callback processing, or session secrets.

The repository E2E regression protects the generic implementation, but this flag represents evidence from the candidate staging identity configuration.

### `--rotation-rollback-validated`

Assert this only after the staging signing-key exercise in `identity-key-rotation-rollout.md` has been completed:

- old and new verification keys overlap;
- both old-token and new-token verification succeeds during overlap;
- issuance can cut over to the new key;
- rollback to the old key succeeds while the overlap remains;
- a verification key is not retired until every token it signed is beyond its validity horizon plus clock-skew and operational margin.

### `--monitoring-validated`

Assert this only after the candidate deployment's authentication/JWKS telemetry is visible in the intended monitoring path:

- authentication success/failure aggregates can be observed;
- JWKS scheduled/forced refresh outcomes can be observed;
- stale fallback, throttling, dependency failure, and invalid-response categories can be distinguished where applicable;
- the alert/on-call path for meaningful authentication or JWKS failure has been exercised;
- raw bearer tokens, refresh/session credentials, subject identifiers, raw `kid`, issuer/JWKS URLs, and provider-specific implementation details are not exposed by the monitoring surface.

This flag keeps its existing authentication/JWKS meaning. It does **not** absorb refresh-coordination canary evidence. See `identity-auth-observability.md` for the stable event boundary.

### `--refresh-coordination-canary-validated`

This flag is valid only with `--browser-topology multi-active`.

Assert it only after the controlled real-environment canary in `browser-auth-operations.md` has succeeded, including:

- two active browser BFF replicas participate in the rehearsal;
- one old V2 session generation causes exactly one upstream refresh-token redemption;
- both `leader_success` and `follower_success` are observed;
- the controlled healthy handoff has none of the documented abnormal outcomes;
- coordination outage and fixed-session-lifetime failure behavior is exercised without process-local fallback or lifetime extension;
- refresh-coordination telemetry contains no prohibited credential, identity, provider, coordination, topology, connection, or raw-error values;
- the fixed refresh categories are visible through the intended monitoring path.

The runner treats this as operator attestation. It does not run the canary or prove the attestation independently.

## Topology-dependent evidence

For `single-active`, operator evidence is complete when these three flags are true:

```text
--browser-flow-validated
--rotation-rollback-validated
--monitoring-validated
```

For `multi-active`, all four are required:

```text
--browser-flow-validated
--rotation-rollback-validated
--monitoring-validated
--refresh-coordination-canary-validated
```

Missing multi-active canary evidence fails closed as `identity_acceptance.failure_phase=operator_evidence` before issuer/JWKS network work and before the real token verifier starts.

## Usage

Run from the repository root. Use the exact policy intended for production; do not loosen the signature, token-type, audience, lifetime, or topology policy only to make acceptance pass.

### Single-active

Existing production/staging invocations migrate by adding the explicit topology:

```bash
set +x
read -rs ACCESS_TOKEN
printf '%s\n' "$ACCESS_TOKEN" | python3 -m scripts.run_identity_acceptance \
  --issuer 'https://id.example.invalid' \
  --jwks-uri 'https://id.example.invalid/.well-known/jwks.json' \
  --audience 'memo-api' \
  --signature-mode 'es384' \
  --token-type-mode 'at-jwt' \
  --max-access-token-lifetime-seconds 300 \
  --browser-topology single-active \
  --browser-flow-validated \
  --rotation-rollback-validated \
  --monitoring-validated
status=$?
unset ACCESS_TOKEN
exit "$status"
```

### Multi-active

Only after the controlled browser refresh canary succeeds, use the same production policy and evidence plus:

```text
--browser-topology multi-active
--refresh-coordination-canary-validated
```

Do not change sustained deployment topology merely because the application code supports shared coordination. Require a successful topology-scoped acceptance result first.

The shell variable in the single-active example is intentionally not exported. Prefer direct piping from an approved token acquisition mechanism when possible.

The runner invokes:

```text
cargo run --locked --quiet --manifest-path backend/Cargo.toml --bin verify_identity_access_token
```

so a working Rust/Cargo toolchain is required when using the repository runner.

## Output and exit status

The first two fields always identify the topology scope and refresh-canary state.

Successful single-active acceptance exits `0` and emits:

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

Successful multi-active acceptance exits `0` and emits:

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

A multi-active request without canary evidence reports:

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

A topology parse/required-argument failure reports topology and canary as `not_run`. `single-active` combined with the irrelevant canary flag is a `usage` failure with `browser_topology=single_active`, `refresh_coordination_canary=not_required`, and all evidence/automated phases `not_run`.

Failure exits non-zero. Exactly one stable failure phase is emitted:

```text
identity_acceptance.failure_phase=operator_evidence
identity_acceptance.failure_phase=issuer_preflight
identity_acceptance.failure_phase=access_token
identity_acceptance.failure_phase=internal
identity_acceptance.failure_phase=usage
```

The order is fail-closed:

1. CLI syntax and topology/canary consistency;
2. topology-dependent operator evidence completeness;
3. issuer/discovery/JWKS preflight;
4. real access-token verification.

An unexpected internal failure also fails closed and emits only the fixed `internal` category. A failure in an earlier phase prevents later phases from running, so a real token is not unnecessarily consumed when topology, rollout evidence, or issuer/JWKS acceptance is already incomplete.

## What `ready=true` does and does not mean

`identity_acceptance.ready=true` means the provider-neutral production identity evidence owned by this repository is complete for `identity_acceptance.browser_topology`.

For `single_active`, it means browser-flow, key-rotation/rollback, authentication/JWKS monitoring, issuer/JWKS preflight, and real access-token verification are accepted while browser-session traffic remains single-active. It does **not** authorize multi-active browser-session traffic.

For `multi_active`, it additionally means the operator attested that the real refresh-coordination canary succeeded. It is the repository-level identity gate required before an independently operated deployment layer may permit sustained multi-active browser-session traffic.

The runner does **not**:

- deploy or configure an identity provider;
- register a client automatically;
- obtain, store, refresh, or persist an access token;
- perform signing-key rotation itself;
- perform rollback itself;
- run the two-replica refresh canary itself;
- create monitoring dashboards or alerts;
- change replica counts or traffic routing;
- prove that an operator attestation is truthful.

Those actions remain owned by the independently operated identity/deployment layer. The runner's job is to prevent an incomplete or topology-ambiguous evidence set from being mistaken for production acceptance and to exercise the standards-only `memo_server` contract with the intended policy.
