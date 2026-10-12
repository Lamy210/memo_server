# Production identity rollout preflight

## Purpose

`memo_server` should reject an incompatible production identity deployment before application traffic is switched to it.

`scripts/validate_identity_rollout.py` is a provider-neutral, read-only preflight for the public issuer/JWKS surface used by the memo application. It uses only the Python standard library and does not require a provider SDK or access to an identity database.

The command performs network reads only. It does not register clients, rotate keys, mutate configuration, request credentials, or issue tokens.

## Usage

Run the preflight from a trusted deployment or operator environment that uses the same DNS/TLS path expected by production:

```bash
python3 scripts/validate_identity_rollout.py \
  --issuer 'https://id.example.invalid' \
  --jwks-uri 'https://id.example.invalid/.well-known/jwks.json' \
  --signature-mode rs256
```

`--signature-mode` accepts the same rollout policy names used by the backend:

- `rs256`
- `rs256-es384`
- `es384`

The command exits with status `0` and emits a JSON summary when the public contract is compatible. Contract, transport, or decoding failures exit with status `1` and return a generic JSON error without dumping the remote response body.

If a standards-valid endpoint contains a query component, the query value is used for the request and exact contract comparison but is replaced with `<redacted>` in the success summary. The issuer itself remains query-free. Do not use endpoint query strings as a substitute for a proper secret-management boundary.

## Checks

The preflight currently verifies:

1. the configured issuer is an absolute HTTPS URL with a host and without credentials, query, fragment, internal/edge whitespace, control characters, backslashes, or a malformed port;
2. the configured JWKS URI is an absolute HTTPS URL with a host and without credentials or fragment; a standards-valid query component is allowed because the backend JWKS contract allows it;
3. discovery is fetched from `<issuer>/.well-known/openid-configuration`;
4. the discovery `issuer` exactly equals the configured issuer string;
5. the discovery `jwks_uri` exactly equals the configured JWKS URI;
6. authorization and token endpoints are present and are HTTPS URLs without credentials or fragment; query components are allowed to match the browser OIDC endpoint contract;
7. an optional end-session endpoint, when present, satisfies the same HTTPS endpoint rules;
8. discovery explicitly advertises PKCE `S256`, matching the SvelteKit browser authorization-code boundary;
9. the token endpoint supports `client_secret_basic`; when `token_endpoint_auth_methods_supported` is omitted, the standards default used by the browser boundary is applied;
10. discovery and JWKS requests use a positive finite timeout, do not follow redirects, and accept at most 256 KiB of JSON;
11. JWKS contains between 1 and 64 keys;
12. published `kid` values are non-empty, unique, and at most 128 bytes;
13. private or symmetric key material is not published in JWKS;
14. published `alg`, `use`, and `key_ops` metadata is structurally valid when present rather than being ignored because another usable key exists;
15. RSA modulus/exponent and EC coordinate fields used by supported verification keys are syntactically valid, non-empty, unpadded base64url values;
16. at least one public verification key is usable for the configured signature mode, including RSA/RS256 and EC P-384/ES384 shape checks plus `use` / `key_ops` constraints when published.

These limits intentionally mirror the resource-server and browser OIDC boundaries where practical so rollout validation fails before production requests encounter the same incompatible data.

## What this does not prove

A successful preflight is necessary but not sufficient for production cutover. It does **not** prove:

- a real access token has the expected `aud`, `sub`, `iat`, `exp`, `client_id`, `jti`, or `typ` profile;
- a browser client registration has the correct callback/logout origins;
- Authorization Code + PKCE login succeeds end-to-end;
- refresh/session rotation works;
- a signing-key rotation survives the full overlap window;
- rollback has been exercised;
- authentication telemetry and alerts are configured.

Those checks remain part of the production rollout described in issue #13 and, for browser behavior, issue #10.

## Recommended rollout order

1. Run this preflight against the candidate public issuer/JWKS endpoint.
2. Validate a real issued access token against a staging `memo_server` configured with the intended issuer, audience, signature mode, token-type mode, and maximum lifetime.
3. Exercise browser login/callback/logout and refresh behavior in staging.
4. Exercise signing-key overlap and rollback while previously issued tokens remain valid.
5. Confirm authentication/JWKS failure monitoring.
6. Only then switch production traffic/configuration.

Do not make production correctness depend on hiding the concrete implementation behind the provider-neutral public boundary.
