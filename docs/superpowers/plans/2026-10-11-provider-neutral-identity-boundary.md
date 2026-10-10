# Provider-Neutral Identity Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `memo_server` production authentication work through a provider-neutral OIDC boundary without exposing or coupling application code to the concrete upstream identity implementation.

**Architecture:** Keep Rust `memo_server` as a standards-only JWT resource server. Make the SvelteKit BFF a provider-neutral OIDC confidential client using Authorization Code + PKCE S256, sealed `Secure`/`HttpOnly` session state, and `App.Locals.accessToken`; the existing memo proxy remains the only browser-to-backend path. Add repository regression gates and a deterministic fake identity server so current/public application surfaces and CI do not depend on a concrete identity product.

**Tech Stack:** Rust/Actix resource server (existing), SvelteKit 2 / TypeScript / Node Web Crypto, Vitest, Playwright, Python 3 stdlib fixtures/checks, OAuth 2.0/OIDC/JWT/JWKS.

**Spec:** `docs/superpowers/specs/2026-10-11-provider-neutral-identity-boundary-design.md`

## Global Constraints

- `memo_server` depends only on the standards-based identity contract defined in the spec.
- Stable application configuration and public naming remain provider-neutral.
- Memo ownership depends only on stable `sub`; provider-specific custom claims are ignored.
- Browser JavaScript never receives access tokens, refresh credentials, authorization codes after callback processing, or session secrets.
- Production browser auth uses Authorization Code + PKCE S256; redirect targets use the existing safe-return-target resolver.
- Public session cookie name is `__Host-schnee_session`; it is `Secure`, `HttpOnly`, `Path=/`, and `SameSite=Lax` in production.
- No provider SDK dependency is added to backend/frontend manifests.
- Existing JWT issuer/audience/lifetime/JOSE/JWKS fail-closed behavior remains unchanged.
- Raw upstream errors, headers, cookies, internal hostnames, or implementation-specific identifiers are never forwarded to browser/API clients.
- Production identity failure never falls back to `DEVELOPMENT_USER_ID` or another identity mechanism.
- Historical Git data is out of scope; regression checks scan only selected current/stable boundaries.

## Review Focus

- Callback with mismatched/replayed `state`: reject the callback, consume/clear transaction state, and do not create a session.
- Discovery or token endpoint that redirects or resolves outside the configured HTTPS issuer contract: fail closed with a generic public error.
- Refresh returns a rotated refresh token: atomically replace the prior credential in the sealed session before the request proceeds.
- Sealed session payload exceeds the cookie budget: reject session creation/refresh rather than emitting a truncated or multi-part secret.
- Production request supplies browser-controlled `Authorization` or development identity headers: existing BFF stripping wins; only `locals.accessToken` reaches `memo_server`.

---

## File Structure

### Documentation / regression boundary

- Modify `docs/authentication.md` — remove concrete implementation names and describe only the stable standards contract.
- Modify `README.md` — keep public setup/auth wording provider-neutral and document frontend identity env names.
- Create `scripts/check_provider_neutral_auth.py` — scan selected current files/manifests/config surfaces for forbidden implementation identifiers and provider SDK dependencies.
- Create `scripts/test_check_provider_neutral_auth.py` — stdlib `unittest` coverage for scanner scope, exclusions, and detection.
- Modify `.github/workflows/ci.yml` — run the provider-neutral architecture check.

### OIDC client and BFF session

- Create `frontend/src/lib/server/identity/config.ts` — parse and validate generic identity configuration.
- Create `frontend/src/lib/server/identity/config.test.ts` — invalid issuer/client/session-key cases.
- Create `frontend/src/lib/server/identity/oidcClient.ts` — discovery, authorization URL, code exchange, refresh, logout URL construction, endpoint validation.
- Create `frontend/src/lib/server/identity/oidcClient.test.ts` — discovery/PKCE/error-normalization tests.
- Create `frontend/src/lib/server/identity/session.ts` — sealed session/transaction cookies using Node Web Crypto AES-256-GCM.
- Create `frontend/src/lib/server/identity/session.test.ts` — tamper, expiry, cookie attributes, rotation, size-limit tests.
- Modify `frontend/src/app.d.ts` — expose only server-controlled auth state in `App.Locals`/`PageData`.
- Modify `frontend/src/hooks.server.ts` — restore/refresh sealed session and populate `locals.accessToken`.
- Create `frontend/src/hooks.server.test.ts` — valid, expired, refresh-failure, and production fail-closed tests.

