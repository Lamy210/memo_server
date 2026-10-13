# Production identity acceptance gate

## Purpose

`scripts.run_identity_acceptance` is the final provider-neutral operator gate for the production identity boundary tracked by issue #13.

It composes two existing automated checks:

1. the issuer/discovery/JWKS preflight from `scripts.validate_identity_rollout`;
2. the canonical Rust resource-server verification path from `verify_identity_access_token`.

It also requires explicit operator attestations for the rollout checks that cannot be proven from one token or one HTTP request:

- staging browser login/callback/refresh/logout behavior;
- signing-key rotation **and rollback** across the required overlap/token-validity horizon;
- authentication/JWKS monitoring visibility and alerting.

`identity_acceptance.ready=true` therefore means all three operator evidence categories are asserted **and** both automated checks passed with the intended production policy.

## Security boundary

A real access token remains a credential.

The acceptance runner therefore:

- never accepts an access token through argv;
- never accepts an access token through its configuration flags;
- does not read or copy stdin token bytes in Python;
- runs issuer/JWKS validation before starting the token verifier;
- passes stdin directly to the existing Rust verifier, which applies the bounded-input and zeroization behavior documented in `identity-access-token-preflight.md`;
- removes common inherited access-token environment variable names before launching the verifier;
- suppresses child verifier output and emits only stable acceptance booleans/failure phases;
- normalizes unexpected preflight exceptions to a fixed `internal` phase instead of echoing dynamic exception details;
- never emits issuer URLs, JWKS URLs, audience values, token contents, claims, subjects, `kid`, or provider-specific diagnostics.

Disable shell tracing before obtaining or piping a token. Never place a production token in a command argument, CI variable dump, issue, pull request, chat message, or shell history.

## Operator evidence

Only set the evidence flags after the corresponding staging evidence exists.

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

Assert this only after the candidate deployment's operational telemetry is visible in the intended monitoring path:

- authentication success/failure aggregates can be observed;
- JWKS scheduled/forced refresh outcomes can be observed;
- stale fallback, throttling, dependency failure, and invalid-response categories can be distinguished where applicable;
- the alert/on-call path for meaningful authentication or JWKS failure has been exercised;
- raw bearer tokens, refresh/session credentials, subject identifiers, raw `kid`, issuer/JWKS URLs, and provider-specific implementation details are not exposed by the monitoring surface.

See `identity-auth-observability.md` for the stable event boundary.

## Usage

Run from the repository root. Use the exact policy intended for production; do not loosen the signature, token-type, audience, or lifetime policy only to make acceptance pass.

Example:

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
  --browser-flow-validated \
  --rotation-rollback-validated \
  --monitoring-validated
status=$?
unset ACCESS_TOKEN
exit "$status"
```

The shell variable above is intentionally not exported. Prefer direct piping from an approved token acquisition mechanism when possible.

The runner invokes:

```text
cargo run --locked --quiet --manifest-path backend/Cargo.toml --bin verify_identity_access_token
```

so a working Rust/Cargo toolchain is required when using the repository runner.

## Output and exit status

Success exits `0` and emits only:

```text
identity_acceptance.browser_flow=true
identity_acceptance.rotation_rollback=true
identity_acceptance.monitoring=true
identity_acceptance.issuer_preflight=true
identity_acceptance.access_token=true
identity_acceptance.ready=true
```

Failure exits non-zero. Unexecuted phases are reported as `not_run`, and one stable failure phase is emitted:

```text
identity_acceptance.failure_phase=operator_evidence
identity_acceptance.failure_phase=issuer_preflight
identity_acceptance.failure_phase=access_token
identity_acceptance.failure_phase=internal
identity_acceptance.failure_phase=usage
```

The order is fail-closed:

1. operator evidence completeness;
2. issuer/discovery/JWKS preflight;
3. real access-token verification.

An unexpected internal failure also fails closed and emits only the fixed `internal` category. A failure in an earlier phase prevents later phases from running, so a real token is not unnecessarily consumed when rollout evidence or issuer/JWKS acceptance is already incomplete.

## What `ready=true` does and does not mean

`ready=true` is a machine-readable aggregation of the production acceptance evidence owned by this repository's identity contract. It is appropriate as a cutover gate only when the operator flags truthfully refer to the candidate staging deployment that will become production.

The runner does **not**:

- deploy or configure an identity provider;
- register a client automatically;
- obtain, store, refresh, or persist an access token;
- perform signing-key rotation itself;
- perform rollback itself;
- create monitoring dashboards or alerts;
- prove that an operator attestation is truthful.

Those actions remain owned by the independently operated identity/deployment layer. The runner's job is to prevent an incomplete set of evidence from being mistaken for production acceptance and to exercise the standards-only `memo_server` contract with the intended policy.
