# Identity access-token preflight

## Purpose

`verify_identity_access_token` validates one real issued access token against the same Rust JWT/JWKS verification path used by `memo_server` before production cutover.

The command is provider-neutral. It does not know or require the concrete identity implementation, a provider SDK, an identity database, a refresh token, or a browser session.

Unlike `scripts/validate_identity_rollout.py`, which checks issuer discovery and the public JWKS surface, this preflight exercises the resource server with an actual token. The two checks are complementary.

## Security boundary

Access tokens are credentials. The command therefore:

- accepts the token from standard input only;
- rejects every command-line argument so a token cannot be supplied through argv;
- reads at most the resource-server token limit plus one terminal CRLF;
- removes only one terminal LF or CRLF and does not otherwise trim or normalize token bytes;
- zeroizes the in-memory stdin buffer after validation, including token-shape failures;
- does not initialize application logging;
- never prints the raw token, decoded claims, subject UUID, JWKS response body, or provider-specific diagnostics;
- maps failures to stable operator-facing categories only.

Do not paste a production token into a pull request, issue, chat log, CI variable dump, shell trace, or command argument.

## Configuration

Run the tool with the same intended JWT resource-server policy as the target deployment:

```text
AUTH_MODE=jwt
AUTH_ISSUER=https://id.example.invalid
AUTH_AUDIENCE=memo-api
AUTH_JWKS_URI=https://id.example.invalid/.well-known/jwks.json
AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS=300
```

The normal optional policy controls also apply:

```text
AUTH_JWT_SIGNATURE_MODE=rs256        # rs256 | rs256-es384 | es384
AUTH_JWT_TYPE_MODE=legacy-any        # legacy-any | at-jwt
```

Production acceptance should use the values intended for production, not looser values chosen only to make the preflight pass.

`AUTH_MODE=development` is rejected by this tool.

## Usage

From `backend/`, obtain an access token through an approved operator/staging path and pipe it to the binary without placing it in argv. For an interactive shell:

```bash
read -rs ACCESS_TOKEN
printf '%s\n' "$ACCESS_TOKEN" | cargo run --locked --quiet --bin verify_identity_access_token
status=$?
unset ACCESS_TOKEN
exit "$status"
```

Disable shell tracing before handling a token. Prefer direct piping from the system that obtains the token when that can be done without logging the credential.

A successful run exits with status `0` and prints only:

```text
access_token.valid=true
```

A failure exits non-zero and prints a stable category to stderr:

```text
access_token.valid=false
access_token.error=verification
```

Possible categories are:

| Error | Meaning |
| --- | --- |
| `usage` | command-line arguments were supplied |
| `input` | stdin was empty, oversized, non-UTF-8, or contained additional line breaks |
| `configuration` | the deployment configuration is invalid, non-JWT, or cannot initialize the verifier |
| `verification` | the real resource-server JWT/JWKS verification path rejected the token |

The generic categories are intentional. Use server-side deployment telemetry for deeper diagnosis rather than making this credential-handling command echo token or verifier internals.

## What is validated

Because the command calls the existing `AuthService` JWT verifier, a successful result exercises the same resource-server rules currently used by application requests, including:

- JWS compact shape and bounded token/header sizes;
- configured signature algorithm policy (`RS256` / migration mode / `ES384`);
- access-token `typ` policy when strict `at-jwt` mode is enabled;
- required bounded `kid` and public JWKS key selection;
- HTTPS JWKS retrieval, bounded response handling, cache/refresh behavior, and signature verification;
- exact configured issuer;
- configured audience inclusion;
- required `exp`, `iat`, `iss`, `aud`, and `sub` claims;
- optional `nbf` validation;
- UUID subject parsing;
- future-`iat` clock-skew policy;
- positive `exp - iat` and configured maximum token lifetime;
- non-empty `client_id` and `jti` in strict `at-jwt` mode;
- rejection of unsupported JOSE critical headers.

Provider-specific custom claims are not part of memo ownership and are not required.

## What this does not prove

A successful token preflight does **not** prove that:

- the browser client registration has correct callback/logout origins;
- Authorization Code + PKCE login works end-to-end;
- refresh/session rotation is correct;
- every replica has the intended deployment configuration;
- a signing-key rotation survives the entire overlap window;
- rollback has been exercised while old tokens remain valid;
- authentication/JWKS monitoring and alerts are operational.

It also does not obtain or persist an access token for the operator.

## Recommended rollout order

1. Run `scripts/validate_identity_rollout.py` against the candidate issuer/JWKS surface.
2. Issue a real staging access token using the intended production token profile.
3. Run `verify_identity_access_token` with the intended issuer, audience, signature mode, token-type mode, and lifetime policy.
4. Exercise browser login/callback/logout and refresh behavior in staging.
5. Exercise signing-key overlap and rollback while previously issued tokens remain valid.
6. Confirm authentication/JWKS failure monitoring.
7. Only then switch production traffic/configuration.

A preflight result is rollout evidence, not a substitute for the remaining acceptance checks in issue #13.