### Login/session routes and UI

- Create `frontend/src/routes/auth/login/+server.ts` — start OIDC Authorization Code + PKCE transaction.
- Create `frontend/src/routes/auth/callback/+server.ts` — validate state, exchange code, seal session, redirect safely.
- Create `frontend/src/routes/auth/logout/+server.ts` — clear local session before optional provider-neutral end-session redirect.
- Create `frontend/src/routes/auth/session/+server.ts` — return only `{ authenticated: boolean }` to same-origin clients.
- Create `frontend/src/routes/+layout.server.ts` — expose authenticated boolean to SSR UI, not tokens.
- Modify `frontend/src/routes/+layout.svelte` — provider-neutral login/logout navigation.
- Create route/unit tests adjacent to the auth server modules where practical.

### Portable CI fixture / E2E

- Create `scripts/identity-fixture-server.py` — deterministic OIDC discovery/authorize/token/logout fixture with no production-provider behavior or naming.
- Modify `scripts/ui-fixture-server.py` — optionally require the expected server-injected Bearer token on memo API requests.
- Create `frontend/tests/e2e/auth-session.spec.ts` — end-to-end login, memo request, refresh, logout, invalid callback, and error-normalization coverage.
- Modify `.github/workflows/ci.yml` — start both fixtures and run authenticated browser E2E with generic identity configuration.

---

### Task 1: Neutralize current public authentication surfaces and enforce the boundary

**Files:**
- Modify: `docs/authentication.md`
- Modify: `README.md`
- Create: `scripts/check_provider_neutral_auth.py`
- Create: `scripts/test_check_provider_neutral_auth.py`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: approved provider-neutral design spec.
- Produces: `check_paths(root: Path) -> list[Finding]` and a CI command `python3 scripts/check_provider_neutral_auth.py .` that exits non-zero on forbidden current-boundary coupling.

- [ ] **Step 1: Write failing scanner tests**

Add stdlib `unittest` cases proving the scanner:

```python
self.assertFinding("docs/authentication.md", "<forbidden-test-identifier>")
self.assertNoFinding("docs/superpowers/specs/archival-example.md", "<forbidden-test-identifier>")
self.assertProviderDependencyFinding("frontend/package.json", "@example/provider-sdk")
```

Tests must also prove generic terms such as `OIDC`, `OAuth`, `JWKS`, `authorization server`, and `identity provider` are allowed.

- [ ] **Step 2: Run the tests and verify RED**

Run: `python3 -m unittest scripts.test_check_provider_neutral_auth -v`

Expected: FAIL because the scanner does not exist.

- [ ] **Step 3: Implement the scanner**

Create:

```python
@dataclass(frozen=True)
class Finding:
    path: str
    reason: str


def check_paths(root: Path) -> list[Finding]: ...
```

Scope the check to current public docs, README, frontend/backend manifests, runtime source, Compose/sample env, and CI configuration. Explicitly exclude Git history and `docs/superpowers/specs/` / `docs/superpowers/plans/` because they are review artifacts and may discuss abstraction requirements.

Dependency checks inspect manifest dependency names, not arbitrary substrings in lockfile integrity data.

- [ ] **Step 4: Neutralize current docs**

Rewrite `docs/authentication.md` and relevant README auth paragraphs so they describe resource server, authorization server, OIDC/OAuth, JWT/JWKS, BFF session behavior, and generic environment variables only. Remove current references that identify a concrete auth product/repository.

- [ ] **Step 5: Run scanner tests and scanner**

