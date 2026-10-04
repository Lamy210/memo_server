# Authentication architecture

## Goal

memo_server uses an authentication service that is operated independently from the memo application.

The authentication service is not a shared/common authentication platform and does not require Ory components. memo_server must not depend on provider-specific claims, SDKs, databases, or session APIs.

## Service boundary

### Dedicated authentication service

The authentication service owns:

- user registration and login
- password hashing and credential policy
- account recovery
- optional MFA/passkeys
- browser/session management
- refresh-token or session rotation
- access-token issuance
- signing-key rotation and JWKS publication
- authentication audit events

A minimal HTTP contract may expose endpoints such as:

- `POST /v1/auth/register`
- `POST /v1/auth/login`
- `POST /v1/auth/refresh`
- `POST /v1/auth/logout`
- `GET /.well-known/jwks.json`

The exact login/session API is intentionally outside memo_server.

### memo_server

memo_server owns:

- validating Bearer access tokens
- checking signature, algorithm, issuer, audience, expiry and issued-at
- checking `nbf` when present
- resolving JWT `sub` to the memo user UUID
- enforcing the user boundary on reads, writes, cache keys and search
- returning 401 for invalid or missing authentication

memo_server does not store passwords or refresh tokens and does not call an authentication database for each request.

## Access-token contract

Production uses `AUTH_MODE=jwt`.

The HTTP authentication boundary rejects ambiguous credential transport before mode-specific verification: `Authorization` and `X-Development-User-Id` may each appear at most once, and a present value must be representable as a valid HTTP header string. Repeated credential headers or invalid header bytes return 401 rather than relying on first-value ordering or treating malformed input as absent. Credential types are also mode-exclusive: `AUTH_MODE=jwt` rejects any `X-Development-User-Id`, while `AUTH_MODE=development` rejects any Bearer `Authorization` value. A request cannot carry a second identity mechanism and rely on the selected mode to silently ignore it.

The dedicated authentication service issues short-lived JWT access tokens. memo_server independently enforces that property: production JWT mode requires `AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS`, accepted only in the repository policy range 60..=3600 seconds, and rejects tokens whose `exp - iat` is non-positive or exceeds that configured maximum. This makes issuer lifetime mistakes fail closed at the resource server. memo_server also uses an explicit signature-policy setting so the RS256 -> ES384 migration does not widen accepted algorithms implicitly.

`AUTH_JWT_SIGNATURE_MODE` supports exactly:

| Value | Accepted JWT `alg` | Intended use |
| --- | --- | --- |
| omitted / `rs256` | `RS256` only | existing production default |
| `rs256-es384` | `RS256` and `ES384` only | bounded issuer migration window |
| `es384` | `ES384` only | target AUTH-1 resource-server policy |

`AUTH_JWT_TYPE_MODE` is a separate bounded migration control:

| Value | Accepted JWT `typ` | Intended use |
| --- | --- | --- |
| omitted / `legacy-any` | compatibility mode; `typ` is not a validation signal | current issuer compatibility |
| `at-jwt` | `at+jwt` or `application/at+jwt`, ASCII case-insensitive | RFC 9068 access-token profile |

The target is `at-jwt` after the dedicated authentication service issues RFC 9068-profile access tokens. In strict mode, missing `typ`, generic `JWT`, ID-token types, and unrelated JWT media types are rejected before JWKS lookup. After signature/issuer/audience validation, the resource server also requires the RFC 9068 mandatory `client_id` and `jti` claims to be present as non-empty strings; `iat`, `iss`, `aud`, `exp`, and `sub` were already mandatory. This keeps `legacy-any` compatible with the current issuer while making `at-jwt` a materially stricter profile rather than a header-only switch.

`AUTH_ISSUER` is also validated as deployment configuration rather than accepted as an arbitrary string. It must be an absolute HTTPS URL with a host and no userinfo, query, or fragment. Path components remain allowed for multi-tenant authorization-server issuer identifiers. The configured string itself is retained and compared exactly with the JWT `iss` claim; memo_server does not normalize issuer identity before comparison.

Access tokens use the JWS Compact Serialization. memo_server rejects malformed compact tokens before JOSE decoding: the token must contain exactly three non-empty base64url segments, the total compact token is capped at 16 KiB, and the protected-header segment is capped at 4 KiB. These are memo_server resource limits, not claims that the JWS standard defines those byte limits.

memo_server does not currently implement any JOSE critical-header extension. A protected header containing `crit` is therefore rejected before JWKS lookup, including an empty `crit` array. Non-critical custom JOSE headers remain ignorable; adding support for a critical extension requires an explicit verifier change that understands and enforces that extension's semantics.

Each token must have:

