# Authentication architecture

## Goal

`memo_server` is a resource server. It authenticates requests through a provider-neutral OAuth/OIDC boundary and must remain independent from the concrete identity implementation behind that boundary.

The application contract is intentionally limited to standards-based token and key metadata. `memo_server` must not depend on provider-specific claims, SDKs, databases, session APIs, internal hostnames, or product naming.

## Service boundary

### Authorization / identity boundary

The identity layer owns browser login and session concerns, including:

- user registration and login;
- credential policy and account recovery;
- optional MFA/passkeys;
- browser/session management;
- refresh/session rotation;
- access-token issuance;
- signing-key rotation and JWKS publication;
- authentication audit events;
- optional federation to other standards-compliant identity providers.

The implementation behind this boundary is outside the `memo_server` contract and may change without a memo-domain migration.

### memo_server

`memo_server` owns:

- validating Bearer access tokens;
- checking signature, algorithm, issuer, audience, expiry, and issued-at;
- checking `nbf` when present;
- resolving JWT `sub` to the memo user UUID;
- enforcing the user boundary on reads, writes, cache keys, and search;
- returning 401 for invalid or missing authentication.

`memo_server` does not store passwords or refresh credentials and does not call an authentication database for every request.

## Access-token contract

Production uses `AUTH_MODE=jwt`.

The HTTP authentication boundary rejects ambiguous credential transport before mode-specific verification: `Authorization` and `X-Development-User-Id` may each appear at most once, and a present value must be representable as a valid HTTP header string. Repeated credential headers or invalid header bytes return 401 rather than relying on first-value ordering or treating malformed input as absent. Credential types are mode-exclusive: `AUTH_MODE=jwt` rejects any `X-Development-User-Id`, while `AUTH_MODE=development` rejects any Bearer `Authorization` value.

The authorization server issues short-lived JWT access tokens. `memo_server` independently enforces that property: production JWT mode requires `AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS`, accepted only in the repository policy range 60..=3600 seconds, and rejects tokens whose `exp - iat` is non-positive or exceeds that configured maximum.

`AUTH_JWT_SIGNATURE_MODE` supports exactly:

| Value | Accepted JWT `alg` | Intended use |
| --- | --- | --- |
| omitted / `rs256` | `RS256` only | existing production default |
| `rs256-es384` | `RS256` and `ES384` only | bounded issuer migration window |
| `es384` | `ES384` only | target resource-server policy |

`AUTH_JWT_TYPE_MODE` is a separate bounded migration control:

| Value | Accepted JWT `typ` | Intended use |
| --- | --- | --- |
| omitted / `legacy-any` | compatibility mode; `typ` is not a validation signal | current issuer compatibility |
| `at-jwt` | `at+jwt` or `application/at+jwt`, ASCII case-insensitive | RFC 9068 access-token profile |

The target is `at-jwt` after the configured issuer produces RFC 9068-profile access tokens. In strict mode, missing `typ`, generic `JWT`, ID-token types, and unrelated JWT media types are rejected before JWKS lookup. After signature/issuer/audience validation, the resource server also requires `client_id` and `jti` to be non-empty strings.

`AUTH_ISSUER` is deployment configuration and must be an absolute HTTPS URL with a host and no userinfo, query, or fragment. Path components remain allowed for authorization-server issuer identifiers. The configured string is retained and compared exactly with the JWT `iss` claim; issuer identity is not normalized before comparison.

Access tokens use JWS Compact Serialization. `memo_server` rejects malformed compact tokens before JOSE decoding: exactly three non-empty base64url segments are required, the total compact token is capped at 16 KiB, and the protected-header segment is capped at 4 KiB. These are `memo_server` resource limits rather than JWS standard limits.

`memo_server` currently implements no JOSE critical-header extension. Any protected header containing `crit` is rejected before JWKS lookup, including an empty `crit` array.

Each token must satisfy:

| Field | Requirement |
| --- | --- |
| JWT header `alg` | allowed by `AUTH_JWT_SIGNATURE_MODE`; no other algorithm is accepted |
| JWT header `typ` | when `AUTH_JWT_TYPE_MODE=at-jwt`, must be `at+jwt` or `application/at+jwt` |
| JWT header `kid` | required; identifies one published JWKS verification key |
| JWT header `crit` | unsupported; any presence is rejected |
| `iss` | must equal `AUTH_ISSUER` |
| `aud` | must include `AUTH_AUDIENCE` |
| `sub` | UUID used as `memo_server`'s user ID |
| `iat` | required; must not exceed the allowed future clock skew |
| `exp` | required; after `iat`, and lifetime must not exceed `AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS` |
| `client_id` | in `at-jwt` mode, required as a non-empty string |
| `jti` | in `at-jwt` mode, required as a non-empty string |
| `nbf` | optional; validated when present |

Provider-specific custom claims are ignored by memo ownership. Their absence must not invalidate an otherwise valid memo access token.

## Subject and audience isolation

`sub` is the only identity key required by memo ownership. `memo_server` must not infer ownership from email, username, display name, organization membership, external account identifiers, or provider-specific fields.

The identity layer is responsible for mapping any upstream login identity to the stable subject issued to `memo_server`.

`AUTH_AUDIENCE` isolates the memo API from other services. A token issued for another service is not accepted solely because it has the same issuer or subject.

## Key distribution and rotation

`memo_server` reads public verification keys from `AUTH_JWKS_URI`.

`AUTH_JWKS_URI` must be an absolute HTTPS URL without userinfo or a fragment. The dedicated JWKS client is HTTPS-only and does not follow redirects, so a 3xx response fails closed rather than moving verification to another origin or scheme.

