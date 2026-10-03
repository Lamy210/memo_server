# Authentication architecture

memo_server is a resource server. It does not own browser login, password storage, refresh credentials, or long-lived user sessions.

The authentication boundary is split into two independently deployable services:

- **Dedicated authentication service**: owns registration/login, password/session/refresh handling, signing-key rotation, JWKS publication, and short-lived access-token issuance.
- **memo_server**: accepts a Bearer access token, validates its signature and claims, resolves `sub` to the memo principal UUID, and applies user-scoped authorization.

The authentication service is not Ory, AuthenticationPlatformReplace, or another shared/common authentication platform.

## Access-token contract

Production memo requests use a signed JWT access token.

Required claims:

- `sub`: opaque memo principal UUID
- `iss`: exact configured issuer
- `aud`: configured memo API audience
- `iat`: issued-at timestamp
- `exp`: expiration timestamp
- `nbf`: optional; enforced when present

The resource server also enforces a configured maximum token lifetime (`exp - iat`) and small bounded clock skew. Tokens whose lifetime exceeds the configured maximum are rejected even when the signature is valid.

The current compatibility signing algorithm is RS256. The resource server supports a staged bounded migration to ES384; enabled algorithms are explicit configuration and are never selected from token input alone. The target production signing policy is ES384 after rollout criteria are satisfied.

Each token must carry a `kid` protected header so memo_server can select a published JWKS verification key. The selected key must match the token algorithm/key family and any advertised `alg`, `use`, or `key_ops` constraints.

Access tokens use the JWS Compact Serialization. memo_server rejects malformed compact tokens before JOSE decoding: the token must contain exactly three non-empty base64url segments, the total compact token is capped at 16 KiB, and the protected-header segment is capped at 4 KiB. These are memo_server resource limits, not claims that the JWS standard defines those byte limits.

memo_server does not currently implement any JOSE critical-header extension. A protected header containing `crit` is therefore rejected before JWKS lookup, including an empty `crit` array. Non-critical custom JOSE headers remain ignorable; adding support for a critical extension requires an explicit verifier change that understands and enforces that extension's semantics.

Each token must have:

| Field | Requirement |
| --- | --- |
| JWT header `alg` | enabled server-side; current compatibility `RS256`, target `ES384` |
| JWT header `kid` | required; identifies one published JWKS verification key |
| JWT header `crit` | unsupported; any presence is rejected until the listed extension semantics are explicitly implemented |
| `iss` | must equal `AUTH_ISSUER` |
| `aud` | must contain `AUTH_AUDIENCE` |
| `sub` | valid UUID |
| `iat` | required; may not be unreasonably far in the future |
| `exp` | required; must be after current time |
| `nbf` | optional; must not be in the future beyond allowed skew |
| `exp - iat` | must be positive and within `AUTH_MAX_TOKEN_LIFETIME_SECONDS` |

Target RFC 9068 access-token profile additionally requires a non-empty string `client_id` and `jti`, plus explicit access-token typing (`typ=at+jwt` or `application/at+jwt`). `AUTH_JWT_TYPE_MODE=legacy-any` temporarily preserves the pre-profile compatibility path while the dedicated authentication service rollout is completed; `AUTH_JWT_TYPE_MODE=at-jwt` enables the strict profile.

## JWKS and key rotation

memo_server fetches verification keys only from the configured `AUTH_JWKS_URL`. Token-provided key URLs or embedded verification keys are not trusted as key sources.

The JWKS URL must be an unambiguous HTTPS URL with a host and no URL userinfo, query, or fragment. The configured issuer is subject to the same fail-closed HTTPS/url-shape policy. This validation happens at startup so deployment configuration fails before serving traffic.

JWKS fetches have a bounded request timeout. The verifier caches successful keysets and serializes refreshes within each process so concurrent unknown-`kid` requests do not amplify outbound JWKS traffic. Forced refresh attempts are rate-limited even when refresh fails.

Stale cached keys may be reused only within the configured stale-if-error window. Unknown `kid` and invalid-signature paths can trigger a bounded forced refresh to support signing-key rotation, but unauthenticated requests cannot cause an unbounded refresh loop.

The authentication service must publish overlapping old/new verification keys during rotation. `kid` values must be stable and unique for each verification key.

## Public authentication errors

memo_server intentionally keeps verifier diagnostics server-side. Public 401 responses use a stable `Unauthorized` error category and the message `Valid authentication credentials are required`; they do not reveal whether rejection was caused by token shape, algorithm policy, key selection, signature, issuer/audience, token time, or profile claims. Internal diagnostic reasons are available only at debug logging and must never include token contents.

## Browser integration

Browser code does not call memo_server directly and does not construct memo-server authentication headers.

The SvelteKit frontend owns the browser-facing BFF boundary under `/api/v1/...`. A future dedicated-auth-service integration will resolve the browser session on the server and expose only a short-lived access token to the BFF through `App.Locals.accessToken`.

The browser integration must provide:

- login/logout UI
- secure server-side session/refresh handling
- no refresh token in `localStorage`
- no long-lived access token in `localStorage`
- Secure + HttpOnly + SameSite cookies for browser session/refresh credentials
- CSRF protection for cookie-authenticated auth mutations
- validated return/callback targets
- explicit logout and session invalidation

Return targets are treated as untrusted input. The shared frontend resolver only accepts an internal absolute-path reference beginning with a single `/`, rejects scheme-relative URLs, backslashes, control characters, oversized values, and any parsed target whose origin escapes the application. Accepted values are canonicalized before use; invalid values fall back to `/memos`. The future login/callback implementation must use this resolver rather than redirecting directly to a query-string value.

The frontend uses a SvelteKit server-side proxy as the memo API BFF boundary. Browser-supplied `Authorization`, `X-Development-User-Id`, Cookie, compression negotiation, and proxy/client-address metadata are not forwarded directly to memo_server. In particular, `Forwarded`, every `X-Forwarded-*` header, `X-Real-IP`, `Via`, and common CDN/client-IP headers are stripped before the backend request is created. The trusted backend hop explicitly requests `Accept-Encoding: identity` so the server-side fetch layer cannot decode a compressed body while stale content-encoding metadata crosses back to the browser. If memo_server later needs client network metadata, it must come from a server-controlled derivation based on a trusted reverse-proxy boundary rather than browser-provided forwarding headers. The proxy only adds authentication from server-controlled context:

- local development: private `DEVELOPMENT_USER_ID`, only when the SvelteKit server is running in development mode
- production: a short-lived access token resolved by the future server-side authentication/session hook and exposed as `App.Locals.accessToken`

`BACKEND_URL` is trusted deployment configuration, but its shape is still fail-closed: the BFF accepts only an absolute HTTP or HTTPS origin with a host and no URL userinfo, path prefix, query, or fragment. The request path is constructed exclusively under `/api/v1/`; invalid backend targets return 502 without issuing a backend request.

The wildcard memo BFF also fail-closes both route and method surface instead of inheriting the entire backend `/api/v1` namespace. It forwards only `GET`/`HEAD`/`POST` for `memos`, `GET`/`HEAD` for `memos/search`, and `GET`/`HEAD`/`PATCH`/`DELETE` for `memos/:id`. Invalid/non-memo paths return 400. A valid memo route with an unsupported method returns 405 with the route-specific `Allow` header before CSRF or backend transport work. Exact `/api/v1/health` is frontend-local and does not expose memo_server health subroutes.

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

- Ory
- AuthenticationPlatformReplace
- a shared/common authentication service
- browser storage of refresh credentials
- memo_server password/session storage
