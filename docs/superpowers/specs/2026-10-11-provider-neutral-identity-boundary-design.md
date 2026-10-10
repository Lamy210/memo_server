# Provider-Neutral Identity Boundary Design

**Date:** 2026-10-11  
**Status:** approved architecture design  
**Scope:** memo_server authentication integration boundary

## Goal

Define an authentication integration boundary that lets `memo_server` interoperate with an independently operated identity platform without coupling the application to any identity-provider implementation, vendor SDK, provider-specific claim, internal hostname, database, session API, or product naming.

The public and application-facing contract must remain stable even when the underlying identity implementation is replaced, federates to additional upstream identity providers, changes internal topology, or changes authentication products.

Success means:

- `memo_server` validates only standards-based access-token metadata and claims;
- the browser and public API never require provider-specific APIs or fields;
- provider implementation details do not appear in `memo_server` public interfaces, runtime errors, cookies, headers, logs intended for clients, documentation, or configuration names;
- replacing the upstream identity implementation does not require a memo-domain migration;
- local development remains possible without a production identity dependency;
- authentication remains secure even if the underlying implementation becomes publicly known.

This is an implementation-abstraction requirement, not a security-by-obscurity requirement. The security model must not depend on concealing which software or service provides authentication.

## Design principles

### 1. Standards are the integration contract

`memo_server` integrates through OAuth/OIDC/JWT/JWKS semantics only.

The resource server may depend on:

- issuer identity (`iss`);
- audience (`aud`);
- subject (`sub`);
- issued-at and expiry (`iat`, `exp`);
- optional not-before (`nbf`);
- token identifier (`jti`) when strict access-token profile mode is enabled;
- client identifier (`client_id`) when strict access-token profile mode is enabled;
- JOSE metadata required for signature verification (`alg`, `kid`, `typ`);
- the configured JWKS endpoint.

The resource server must not depend on:

- vendor SDKs;
- vendor-specific token fields;
- vendor-specific session endpoints;
- vendor-specific tenant or organization claim names;
- vendor database identifiers;
- upstream login challenge identifiers;
- internal authorization-server APIs;
- provider-specific error payloads.

### 2. Identity implementation is replaceable

The identity layer is treated as an independently deployable authorization server and identity system.

From `memo_server`, the only required deployment contract is conceptually:

```text
AUTH_ISSUER=https://<identity-public-origin>
AUTH_JWKS_URI=https://<identity-public-origin>/.well-known/jwks.json
AUTH_AUDIENCE=memo-api
```

The implementation behind that origin is outside the `memo_server` contract.

### 3. Public naming is implementation-neutral

Public and repository-visible names use generic terminology such as:

- `IdentityProvider`
- `AuthorizationServer`
- `IdentityConnection`
- `OIDCIssuer`
- `AccessTokenVerifier`
- `SessionProvider`

Names of a specific authentication product, component, internal repository, or internal deployment unit must not become part of `memo_server`'s public API or stable configuration contract.

### 4. No hidden trust extension

Abstracting the provider must not broaden trust.

`memo_server` continues to fail closed on:

- invalid or unexpected algorithms;
- invalid issuer or audience;
- invalid token lifetime;
- malformed JWS compact tokens;
- missing or invalid `kid`;
- unsupported JOSE critical headers;
- invalid JWKS metadata;
- duplicate or ambiguous keys;
- insecure or redirected JWKS transport;
- provider-specific claims being absent.

## Logical architecture

```text
Browser
  |
  v
SvelteKit frontend / BFF
  |
  +--> Public Identity Boundary
  |      - browser login/session flow
  |      - refresh/session lifecycle
  |      - standards-based authorization endpoints
  |
  +--> memo_server
         Authorization: Bearer <short-lived access token>
         |
         +--> validate issuer/audience/time/signature
         +--> resolve sub -> memo user UUID
         +--> enforce memo authorization
```

The identity system may itself federate to one or more upstream providers. That federation is not observable through the `memo_server` contract and must not require changes to memo authorization logic.

## Resource-server contract

### Access tokens

Production uses short-lived signed JWT access tokens.

`memo_server` remains responsible for local verification and must not require runtime introspection for every request.

The target production profile is RFC 9068-style JWT access tokens with:

- explicit access-token media type (`at+jwt` or equivalent accepted profile value);
- `iss`;
- `aud`;
- `sub`;
- `iat`;
- `exp`;
- `client_id`;
- `jti`;
- `kid` in the JOSE header.