JWKS behavior is bounded:

- fresh-cache reuse is capped at five minutes and `AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS`, whichever is shorter;
- an unknown `kid` may trigger a forced refresh;
- an invalid signature may trigger one forced refresh before rejection;
- refresh work is serialized to avoid concurrent fan-out;
- forced refresh attempts are rate-limited per process, including failed attempts;
- requests have a bounded timeout;
- response bodies are streamed with a 256 KiB hard limit;
- accepted JWKS sets are limited to 64 keys;
- empty sets, duplicate `kid`, empty/oversized `kid`, or metadata inconsistent with verification are rejected;
- cached keys may be used during a temporary refresh failure only inside the bounded stale-if-error window;
- private signing keys never belong in `memo_server`.

When `alg`, `use`, or `key_ops` are present in JWK metadata, they must agree with verification. RS256 requires an RSA JWK; ES384 requires an EC JWK on P-384. A `kid` must not be reused to change one active key from RSA to EC during migration.

### RS256 -> ES384 migration

Use an overlap rather than an algorithm flag day:

1. deploy resource servers with ES384 support while leaving `AUTH_JWT_SIGNATURE_MODE=rs256`;
2. publish a new P-384 verification key under a new `kid`;
3. switch resource servers to `rs256-es384`;
4. start issuing ES384 access tokens;
5. wait at least the maximum RS256 token lifetime plus clock-skew/rollout margin while monitoring verification failures;
6. switch every replica to `AUTH_JWT_SIGNATURE_MODE=es384`;
7. retire the RSA verification key only after no valid RS256 token can remain.

Rollback during the overlap is performed by issuing RS256 again while resource servers remain dual. After ES384-only cutover, re-enabling RS256 acceptance is an explicit security-policy rollback.

## Public identity origin

Production browser integration should use a provider-neutral public identity origin such as:

```text
https://id.<public-domain>
```

The hostname, cookies, configuration names, application errors, and public documentation must not encode a concrete implementation, internal repository, or internal component name.

Standards-defined metadata may expose endpoints such as:

```text
/.well-known/openid-configuration
/.well-known/jwks.json
/authorize
/token
/userinfo
/logout
```

`memo_server` itself requires only the issuer/JWKS resource-server contract.

## Browser integration

The browser/session transport is separate from `memo_server` JWT verification. Verifier diagnostics remain server-side; HTTP 401 responses expose a stable generic message instead of details such as token shape, algorithm policy, or key lookup behavior.

Production browser integration uses the SvelteKit BFF and should provide:

- Authorization Code flow with PKCE S256;
- `Secure` + `HttpOnly` + appropriate `SameSite` cookies for long-lived session/refresh state;
- short-lived access tokens resolved server-side;
- no access or refresh credentials in `localStorage`;
- CSRF protection for cookie-authenticated state-changing endpoints;
- validated return/callback targets;
- explicit logout and session invalidation.

Return targets are untrusted input. The shared frontend resolver accepts only safe internal absolute-path references and falls back to `/memos` for invalid values.

The frontend uses a SvelteKit server-side proxy as the memo API BFF boundary. Browser-supplied `Authorization`, `X-Development-User-Id`, Cookie, compression negotiation, and proxy/client-address metadata are not forwarded directly to `memo_server`. `Forwarded`, every `X-Forwarded-*` header, `X-Real-IP`, `Via`, and common client-IP headers are stripped before the backend request is created. The trusted backend hop requests `Accept-Encoding: identity`; a backend response that still carries `Content-Encoding` fails closed through the generic 502 path.

The BFF only adds authentication from server-controlled context:

- local development: private `DEVELOPMENT_USER_ID`, only in development mode;
- production: a short-lived access token resolved by the server-side identity/session hook and exposed as `App.Locals.accessToken`.

Production browser configuration uses provider-neutral private environment names:

```text
IDENTITY_ISSUER
IDENTITY_CLIENT_ID
IDENTITY_CLIENT_SECRET
IDENTITY_SESSION_KEY
```

`BACKEND_URL` remains trusted deployment configuration. The BFF accepts only an absolute HTTP or HTTPS origin with a host and no URL userinfo, path prefix, query, or fragment.

The BFF rejects unsafe memo mutations unless the request carries the frontend-controlled `X-Schnee-Memo-Request: 1` marker and the HTTP `Origin` exactly matches the SvelteKit request origin. The marker is stripped before forwarding to `memo_server`.

Browser-facing memo API responses always set `Cache-Control: no-store`.

## Error and metadata normalization

Provider-specific failures are translated at the public boundary into stable categories such as:

- `login_required`;
- `invalid_session`;
- `access_denied`;
- `temporarily_unavailable`;
- generic 401/403/502 behavior where applicable.

Public responses must not intentionally expose raw upstream errors, internal hostnames, internal challenge identifiers, implementation-specific cookies, product/framework headers, or implementation-specific `X-*` metadata when protocol semantics allow normalization.

This reduces accidental fingerprinting only. Authentication security must remain correct even if the underlying implementation is identified.

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

The development identity is private server configuration, not a `VITE_*` browser variable. Client-supplied authentication headers are discarded by the frontend proxy. Production deployments must use `AUTH_MODE=jwt`.

## Non-goals

This design does not require `memo_server` to know:

- the concrete identity implementation;
- identity-provider database schemas;
- provider-specific tenant claims;
- federation/SAML internals;
- provider SDKs;
- runtime token introspection on every memo request.
