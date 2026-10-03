# Authentication architecture

memo_server is a resource server. Authentication is provided by a dedicated external authentication service; memo_server validates Bearer access tokens and applies per-user authorization to memo data.

The authentication service is intentionally separate from memo_server. Passwords, browser sessions, refresh credentials, account recovery, MFA/passkeys, signing private keys, and authentication audit events do not belong in the memo_server backend.

## Resource-server contract

The backend supports two explicit modes:

- `AUTH_MODE=development`: local development/CI only. A valid `X-Development-User-Id` request header supplies the memo user UUID.
- `AUTH_MODE=jwt`: production mode. Requests must provide `Authorization: Bearer <access-token>`.

The selected identity is carried through `AuthenticatedUser` and every user-scoped repository boundary. Health endpoints remain unauthenticated.

### JWT verification

Production access tokens are verified locally against configured JWKS keys. The verifier enforces:

- compact JWS with exactly three non-empty base64url segments
- configured signature algorithm policy
- required `kid`
- no unsupported JOSE critical header extensions
- exact configured issuer
- configured audience
- `exp`, `iat`, and optional `nbf`
- bounded maximum token lifetime (`exp - iat`)
- UUID-shaped `sub`
- JWKS key type/use/algorithm consistency when metadata is present

`AUTH_ISSUER` must be an unambiguous HTTPS issuer URL. The configured JWKS URL is also HTTPS-only and rejects URL userinfo, query strings, and fragments. memo_server never follows a signing-key URL supplied by the access token itself.

JWKS refresh is bounded, serialized per process, and rate-limited. A bounded stale-if-error window may reuse a previously validated key set during a transient JWKS outage, but an unknown key remains unauthorized when no acceptable cached key exists.

### Access-token profile migration

`AUTH_JWT_TYPE_MODE` controls the staged RFC 9068 profile migration:

- `legacy-any`: preserves the legacy token contract. Unknown/new profile claims do not affect acceptance.
- `at-jwt`: requires explicit access-token typing (`typ=at+jwt` or `application/at+jwt`) and non-empty string `client_id` / `jti` claims.

Strict mode remains fail-closed. The compatibility mode exists only for staged issuer migration and should be removed once the dedicated authentication service consistently emits the target profile.

## Public authentication errors

Authentication decisions remain detailed internally, but public HTTP 401 responses do not expose verifier internals such as token shape, signing algorithm policy, key lookup outcomes, or development-auth expectations.

The stable public contract is:

- status: `401 Unauthorized`
- JSON error: `Unauthorized`
- JSON message: `Valid authentication credentials are required`

Clients should branch on the HTTP status rather than provider/verifier diagnostic text. Internal reasons remain available to server-side debug logging and must never include token contents.

Similarly, generic dependency/cryptographic failures exposed as HTTP 503 use a stable public message rather than reflecting internal service/storage diagnostics into the response body.

## Browser integration boundary

The browser does not construct memo_server authentication directly. Browser memo API traffic goes through the same-origin SvelteKit `/api/v1/...` BFF.

The future production session integration must keep long-lived credentials server-side/browser-cookie scoped:

- Secure + HttpOnly + SameSite cookies for long-lived browser session or refresh credentials
- short-lived access tokens
- no access/refresh tokens in localStorage
- CSRF protection where cookie-authenticated state-changing endpoints are used
- validated return/callback targets
- explicit logout and session invalidation

Return targets are treated as untrusted input. The shared frontend resolver only accepts an internal absolute-path reference beginning with a single `/`, rejects scheme-relative URLs, backslashes, control characters, oversized values, and any parsed target whose origin escapes the application. Accepted values are canonicalized before use; invalid values fall back to `/memos`. The future login/callback implementation must use this resolver rather than redirecting directly to a query-string value.

The frontend uses a SvelteKit server-side proxy as the memo API BFF boundary. Browser-supplied `Authorization`, `X-Development-User-Id`, Cookie, compression negotiation, proxy/client-address metadata, and browser provenance metadata are not forwarded directly to memo_server. In particular, `Forwarded`, every `X-Forwarded-*` header, `X-Real-IP`, `Via`, common CDN/client-IP headers, `Origin`, `Referer`, and every `Sec-Fetch-*` header are stripped before the backend request is created. The BFF consumes `Origin` first when enforcing its same-origin mutation rule, then removes it together with the remaining browser provenance fields so future memo_server middleware cannot mistake browser metadata for properties of the trusted server-to-server hop. The trusted backend hop explicitly requests `Accept-Encoding: identity` so the server-side fetch layer cannot decode a compressed body while stale content-encoding metadata crosses back to the browser. If memo_server later needs client network or browser provenance metadata, it must come from a server-controlled derivation based on a reviewed trust boundary rather than copied browser headers. The proxy only adds authentication from server-controlled context:

- local development: private `DEVELOPMENT_USER_ID`, only when the SvelteKit server is running in development mode
- production: a short-lived access token resolved by the future server-side authentication/session hook and exposed as `App.Locals.accessToken`

`BACKEND_URL` is trusted deployment configuration, but its shape is still fail-closed: the BFF accepts only an absolute HTTP or HTTPS origin with a host and no URL userinfo, path prefix, query, or fragment. The request path is constructed exclusively under `/api/v1/`; invalid backend targets return 502 without issuing a backend request.

The frontend also emits a browser hardening baseline from SvelteKit itself. The production response includes a Content Security Policy that limits scripts, connections, forms, framing, objects, fonts, images, manifests, and workers to expected same-origin sources. Inline style attributes remain temporarily allowed because the current SvelteKit app template uses `style="display: contents"`; scripts do not receive `unsafe-inline`. A server hook also enforces `nosniff`, frame denial, strict referrer behavior, restrictive permissions policy, COOP/CORP, and cross-domain policy headers.

The BFF also rejects unsafe memo mutations unless both conditions hold:

- the browser request carries the frontend-controlled `X-Schnee-Memo-Request: 1` marker
- the HTTP `Origin` exactly matches the SvelteKit request origin

The marker is stripped before forwarding to memo_server. The validated browser `Origin` is also stripped after this trust decision, together with `Referer` and `Sec-Fetch-*` metadata. Cross-site forms cannot add the custom header, while cross-origin JavaScript requires CORS preflight and still fails the exact-origin check. Production reverse proxies must configure SvelteKit's canonical request origin correctly so `event.url.origin` reflects the public application origin.

The BFF also enforces the memo write-body budget before proxying: declared oversized bodies can be rejected immediately, and the actual request stream is counted so chunked or inaccurate length metadata cannot bypass the 512 KiB limit. Oversized bodies return 413 without reaching memo_server; memo_server independently keeps the same limit as a second boundary. Browser-facing memo API responses always set `Cache-Control: no-store`, overriding any backend cache directive so authenticated/private memo payloads are not intentionally stored by browser or intermediary HTTP caches.

The login/session/refresh implementation that populates this production server context remains part of the dedicated authentication-service integration tracked by #10 and #13.

## Local development

Local Compose uses:

```text
Backend:
  AUTH_MODE=development

Frontend server:
  DEVELOPMENT_USER_ID=<UUID>
```

The browser cannot override the development identity through `Authorization`, `X-Development-User-Id`, Cookie, forwarding headers, or mutation metadata because the BFF strips browser-controlled copies before it creates the backend request.

Production deployments must not enable `AUTH_MODE=development`.