The currently supported bounded signature-algorithm migration mechanism remains valid. Provider neutrality does not change the requirement for an explicit algorithm allowlist.

### Subject handling

`sub` is the only identity key required by memo ownership.

`memo_server` must not infer ownership from:

- email address;
- username;
- display name;
- tenant-specific external identifiers;
- organization membership claims;
- provider-specific account IDs.

The identity platform is responsible for mapping any upstream identity to the stable subject it issues to `memo_server`.

The preferred long-term rule is that a memo subject is an opaque UUID under the identity issuer's namespace.

### Audience isolation

Access tokens accepted by `memo_server` must include the memo API audience.

A token issued for another Schnee service must not be accepted solely because it has the same issuer or subject.

Audience isolation is part of the stable boundary and is not an implementation detail.

## Public identity origin

A single provider-neutral public identity origin should be used for production integration, for example:

```text
https://id.<public-domain>
```

Exact naming is deployment policy, but the hostname must not encode a specific provider or internal component name.

Public metadata may expose standards-defined endpoints such as:

```text
/.well-known/openid-configuration
/.well-known/jwks.json
/authorize
/token
/userinfo
/logout
```

`memo_server` does not require every endpoint above; its resource-server dependency is limited to issuer semantics and JWKS verification.

## Browser/BFF boundary

The browser must not receive or depend on identity implementation internals.

### Session transport

Preferred production browser model:

- long-lived browser session or refresh credential in `Secure`, `HttpOnly`, `SameSite` cookies;
- no access token in `localStorage`;
- no refresh token in `localStorage`;
- short-lived access token resolved server-side;
- SvelteKit stores the resulting short-lived access token only in server-controlled request context;
- the memo BFF attaches the Bearer token when forwarding to `memo_server`.

### Cookie naming

Cookies exposed at the public application boundary use application-neutral names.

They must not expose an internal product, component, repository, or upstream provider name.

Recommended form:

```text
__Host-schnee_session
```

The exact final cookie name is an implementation decision, but it must follow host-prefix security requirements where applicable and must remain provider-neutral.

### Error normalization

Provider-specific authentication errors must be translated at the public boundary.

Public clients receive stable categories such as:

- `login_required`;
- `invalid_session`;
- `access_denied`;
- `temporarily_unavailable`;
- generic HTTP 401/403/502 behavior as applicable.

The browser or API must not receive:

- internal login challenge identifiers;
- internal component names;
- upstream stack traces;
- internal hostnames;
- raw provider error payloads;
- internal session identifiers unless they are explicitly part of the public session protocol.

Detailed diagnostics remain server-side.

## HTTP metadata normalization

The public boundary should strip or normalize implementation-revealing transport metadata where doing so does not violate protocol semantics.

Examples include:

- framework/product `Server` headers;
- `X-Powered-By`;
- provider-specific `X-*` headers;
- internal `Via` chains exposed to public clients;
- internal reverse-proxy hostnames;
- provider-specific cookies not intended for public use.

This normalization is defense-in-depth for implementation abstraction. Authentication security must continue to hold even when a determined observer identifies the underlying implementation.

## Configuration naming

`memo_server` production configuration remains generic.

Preferred names:

```text
AUTH_MODE
AUTH_ISSUER
AUTH_AUDIENCE
AUTH_JWKS_URI
AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS
AUTH_JWT_SIGNATURE_MODE
AUTH_JWT_TYPE_MODE
```

Do not introduce stable `memo_server` configuration names containing:

- an authentication product name;
- an internal authentication repository name;
- an upstream social/enterprise provider name unless configuring an explicitly user-visible first-party feature outside the resource-server boundary.

## Logging and observability

### Application logs

`memo_server` may log authentication verification categories, but production logs should use neutral classifications such as:

```text
auth.jwt.signature_invalid
auth.jwt.audience_invalid
auth.jwks.refresh_failed
auth.subject.invalid
auth.session.unavailable
```

The application must not log raw access tokens, refresh credentials, passwords, authorization codes, or session secrets.

### Metrics

Metrics use generic names and labels.

Recommended examples:

```text
auth_verification_total{result="success|failure",reason="..."}
auth_jwks_refresh_total{result="success|failure"}
auth_jwks_cache_age_seconds
auth_unauthorized_total{reason="..."}
```