Run:

```bash
python3 -m unittest scripts.test_check_provider_neutral_auth -v
python3 scripts/check_provider_neutral_auth.py .
```

Expected: PASS / exit 0.

- [ ] **Step 6: Add the scanner to CI**

Add a backend-independent CI step before expensive integration jobs so provider-coupling regressions fail early.

- [ ] **Step 7: Run affected validation**

Run the scanner plus existing formatting/lint/check commands for files touched by docs/CI edits.

- [ ] **Step 8: Commit**

```bash
git add docs/authentication.md README.md scripts/check_provider_neutral_auth.py scripts/test_check_provider_neutral_auth.py .github/workflows/ci.yml
git commit -m "docs: enforce provider-neutral auth boundary"
```

### Task 2: Add generic identity configuration and OIDC client primitives

**Files:**
- Create: `frontend/src/lib/server/identity/config.ts`
- Create: `frontend/src/lib/server/identity/config.test.ts`
- Create: `frontend/src/lib/server/identity/oidcClient.ts`
- Create: `frontend/src/lib/server/identity/oidcClient.test.ts`

**Interfaces:**
- Consumes: SvelteKit private env and standards OIDC discovery metadata.
- Produces:

```ts
export type IdentityConfig = {
  issuer: URL;
  clientId: string;
  clientSecret: string;
  sessionKey: Uint8Array;
};

export function readIdentityConfig(env: Record<string, string | undefined>): IdentityConfig;
export async function discoverIdentity(config: IdentityConfig, fetchFn: typeof fetch): Promise<OidcMetadata>;
export function createAuthorizationRequest(metadata: OidcMetadata, input: AuthorizationRequestInput): AuthorizationRequest;
export async function exchangeAuthorizationCode(...): Promise<TokenSet>;
export async function refreshAccessToken(...): Promise<TokenSet>;
```

Stable private env names:

```text
IDENTITY_ISSUER
IDENTITY_CLIENT_ID
IDENTITY_CLIENT_SECRET
IDENTITY_SESSION_KEY
```

`IDENTITY_SESSION_KEY` is base64 for exactly 32 random bytes. No provider/product name is permitted in these names.

- [ ] **Step 1: Write failing configuration tests**

Cover: HTTPS issuer required outside local test mode; no userinfo/query/fragment; non-empty client ID/secret; exactly 32 decoded session-key bytes; malformed base64 rejected.

- [ ] **Step 2: Run config tests and verify RED**

Run: `cd frontend && pnpm vitest run src/lib/server/identity/config.test.ts`

Expected: FAIL because module does not exist.

- [ ] **Step 3: Implement `readIdentityConfig`**

Keep validation provider-neutral. Do not add provider SDKs or discovery libraries.

- [ ] **Step 4: Write failing OIDC discovery/PKCE tests**

Assert:

- discovery URL is `${issuer}/.well-known/openid-configuration`;
- returned `issuer` exactly equals configured issuer;
- authorization/token endpoints are absolute HTTPS URLs;
- `code_challenge_methods_supported` contains `S256`;
- authorization request uses `response_type=code`, `scope=openid`, transaction-specific `state`, `nonce`, and `code_challenge_method=S256`;
- discovery and token fetches use `redirect: 'error'`;
- raw upstream error bodies are not returned from public helper errors.

- [ ] **Step 5: Run OIDC tests and verify RED**

Run: `cd frontend && pnpm vitest run src/lib/server/identity/oidcClient.test.ts`

Expected: FAIL because the OIDC client does not exist.

- [ ] **Step 6: Implement OIDC primitives with native Fetch/Web Crypto**

Do not add a third-party/provider SDK. Keep returned errors typed into stable internal categories (`configuration`, `temporarily_unavailable`, `access_denied`, `invalid_response`).

- [ ] **Step 7: Run identity primitive tests**

Run:

```bash
cd frontend
pnpm vitest run src/lib/server/identity/config.test.ts src/lib/server/identity/oidcClient.test.ts
pnpm check
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/lib/server/identity
 git commit -m "feat: add provider-neutral OIDC client primitives"
```

### Task 3: Add sealed BFF session and request hook

**Files:**
- Create: `frontend/src/lib/server/identity/session.ts`
- Create: `frontend/src/lib/server/identity/session.test.ts`
- Modify: `frontend/src/app.d.ts`
- Modify: `frontend/src/hooks.server.ts`
- Create: `frontend/src/hooks.server.test.ts`

**Interfaces:**
- Consumes: `IdentityConfig`, `TokenSet`, `refreshAccessToken` from Task 2.
- Produces:

```ts
export const SESSION_COOKIE_NAME = '__Host-schnee_session';
export const LOGIN_TRANSACTION_COOKIE_NAME = '__Host-schnee_login';

export type IdentitySession = {
  accessToken: string;
  accessTokenExpiresAt: number;
  refreshToken: string;
};

export type LoginTransaction = {
  state: string;
  nonce: string;
  codeVerifier: string;
  returnTarget: string;
  expiresAt: number;
};

export async function sealSession(value: IdentitySession, key: Uint8Array): Promise<string>;
export async function openSession(value: string, key: Uint8Array): Promise<IdentitySession | null>;
export async function sealLoginTransaction(value: LoginTransaction, key: Uint8Array): Promise<string>;
export async function openLoginTransaction(value: string, key: Uint8Array): Promise<LoginTransaction | null>;
```

`App.Locals` exposes `accessToken?: string` and `authenticated: boolean`; it never exposes refresh credentials.

- [ ] **Step 1: Write failing sealed-cookie tests**

Cover AES-256-GCM round trip, random nonce per seal, tamper rejection, wrong-key rejection, expired login transaction rejection, and encoded cookie budget `<= 3800` bytes. Test helpers must reject a session that would exceed the budget rather than truncate it.

- [ ] **Step 2: Run session tests and verify RED**

Run: `cd frontend && pnpm vitest run src/lib/server/identity/session.test.ts`

Expected: FAIL because session module does not exist.

- [ ] **Step 3: Implement sealed session/transaction values**

Use Node Web Crypto AES-GCM with a versioned payload. Never log plaintext session contents or cryptographic failures containing secret material.

- [ ] **Step 4: Write failing hook tests**

Cover:

- valid unexpired session -> `locals.authenticated === true` and exact `locals.accessToken`;
- expired access token + valid refresh -> refresh once and rotate session cookie;
- refresh response containing a new refresh token replaces the old one;
- refresh failure -> clear session, `authenticated=false`, no access token;
- malformed/tampered cookie -> clear session and continue unauthenticated;
- production never creates identity from browser request headers or `DEVELOPMENT_USER_ID`.

- [ ] **Step 5: Run hook tests and verify RED**

Run: `cd frontend && pnpm vitest run src/hooks.server.test.ts`

Expected: FAIL before the hook restores sessions.

- [ ] **Step 6: Extend `app.d.ts` and `hooks.server.ts`**

Compose authentication restoration with the existing `applyFrontendSecurityHeaders` behavior; security headers must still apply to every response, including auth failures.

Refresh early enough to avoid forwarding an already-expired access token. Only server-controlled `locals.accessToken` feeds the existing memo proxy.

- [ ] **Step 7: Run hook/session and existing proxy tests**

Run:

```bash
cd frontend
pnpm vitest run src/lib/server/identity/session.test.ts src/hooks.server.test.ts src/lib/server/memoProxy.test.ts
pnpm check
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/lib/server/identity/session.ts frontend/src/lib/server/identity/session.test.ts frontend/src/app.d.ts frontend/src/hooks.server.ts frontend/src/hooks.server.test.ts
git commit -m "feat: resolve provider-neutral BFF sessions"
```

### Task 4: Add login, callback, logout, and authenticated navigation