| Field | Requirement |
| --- | --- |
| JWT header `alg` | allowed by `AUTH_JWT_SIGNATURE_MODE`; no other algorithm is accepted |
| JWT header `typ` | when `AUTH_JWT_TYPE_MODE=at-jwt`, must be `at+jwt` or `application/at+jwt` |
| JWT header `kid` | required; identifies one published JWKS verification key |
| JWT header `crit` | unsupported; any presence is rejected until the listed extension semantics are explicitly implemented |
| `iss` | must equal `AUTH_ISSUER` |
| `aud` | must include `AUTH_AUDIENCE` |
| `sub` | UUID used as memo_server's user ID |
| `iat` | required; must not be more than the existing clock-skew allowance in the future |
| `exp` | required; must be after `iat` and `exp - iat` must not exceed `AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS` |
| `client_id` | when `AUTH_JWT_TYPE_MODE=at-jwt`, required as a non-empty string |
| `jti` | when `AUTH_JWT_TYPE_MODE=at-jwt`, required as a non-empty string |
| `nbf` | optional; validated when present |

JWKS metadata is also fail-closed. When `alg`, `use`, or `key_ops` are published they must agree with verification. RS256 requires an RSA JWK; ES384 requires an EC JWK on P-384. Token `kid` values must be non-empty and at most 128 bytes. Published JWKs that carry a `kid` must use a unique non-empty value; duplicate `kid` values invalidate the JWKS rather than relying on document order. A `kid` must not be reused to change one active key from RSA to EC during migration.

memo_server intentionally ignores unrelated custom claims. Authentication-provider-specific fields must not be required for memo ownership.

## Key distribution and rotation

memo_server reads public verification keys from `AUTH_JWKS_URI`.

`AUTH_JWKS_URI` is treated as trusted deployment configuration but still has an explicit transport boundary: it must be an absolute HTTPS URL, must not contain URL userinfo/password data, and must not contain a fragment. The dedicated JWKS HTTP client is HTTPS-only and does not follow redirects. A 3xx response therefore fails closed instead of allowing the configured authentication service to redirect memo_server to a different origin or scheme.

- JWKS fresh-cache reuse is capped at five minutes **and** at `AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS`, whichever is shorter.
- An unknown `kid` can trigger a forced refresh.
- An invalid signature can trigger one forced JWKS refresh before the token is rejected.
- Refresh work is serialized so concurrent cache misses do not fan out into parallel JWKS requests.
- Forced refresh attempts are rate-limited per memo_server process by a short cooldown, including failed attempts, so attacker-controlled `kid` values or invalid signatures cannot cause one outbound JWKS request per API request.
- JWKS requests have a bounded timeout.
- JWKS response bodies are streamed with a hard 256 KiB limit and accepted sets are limited to 64 keys.
- Empty JWKS documents, oversized key sets, empty/oversized published `kid` values, and duplicate published `kid` values are rejected before caching.
- Cached keys may continue to be used when a normal refresh temporarily fails, but only for a bounded stale-if-error window: at most one hour from the successful fetch and never longer than `AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS + 30s`, whichever is shorter. For short-lived tokens this leaves only the existing clock-skew margin after fresh-cache expiry, instead of inheriting the full one-hour stale-key window.
- Private signing keys remain only in the authentication service.

This allows the authentication service and memo_server to be deployed and released independently.

### RS256 -> ES384 migration

Use an overlap rather than an algorithm flag day:

1. deploy resource servers with ES384 support while leaving `AUTH_JWT_SIGNATURE_MODE=rs256`,
2. publish a new P-384 JWKS key under a new `kid`,
3. switch resource servers to `rs256-es384`,
4. start issuing ES384 access tokens from the authentication service,
5. wait at least the maximum RS256 access-token lifetime plus clock-skew/rollout margin while monitoring verification failures,
6. switch every memo_server replica to `AUTH_JWT_SIGNATURE_MODE=es384`,
7. retire the old RSA verification key from JWKS only after no valid RS256 tokens can remain.

Do not move an issuer to ES384 before every resource-server replica that may receive its tokens is in the explicit dual-accept phase. Rollback during the overlap is performed by making the issuer issue RS256 again while resource servers remain dual; after ES384-only cutover, re-enabling RS256 acceptance is a deliberate security-policy rollback and should be separately reviewed.

## Deployment

A typical production topology is:

```text
Browser
  |
  +--> Frontend / optional BFF
  |        |
  |        +--> Dedicated Auth Service
  |        |      - register/login/session/refresh
  |        |      - signing keys
  |        |      - JWKS
  |        |
  |        +--> memo_server
  |               Authorization: Bearer <short-lived access token>
  |
  +--> Auth Service login/session endpoints when required
```

