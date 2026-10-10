# Provider-Neutral Identity Boundary Design

**Date:** 2026-10-11  
**Status:** approved  
**Scope:** `memo_server` authentication integration boundary

## Goal

Define an authentication boundary that lets `memo_server` interoperate with an independently operated identity platform without coupling the application to any particular identity product, vendor SDK, internal repository, internal hostname, database, session API, or provider-specific claim.

The application-facing contract must remain stable when the identity implementation is replaced, its internal topology changes, or additional upstream identity providers are federated behind it.

Success means:

- `memo_server` validates only standards-based access-token metadata and claims;
- browser and public API flows do not require implementation-specific APIs or fields;
- implementation details do not intentionally appear in public interfaces, runtime errors, public cookies/headers, current public documentation, or stable configuration names;
- replacing the identity implementation does not require memo-domain or memo-persistence migration;
- local development remains independent of production identity infrastructure;
- authentication remains secure even if an observer identifies the underlying implementation.

This is implementation abstraction, not security by obscurity. Concealing a product name is never a trust control.

## Chosen architecture

Use an **opaque, provider-neutral identity boundary**.

```text
Browser
  |
  v
SvelteKit frontend / BFF
  |
  +--> Public Identity Boundary
  |      - login/session/refresh
  |      - standards-based authorization endpoints
  |      - provider-neutral public origin
  |
  +--> memo_server
         Authorization: Bearer <short-lived access token>
         |
         +--> signature / issuer / audience / time validation
         +--> sub -> memo user UUID
         +--> memo authorization
```

The identity platform may internally federate to local, social, enterprise OIDC, SAML, passkey, or another standards-compliant identity system. Those details are outside the `memo_server` contract.

## Stable integration contract

`memo_server` integrates through OAuth/OIDC/JWT/JWKS semantics only.

It may depend on:

- `iss`;
- `aud`;
- `sub`;
- `iat` and `exp`;
- optional `nbf`;
- `jti` and `client_id` in strict access-token profile mode;
- JOSE metadata required for verification (`alg`, `kid`, `typ`);
- the configured JWKS endpoint.

It must not depend on:

- vendor SDKs;
- provider-specific token fields;
- provider-specific tenant/organization claim names;
- provider session APIs;
- provider database identifiers;
- upstream login challenge identifiers;
- internal authorization-server APIs;
- raw upstream error payloads.

Provider-specific custom claims must be safely ignored by memo ownership logic; their absence must not make an otherwise valid memo access token invalid.

## Generic configuration

Stable application configuration remains provider-neutral:

```text
AUTH_MODE
AUTH_ISSUER
AUTH_AUDIENCE
AUTH_JWKS_URI
AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS
AUTH_JWT_SIGNATURE_MODE
AUTH_JWT_TYPE_MODE
```

Conceptually:

```text
AUTH_ISSUER=https://<identity-public-origin>
AUTH_JWKS_URI=https://<identity-public-origin>/.well-known/jwks.json
AUTH_AUDIENCE=memo-api
```

Do not add stable `memo_server` configuration names containing a concrete identity product, internal authentication repository, or upstream provider name.

## Token contract

Production continues to use short-lived signed JWT access tokens verified locally by `memo_server`; normal memo requests do not require runtime token introspection.

The target strict profile remains RFC 9068-style access tokens with:

- access-token `typ` profile;
- `iss`;
- `aud`;
- `sub`;
- `iat`;
- `exp`;
- `client_id`;
- `jti`;
- JOSE `kid`.

Existing explicit signature-algorithm migration policy remains in force. The resource server must never infer accepted algorithms merely from the keys published in JWKS.

### Subject ownership

`sub` is the only identity key required by memo ownership.

`memo_server` must not infer ownership from email, username, display name, organization membership, external account IDs, or provider-specific claims.

The identity layer is responsible for mapping any upstream identity to a stable subject. The preferred long-term subject is an opaque UUID under the public issuer namespace.

### Audience isolation

Tokens accepted by `memo_server` must include the memo API audience. A token for another Schnee service is not accepted solely because it has the same issuer or subject.

## Public identity origin

Production should use one provider-neutral public identity origin, for example:

```text
https://id.<public-domain>
```