**Files:**
- Create: `frontend/src/routes/auth/login/+server.ts`
- Create: `frontend/src/routes/auth/callback/+server.ts`
- Create: `frontend/src/routes/auth/logout/+server.ts`
- Create: `frontend/src/routes/auth/session/+server.ts`
- Create: route tests for the four handlers using the repository's Vitest conventions.
- Create: `frontend/src/routes/+layout.server.ts`
- Modify: `frontend/src/routes/+layout.svelte`

**Interfaces:**
- Consumes: Task 2 OIDC helpers, Task 3 cookie helpers, existing `resolveSafeReturnTarget`.
- Produces: same-origin `/auth/login`, `/auth/callback`, `/auth/logout`, `/auth/session` public BFF routes and `PageData.authenticated: boolean`.

- [ ] **Step 1: Write failing login/callback route tests**

Assert login:

- resolves `return_to` through `resolveSafeReturnTarget`;
- creates random `state`, `nonce`, and PKCE verifier;
- writes only the sealed login transaction to `__Host-schnee_login`;
- redirects to the discovered authorization endpoint.

Assert callback:

- rejects missing/mismatched/replayed state;
- rejects missing code;
- exchanges code with the original verifier and exact registered callback URI;
- validates ID-token nonce if an ID token is returned and the implementation chooses to consume it;
- clears transaction cookie on success and terminal failure;
- writes `__Host-schnee_session` and redirects only to the stored safe return target.

- [ ] **Step 2: Run route tests and verify RED**

Run the new auth route test files with `pnpm vitest run ...`.

Expected: FAIL because routes do not exist.

- [ ] **Step 3: Implement login/callback handlers**

Map upstream failures to generic 401/502 responses or a stable same-origin error redirect; never surface raw token/discovery response bodies.

- [ ] **Step 4: Write failing logout/session tests**

Assert logout clears local session before any upstream redirect, and `/auth/session` returns only `{ authenticated: true|false }` with `Cache-Control: no-store`.

- [ ] **Step 5: Implement logout/session handlers and SSR layout state**

`+layout.server.ts` returns only authenticated state. `+layout.svelte` shows provider-neutral `ログイン` / `ログアウト` controls without naming the upstream provider.

- [ ] **Step 6: Run frontend unit/check/lint**

Run:

```bash
cd frontend
pnpm test -- --run
pnpm check
pnpm lint
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/routes/auth frontend/src/routes/+layout.server.ts frontend/src/routes/+layout.svelte
git commit -m "feat: add provider-neutral browser auth flow"
```

### Task 5: Add deterministic provider-neutral auth fixture and E2E coverage

**Files:**
- Create: `scripts/identity-fixture-server.py`
- Modify: `scripts/ui-fixture-server.py`
- Create: `frontend/tests/e2e/auth-session.spec.ts`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: generic OIDC endpoints from Tasks 2-4.
- Produces: deterministic CI-only authorization server fixture supporting discovery, authorize, token authorization-code exchange, refresh rotation, and logout.

- [ ] **Step 1: Write failing E2E cases**

Add Playwright cases:

1. unauthenticated navigation shows login and memo API request is not given a browser-controlled identity;
2. login follows fixture authorization and returns to `/memos`;
3. memo backend fixture receives the server-injected Bearer token;
4. expired access token triggers refresh and rotated refresh credential is subsequently used;
5. logout clears session and subsequent memo access is unauthenticated;
6. forged callback `state` cannot establish a session;
7. malicious external `return_to` resolves to the safe local fallback;
8. upstream fixture error renders only a generic application error;
9. provider-specific response headers/cookies emitted by the fixture are not reflected to the browser.

- [ ] **Step 2: Run E2E and verify RED**

Run with the existing memo fixture plus a placeholder/nonexistent identity fixture command.

Expected: FAIL because identity fixture is absent.

- [ ] **Step 3: Implement `identity-fixture-server.py`**

Use Python stdlib only. Fixture terminology and endpoints are generic; do not emulate or name a production vendor. Make authorization codes one-time, bind them to state-independent PKCE challenge data, support `S256`, and rotate refresh credentials deterministically for assertions.