The auth service may use a separate database, hostname, deployment pipeline and scaling policy.

## Browser integration

The browser/session transport is a separate concern from memo_server's JWT verification boundary. memo_server keeps authentication verifier diagnostics server-side: HTTP 401 responses expose a stable generic message rather than details such as token shape, algorithm policy, key lookup, or development-auth parsing failures. Clients must branch on the 401 status instead of matching diagnostic text.

The production frontend integration should prefer:

- Secure + HttpOnly + SameSite cookies for long-lived browser session or refresh credentials
- short-lived access tokens
- no access/refresh tokens in localStorage
- CSRF protection where cookie-authenticated state-changing endpoints are used
- validated return/callback targets
- explicit logout and session invalidation

Return targets are treated as untrusted input. The shared frontend resolver only accepts an internal absolute-path reference beginning with a single `/`, rejects scheme-relative URLs, backslashes, control characters, oversized values, and any parsed target whose origin escapes the application. Accepted values are canonicalized before use; invalid values fall back to `/memos`. The future login/callback implementation must use this resolver rather than redirecting directly to a query-string value.

The frontend uses a SvelteKit server-side proxy as the memo API BFF boundary. Browser-supplied `Authorization`, `X-Development-User-Id`, Cookie, compression negotiation, and proxy/client-address metadata are not forwarded directly to memo_server. In particular, `Forwarded`, every `X-Forwarded-*` header, `X-Real-IP`, `Via`, and common CDN/client-IP headers are stripped before the backend request is created. The trusted backend hop explicitly requests `Accept-Encoding: identity`. Because server-side Fetch can still decode a compressed response from a non-conforming upstream while retaining its `Content-Encoding` metadata, the BFF also strips backend `Content-Encoding` before constructing the browser-facing response. If memo_server later needs client network metadata, it must come from a server-controlled derivation based on a trusted reverse-proxy boundary rather than browser-provided forwarding headers. The proxy only adds authentication from server-controlled context:

- local development: private `DEVELOPMENT_USER_ID`, only when the SvelteKit server is running in development mode
- production: a short-lived access token resolved by the future server-side authentication/session hook and exposed as `App.Locals.accessToken`

`BACKEND_URL` is trusted deployment configuration, but its shape is still fail-closed: the BFF accepts only an absolute HTTP or HTTPS origin with a host and no URL userinfo, path prefix, query, or fragment. The request path is constructed exclusively under `/api/v1/`; invalid backend targets return 502 without issuing a backend request.

The frontend also emits a browser hardening baseline from SvelteKit itself. The production response includes a Content Security Policy that limits scripts, connections, forms, framing, objects, fonts, images, manifests, and workers to expected same-origin sources. Inline style attributes remain temporarily allowed because the current SvelteKit app template uses `style="display: contents"`; scripts do not receive `unsafe-inline`. A server hook also enforces `nosniff`, frame denial, strict referrer behavior, restrictive permissions policy, COOP/CORP, and cross-domain policy headers.

The BFF also rejects unsafe memo mutations unless both conditions hold:

- the browser request carries the frontend-controlled `X-Schnee-Memo-Request: 1` marker
- the HTTP `Origin` exactly matches the SvelteKit request origin

The marker is stripped before forwarding to memo_server. Cross-site forms cannot add the custom header, while cross-origin JavaScript requires CORS preflight and still fails the exact-origin check. Production reverse proxies must configure SvelteKit's canonical request origin correctly so `event.url.origin` reflects the public application origin.

The BFF also enforces the memo write-body budget before proxying: declared oversized bodies can be rejected immediately, and the actual request stream is counted so chunked or inaccurate length metadata cannot bypass the 512 KiB limit. Oversized bodies return 413 without reaching memo_server; memo_server independently keeps the same limit as a second boundary. Browser-facing memo API responses always set `Cache-Control: no-store`, overriding any backend cache directive so authenticated/private memo payloads are not intentionally stored by browser or intermediary HTTP caches.

The login/session/refresh implementation that populates this production server context remains part of the dedicated authentication-service integration tracked by #10 and #13.

## Local development

Local Compose uses:

```text
Backend:
  AUTH_MODE=development

Frontend server:
  DEVELOPMENT_USER_ID=<UUID>

Forwarded to memo_server:
  X-Development-User-Id: <UUID>
```

The development identity is private server configuration, not a `VITE_*` browser variable. Client-supplied authentication headers are discarded by the frontend proxy. This mode is only for local development and CI. Production deployments must use `AUTH_MODE=jwt`.

## Non-goals

This design does not require:

- Ory Hydra
- Ory Kratos
- Ory Keto
- a shared authentication platform
- AuthenticationPlatformReplace
- provider-specific tenant claims
- runtime token introspection on every memo request