Avoid high-cardinality labels containing subject IDs, token IDs, issuer URLs, or upstream provider names unless metrics are explicitly isolated to a private operational system and justified.

### Tracing

Authentication spans should stop at the logical identity boundary in public/shared telemetry.

Internally trusted observability may contain deeper topology, but it must be access-controlled and must not be exported to browser-visible telemetry or public status APIs.

## Documentation boundary

Public `memo_server` documentation describes authentication in terms of:

- resource server;
- authorization server;
- OIDC/OAuth;
- JWT access tokens;
- JWKS;
- browser session/BFF behavior.

It must not require the reader to know the upstream implementation.

Historical repository data cannot be assumed to disappear merely because current files are edited. Therefore this design only guarantees that new and current public contracts remain provider-neutral; repository-history rewriting, if ever required, is a separate operational decision with its own risk and review.

## Federation compatibility

The identity implementation may authenticate users through:

- local credentials;
- passkeys/WebAuthn;
- social OAuth/OIDC providers;
- enterprise OIDC;
- enterprise SAML;
- another standards-compliant identity broker.

`memo_server` is unaffected as long as the public issuer continues to produce tokens satisfying the stable resource-server contract.

Federation-specific account linking, tenant discovery, home-realm discovery, SAML handling, upstream refresh credentials, and external-provider callbacks belong to the identity layer, not `memo_server`.

## Account-linking boundary

`memo_server` never links external accounts.

The identity layer may associate multiple upstream identities with one stable subject, but automatic linking must not rely solely on matching email addresses unless the identity layer has a separately reviewed, secure policy that proves the required ownership relationship.

From `memo_server`'s perspective, multiple upstream identities linked to the same account are indistinguishable because they resolve to the same stable `sub`.

## Service-to-service authentication

This design does not require service-to-service workloads to reuse browser sessions.

Future machine identities should use a dedicated standards-based grant or workload-identity mechanism and receive audience-scoped tokens.

The resource-server validation boundary should remain reusable for those tokens where their profile is intentionally compatible.

Token exchange or delegation is explicitly deferred until a concrete cross-service requirement exists.

## Local development

Local development remains independent of the production identity platform.

Current development behavior may continue using:

```text
AUTH_MODE=development
DEVELOPMENT_USER_ID=<UUID>
```

with the frontend BFF adding the development identity header only from private server configuration.

Requirements:

- development identity transport is disabled in production;
- production JWT mode rejects development identity headers;
- development mode rejects production Bearer identity to avoid ambiguous dual-identity requests;
- no browser-controlled value can select an arbitrary development user in production.

## Failure behavior

### Identity platform unavailable

Existing valid access tokens continue to be accepted while their signature keys remain valid and locally cached within the bounded JWKS policy.

New login, refresh, or session renewal may fail until the identity service recovers.

The memo API must not convert identity-platform unavailability into authorization bypass.

### JWKS unavailable

Use the existing bounded fresh-cache and stale-if-error policy.

Failure rules remain fail-closed for unknown or unverifiable keys.

### Key rotation

Key rotation uses overlapping JWKS publication and explicit algorithm policy.

The resource server must not derive accepted algorithms from whatever keys happen to appear in JWKS.

### Provider replacement

A provider replacement is successful when all of the following remain stable from the memo application's perspective:

- issuer contract or an explicitly planned issuer migration;
- audience;
- stable subject mapping;
- token profile;
- JWKS semantics;
- login/session behavior visible to the BFF.

If the issuer URL itself must change, treat that as a security-sensitive migration rather than a transparent implementation swap.

## Migration from current documentation and integration

Implementation should proceed in small, auditable stages.

### Stage 1: neutralize application-facing documentation

Update authentication documentation so the stable contract is described without naming or requiring a concrete provider implementation.

Keep protocol and security requirements explicit; only implementation-specific references are removed from the public contract.

### Stage 2: audit provider-specific coupling

Search application code, tests, configuration, frontend code, container definitions, CI, documentation, and sample environment files for:

- product/component names;
- provider-specific endpoint paths;
- provider-specific headers and cookies;
- provider-specific claims;
- provider SDK imports;
- internal identity hostnames;
- raw upstream error bodies.

Every finding is classified as:

1. public contract coupling — must be removed;
2. private deployment configuration — allowed only if not part of the application API and not committed as sensitive infrastructure detail;
3. test fixture naming — rename when it creates unnecessary coupling;
4. legitimate standards term — retain.