- [ ] **Step 4: Require server-injected Bearer auth in memo fixture scenario**

Add a scenario/flag to `ui-fixture-server.py` that returns 401 unless the expected CI access token appears in `Authorization: Bearer ...`. Do not accept browser development headers in this scenario.

- [ ] **Step 5: Run authenticated E2E locally**

Run the identity fixture, memo fixture, SvelteKit preview with generic `IDENTITY_*` env, then:

```bash
cd frontend
pnpm playwright test tests/e2e/auth-session.spec.ts
```

Expected: PASS.

- [ ] **Step 6: Wire the fixture into CI**

Start/stop both fixtures reliably, wait for health endpoints, pass only generic `IDENTITY_*` configuration to the frontend, and retain the current unauthenticated/development-mode test paths separately.

- [ ] **Step 7: Commit**

```bash
git add scripts/identity-fixture-server.py scripts/ui-fixture-server.py frontend/tests/e2e/auth-session.spec.ts .github/workflows/ci.yml
git commit -m "test: add provider-neutral auth integration fixture"
```

### Task 6: Final provider-neutral and regression verification

**Files:**
- Modify only files required by failures found during this gate.

**Interfaces:**
- Consumes: all prior tasks.
- Produces: a branch that satisfies the design spec without provider coupling regressions.

- [ ] **Step 1: Run the provider-neutral scanner**

```bash
python3 -m unittest scripts.test_check_provider_neutral_auth -v
python3 scripts/check_provider_neutral_auth.py .
```

Expected: PASS / exit 0.

- [ ] **Step 2: Run the complete frontend gate**

```bash
cd frontend
pnpm format:check
pnpm lint
pnpm check
pnpm test -- --run
pnpm build
pnpm playwright test
```

Expected: PASS.

- [ ] **Step 3: Run backend auth/config regression coverage**

Run the existing Rust tests that cover auth config, JWT issuer/audience/subject/lifetime/type/algorithm rules, JWKS refresh/rotation/duplicate-`kid`, and custom-claim ignorance, followed by the normal backend test suite.

Expected: PASS with no resource-server contract change required.

- [ ] **Step 4: Run repository container/full-stack smoke gates used by CI**

Expected: PASS; production-like mode must not silently fall back to development identity.

- [ ] **Step 5: Inspect public response metadata in E2E**

Confirm login/callback/session/logout responses do not expose raw upstream `Server`, `X-Powered-By`, provider-specific `X-*`, upstream cookies, or internal hostnames.

- [ ] **Step 6: Review diff for accidental implementation names and secrets**

Inspect source/docs/config diff and generated test logs. No concrete production identity product/repository name, token, client secret, session key, auth code, refresh credential, or internal identity hostname may be committed.

- [ ] **Step 7: Commit any final test-only corrections**

```bash
git add <only-files-required-by-verification>
git commit -m "test: close provider-neutral auth verification gaps"
```

## Self-Review Notes

- **Spec coverage:** Stages 1-4 are covered: public-doc neutralization (Task 1), coupling audit/regression gate (Task 1), production browser session integration (Tasks 2-4), and provider-neutral CI fixture (Task 5). Existing backend JWT/JWKS behavior is deliberately retained and verified in Task 6 rather than rewritten.
- **Scope:** Federation internals, SCIM, account-linking implementation, token exchange, identity-platform database design, and history rewriting remain outside this plan as required by the spec.
- **Type consistency:** `IdentityConfig`, `TokenSet`, `IdentitySession`, `LoginTransaction`, and `App.Locals.accessToken/authenticated` are the only cross-task interfaces introduced here.
- **Security consistency:** Provider abstraction is not treated as a trust control; all auth flows remain fail-closed, use safe return targets, and preserve the existing browser-to-memo BFF boundary.
- **Operational consistency:** The frontend requires only generic identity issuer/client/session-key configuration; the upstream implementation can change behind the stable public issuer as long as its OIDC contract remains compatible.