The public hostname must not encode a concrete provider, internal repository, or internal component name.

Standards-defined public metadata may include:

```text
/.well-known/openid-configuration
/.well-known/jwks.json
/authorize
/token
/userinfo
/logout
```

`memo_server` itself requires only the issuer/JWKS resource-server contract.

## Browser and BFF boundary

Production browser authentication should use:

- `Secure` + `HttpOnly` + appropriate `SameSite` cookies for long-lived browser session/refresh state;
- no access token in `localStorage`;
- no refresh credential in `localStorage`;
- short-lived access tokens resolved server-side;
- `App.Locals.accessToken` or equivalent server-controlled request context;
- the existing SvelteKit BFF as the only browser-to-`memo_server` forwarding path.

### Cookie naming

Public cookies use implementation-neutral names. They must not expose an internal product, repository, component, or upstream provider name.

A suitable form is:

```text
__Host-schnee_session
```

The exact cookie name is an implementation decision, but its security attributes and provider-neutral naming are architectural requirements.

### Error normalization

Provider-specific failures are translated at the public boundary into stable categories such as:

- `login_required`;
- `invalid_session`;
- `access_denied`;
- `temporarily_unavailable`;
- generic HTTP 401/403/502 behavior where applicable.

Never intentionally expose to browser/API clients:

- internal login challenge identifiers;
- internal component names;
- internal hostnames;
- upstream stack traces;
- raw upstream error payloads;
- private session identifiers that are not part of the public protocol.

Detailed diagnostics remain server-side.

## Transport metadata normalization

At the public identity/application boundary, strip or normalize implementation-revealing metadata when protocol semantics allow it, including:

- product/framework `Server` headers;
- `X-Powered-By`;
- implementation-specific `X-*` headers;
- internal `Via` chains;
- internal reverse-proxy hostnames;
- implementation-specific cookies not intended for public use.

This reduces incidental fingerprinting only. Security must remain correct even when fingerprinting succeeds.

## Logging, metrics, and tracing

Application observability uses neutral names such as:

```text
auth.jwt.signature_invalid
auth.jwt.audience_invalid
auth.jwks.refresh_failed
auth.subject.invalid
```

and metrics such as:

```text
auth_verification_total{result="success|failure",reason="..."}
auth_jwks_refresh_total{result="success|failure"}
auth_jwks_cache_age_seconds
```

Never log raw access tokens, refresh credentials, passwords, authorization codes, or session secrets.

Avoid high-cardinality/public telemetry labels containing subject IDs, token IDs, upstream provider names, or internal topology. Deeper implementation telemetry may exist only inside access-controlled operational systems.

## Documentation boundary

Current public `memo_server` documentation describes authentication using only implementation-neutral concepts:

- resource server;
- authorization server;
- OAuth/OIDC;
- JWT access token;
- JWKS;
- browser session/BFF behavior.

Current documentation must not require knowledge of the concrete upstream implementation.

Editing current files does not erase source-control history. Historical references, if any, are a separate operational/history-management concern and are not a security dependency of this design.

## Federation and account linking

The identity implementation may authenticate users through local credentials, passkeys/WebAuthn, social OAuth/OIDC, enterprise OIDC, enterprise SAML, or another standards-compliant identity broker.

Federation-specific account linking, tenant discovery, home-realm discovery, SAML handling, upstream refresh credentials, and external callbacks remain inside the identity layer.

`memo_server` never links external accounts.

If multiple upstream identities map to one identity account, the identity layer emits the same stable `sub`. Automatic account linking must not be performed merely because two providers report the same email address unless that policy has its own explicit security review.

## Failure behavior

### Identity service unavailable

Existing valid access tokens may continue to work while their keys remain valid and available under the bounded JWKS cache policy. New login, refresh, or session renewal may fail.

Identity unavailability must never degrade into authorization bypass.

### JWKS unavailable

Retain the existing bounded fresh-cache/stale-if-error policy. Unknown or unverifiable keys fail closed.

### Key rotation

Use overlapping key publication and explicit algorithm policy. Do not remove old verification keys until no still-valid token can require them.

### Provider replacement

A provider replacement is transparent to memo-domain code when these remain stable:

- issuer contract, or an explicitly planned issuer migration;
- audience;
- stable subject mapping;
- access-token profile;
- JWKS semantics;
- BFF-visible login/session contract.

An issuer URL change is a security-sensitive migration, not an invisible implementation swap.

## Local development

Local development stays independent of production identity infrastructure:

```text
AUTH_MODE=development
DEVELOPMENT_USER_ID=<UUID>
```

Requirements remain:

- development identity transport is disabled in production;
- production JWT mode rejects development identity headers;
- development mode rejects Bearer identity to avoid ambiguous dual identity;
- browser-controlled values cannot select arbitrary development users in production.

## Migration plan boundary

Implementation planning will split the work into auditable stages.

### Stage 1 — neutralize current application-facing documentation

Describe the stable authentication contract without naming or requiring a concrete identity implementation. Keep all security/protocol requirements explicit.

### Stage 2 — provider-coupling audit

Audit code, tests, configuration, frontend, container definitions, CI, docs, and sample environment files for:

- implementation/product names;
- provider-specific endpoints;
- provider-specific headers/cookies;
- provider-specific claims;
- provider SDK imports;
- internal identity hostnames;
- raw upstream error forwarding.

Classify each finding as public contract coupling, private deployment configuration, test-only naming, or legitimate standards terminology.

### Stage 3 — production browser session integration

Implement server-side login/session/refresh handling that resolves a short-lived access token into server-controlled frontend context without exposing long-lived credentials to browser JavaScript.

### Stage 4 — provider-neutral verification

Use a provider-neutral test authorization server/fixture for portable CI. Environment-level tests may separately validate the deployed identity platform without making it part of the application contract.

## Required tests

Architecture/regression coverage should verify:

- no provider SDK dependency is added to backend/frontend manifests;
- memo authorization does not read provider-specific claims;
- selected public docs/config/code paths remain free of concrete implementation identifiers;
- raw upstream authentication errors/headers/cookies are not forwarded to browser clients.

Backend coverage retains issuer, audience, subject UUID, lifetime, JOSE algorithm/type, JWKS rotation, duplicate `kid`, unknown `kid`, stale-if-error, and custom-claim-ignorance cases.

BFF/E2E coverage adds session cookie attributes, login/logout/session behavior, refresh/expiry behavior, error normalization, safe return targets, and verification that browser-provided identity headers cannot override server-controlled identity.

Forbidden-name architecture checks must be scoped to stable/current boundaries rather than the entire Git history or intentionally archival material.

## Security properties

This design provides:

- provider implementation replaceability;
- stable standards-based token validation;
- audience isolation;
- no normal-request dependency on an authentication database;
- reduced accidental provider fingerprinting through application contracts;
- continued fail-closed verification.

It does not claim that a determined external observer can never fingerprint implementation software through DNS, TLS, timing, historical source control, deployment metadata, or operational mistakes.

## Operational boundary

The identity platform and `memo_server` remain independently deployable and rollbackable.

Before a production identity change, verify:

1. discovery/JWKS behavior;
2. issuer and audience;
3. token claim profile;
4. signing algorithm compatibility;
5. key-rotation overlap;
6. BFF login/session integration;
7. rollback while existing tokens remain valid;
8. 401 reason and JWKS-refresh monitoring during rollout.

Application rollback must not require identity implementation rollback, and identity rollback must not require memo data migration.

## Non-goals

This design does not:

- select or publicly identify a concrete authentication product;
- define the identity platform's internal database schema;
- define federation/SAML internals;
- define SCIM provisioning;
- define token exchange/delegation;
- move memo authorization into identity-provider claims;
- require runtime token introspection;
- guarantee removal of historical implementation references from source-control history.

## Acceptance criteria

The written design is accepted when all are agreed:

1. `memo_server` depends only on the standards-based identity contract defined here.
2. Stable application configuration is provider-neutral.
3. Memo ownership depends only on stable `sub`, not provider-specific fields.
4. Browser session/refresh credentials remain server-controlled.
5. Current public errors, cookies, headers, configuration, and documentation do not intentionally reveal implementation internals.
6. CI can test authentication without depending on the production identity implementation.
7. Provider replacement does not require memo-domain or persistence changes.
8. Authentication security remains valid even when the underlying implementation is known.