### Stage 3: browser session integration

Implement the server-side production session/login flow that populates `App.Locals.accessToken` without exposing refresh credentials to browser JavaScript.

### Stage 4: integration verification

Verify the application using a provider-neutral test authorization server or fixture so CI does not depend on a concrete production identity implementation.

A separate environment-level integration test may validate the actual deployed identity platform, but that test belongs outside the application's portable contract where practical.

## Testing requirements

### Architecture tests

Add regression checks that prevent obvious provider coupling from entering stable `memo_server` boundaries.

Possible checks:

- forbidden provider/product identifiers in selected public docs/config/code paths;
- no provider SDK dependencies in backend/frontend manifests;
- no provider-specific claim access in memo authorization;
- no raw authentication upstream payload forwarding through BFF routes.

The forbidden-name mechanism must be scoped carefully so historical migration documents or vendored standards material do not create brittle global tests.

### Backend tests

Cover:

- issuer validation;
- audience validation;
- subject UUID validation;
- access-token maximum lifetime;
- JOSE algorithm policy;
- strict access-token `typ` profile;
- `client_id` and `jti` requirements in strict mode;
- JWKS key rotation;
- duplicate `kid` rejection;
- unknown `kid` refresh behavior;
- bounded stale-if-error behavior;
- provider-specific custom claims being ignored.

### Frontend/BFF tests

Cover:

- browser authorization headers cannot override server identity;
- development identity remains server-only;
- session cookie security attributes;
- authentication errors are normalized;
- raw upstream headers are not forwarded;
- raw upstream cookies are not exposed unintentionally;
- logout invalidates the browser session;
- expired access tokens trigger the designed refresh/session path;
- authentication failure preserves the existing safe return-target rules.

### End-to-end tests

Use an implementation-neutral identity fixture capable of:

- login success;
- login failure;
- access-token issuance;
- token expiry;
- refresh/session rotation;
- logout;
- JWKS rotation;
- invalid audience;
- invalid issuer;
- invalid signature.

CI must not require knowledge of the production identity provider.

## Security properties

This architecture provides:

- provider implementation replaceability;
- reduced application attack surface from provider-specific SDKs;
- stable token validation semantics;
- audience isolation;
- no runtime authentication-database dependency for normal memo requests;
- minimized provider fingerprinting through the application contract;
- continued fail-closed verification.

It does **not** claim:

- that an advanced external observer can never fingerprint the underlying identity implementation;
- that DNS, TLS, timing, historical source control, deployment metadata, or operational mistakes can never reveal implementation details;
- that hiding implementation details is itself a security control sufficient to protect authentication.

## Operational boundary

The identity platform and `memo_server` should be independently deployable and independently rollbackable.

`memo_server` release safety depends only on the documented token/JWKS contract.

Before a production identity change:

1. verify discovery/JWKS endpoints;
2. verify issuer and audience values;
3. verify access-token claim profile;
4. verify signing algorithm compatibility;
5. verify key-rotation overlap;
6. verify BFF login/session integration;
7. verify rollback while existing tokens remain valid;
8. monitor 401 reasons and JWKS refresh failures during rollout.

## Rollback

Application rollback must not require reverting the identity implementation.

Identity rollback must not require memo data migration.

During a signing-key or algorithm migration, rollback follows the existing explicit dual-acceptance window. Do not remove old verification keys or algorithms until no valid token relying on them can remain.

## Non-goals

This design does not:

- select a specific authentication product;
- define the identity platform's internal database schema;
- define upstream IdP federation implementation;
- define enterprise SAML internals;
- define SCIM provisioning;
- define cross-service token exchange;
- move authorization policy out of `memo_server`;
- move memo ownership into identity-provider claims;
- require runtime token introspection;
- guarantee that source-control history contains no old implementation references.

## Acceptance criteria

The design is ready for implementation planning when all of the following are accepted:

1. `memo_server` depends only on the standards-based identity contract documented here.
2. Stable application configuration remains provider-neutral.
3. Memo ownership depends only on stable subject identity, not provider-specific fields.
4. Browser session and refresh credentials remain server-controlled.
5. Public errors, cookies, headers, and documentation do not intentionally reveal identity implementation internals.
6. CI can test authentication without depending on the production identity implementation.
7. Provider replacement can occur without memo-domain or persistence changes.
8. The security model remains valid even if the underlying identity implementation becomes known.
