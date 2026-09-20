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

The dedicated authentication service issues short-lived RS256 JWT access tokens. Each token must have:

| Field | Requirement |
| --- | --- |
| JWT header `alg` | `RS256` |
| JWT header `kid` | required; identifies a published JWKS key |
| `iss` | must equal `AUTH_ISSUER` |
| `aud` | must include `AUTH_AUDIENCE` |
| `sub` | UUID used as memo_server's user ID |
| `iat` | required |
| `exp` | required |
| `nbf` | optional; validated when present |

memo_server intentionally ignores unrelated custom claims. Authentication-provider-specific fields must not be required for memo ownership.

## Key distribution and rotation

memo_server reads public verification keys from `AUTH_JWKS_URI`.

- JWKS is cached for five minutes.
- An unknown `kid` can trigger a forced refresh.
- An invalid signature can trigger one forced JWKS refresh before the token is rejected.
- Refresh work is serialized so concurrent cache misses do not fan out into parallel JWKS requests.
- Forced refresh attempts are rate-limited per memo_server process by a short cooldown, including failed attempts, so attacker-controlled `kid` values or invalid signatures cannot cause one outbound JWKS request per API request.
- JWKS requests have a bounded timeout.
- Cached keys may continue to be used when a normal refresh temporarily fails, but only for a bounded stale-if-error window (currently one hour from the successful fetch).
- Private signing keys remain only in the authentication service.

This allows the authentication service and memo_server to be deployed and released independently.

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

The browser/session transport is a separate concern from memo_server's JWT verification boundary.

The production frontend integration should prefer:

- Secure + HttpOnly + SameSite cookies for long-lived browser session or refresh credentials
- short-lived access tokens
- no access/refresh tokens in localStorage
- CSRF protection where cookie-authenticated state-changing endpoints are used
- validated return/callback targets
- explicit logout and session invalidation

The frontend uses a SvelteKit server-side proxy as the memo API BFF boundary. Browser-supplied `Authorization`, `X-Development-User-Id`, and Cookie headers are not forwarded directly to memo_server. The proxy only adds authentication from server-controlled context:

- local development: private `DEVELOPMENT_USER_ID`, only when the SvelteKit server is running in development mode
- production: a short-lived access token resolved by the future server-side authentication/session hook and exposed as `App.Locals.accessToken`

The BFF also rejects unsafe memo mutations unless both conditions hold:

- the browser request carries the frontend-controlled `X-Schnee-Memo-Request: 1` marker
- the HTTP `Origin` exactly matches the SvelteKit request origin

The marker is stripped before forwarding to memo_server. Cross-site forms cannot add the custom header, while cross-origin JavaScript requires CORS preflight and still fails the exact-origin check. Production reverse proxies must configure SvelteKit's canonical request origin correctly so `event.url.origin` reflects the public application origin.

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
