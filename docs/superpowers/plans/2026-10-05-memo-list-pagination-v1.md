# Memo List Pagination v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace unbounded normal memo-list reads with a versioned, bounded cursor API while preserving a bounded legacy compatibility response and the MEMO-HIGH-1 metadata boundary.

**Architecture:** Add a `cursor-v1` UUIDv4-descending keyset contract above the current legacy/encrypted route switch. Every authoritative store reads at most `limit + 1` records, the encrypted adapter decrypts only that bounded page, and the first-party SvelteKit UI consumes pages with an explicit load-more control. The old array response remains only as a bounded <=100-item compatibility bridge.

**Tech Stack:** Rust 2021, Actix Web, ScyllaDB Rust driver, MongoDB Rust driver, SvelteKit/Svelte 5, TypeScript, Vitest, Playwright, pnpm 9.15.9.

**Spec:** `docs/superpowers/specs/2026-10-05-memo-list-pagination-v1-design.md`

## Global Constraints

- `cursor-v1` ordering is canonical UUIDv4 descending; do not add plaintext `updated_at` or another new memo sort field to encrypted storage.
- Default page size is 20; maximum page size is 100; one store call may inspect at most 101 records/envelopes.
- The normal paginated response has no exact total count.
- The legacy unversioned array response may return at most 100 memos and must fail explicitly rather than truncate if another memo exists.
- Owner scoping is mandatory in every page query; cursor values never replace the owner predicate.
- Legacy and encrypted data routes must interpret the same cursor identically.
- The frontend uses an explicit `さらに読み込む` control; no infinite-scroll observer in v1.
- Existing BFF authentication, CSRF, path/method/header/query hardening, 30-second backend deadline, identity encoding, response allowlist, and `Cache-Control: no-store` remain intact.
- Existing search pagination semantics are unchanged.

## Review Focus

- **Cross-route ordering:** the same owner IDs must produce the same page boundary in ScyllaDB, plaintext MongoDB, and encrypted MongoDB.
- **Cursor parser ambiguity:** non-canonical, non-v4, unsupported-version, duplicate, or trailing-data cursors must fail before storage access.
- **Legacy compatibility:** <=100 legacy rows still return the old complete `updated_at DESC` array; >100 returns an explicit client error without an unbounded read.
- **Concurrent mutations:** updates must not duplicate items across pages because the cursor key is immutable; docs must state that inserts are not snapshot-consistent.
- **Encrypted work bound:** one HIGH list page must decrypt no more than the bounded envelope page and must not call the old all-owner loader.

---

### Task 1: Define the cursor-v1 application contract

**Files:**
- Modify: `backend/src/domain/memo/repository.rs`
- Modify: `backend/src/application/memo/dto.rs`
- Modify: `backend/src/application/memo/service.rs`
- Test: `backend/src/application/memo/service.rs` test module

**Interfaces:**
- Produces: `MemoListPage { items: Vec<Memo>, has_more: bool }`.
- Produces: `MemoRepository::list_page_by_user_id(user_id: Uuid, after: Option<Uuid>, limit: usize) -> AppResult<MemoListPage>`.
- Produces: a versioned list response DTO with fields `pagination`, `items`, `limit`, `next_cursor`.
- Produces: cursor-v1 parse/format helpers used by the REST/service layer.
- Removes request-path dependence on `MemoRepository::find_all_by_user_id`.

- [ ] **Step 1: Write failing service tests for pagination input validation**

Add tests that assert:

- no cursor parses as first page;
- `v1.<canonical-lowercase-v4>` parses to the expected UUID;
- uppercase/non-canonical UUID text, UUIDv1/v7/nil, missing `v1.` prefix, `v2.*`, control characters, and trailing data return `AppError::BadRequest`;
- `limit=0` and `limit=101` are rejected;
- omitted limit resolves to 20.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cd backend && cargo test --locked memo::service -- --nocapture
```

Expected: the new cursor/page tests fail because the v1 types/helpers do not exist.

- [ ] **Step 3: Add the minimal domain/DTO/service interfaces**

Implement the exact `MemoListPage` and bounded repository method. Add a response DTO equivalent to:

```rust
pub struct MemoListResponse {
    pub pagination: &'static str,
    pub items: Vec<MemoResponse>,
    pub limit: usize,
    pub next_cursor: Option<String>,
}
```

Use constant pagination name `cursor-v1`, cursor prefix `v1.`, default 20, maximum 100. Require UUID version 4 and canonical lowercase formatting.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run the same `cargo test --locked memo::service -- --nocapture` command.
Expected: new validation tests pass.

- [ ] **Step 5: Commit**

```bash
git add backend/src/domain/memo/repository.rs backend/src/application/memo/dto.rs backend/src/application/memo/service.rs
git commit -m "feat: define memo list cursor v1 contract"
```

---

### Task 2: Add bounded ScyllaDB and plaintext MongoDB list pages

**Files:**
- Modify: `backend/src/infrastructure/persistence/ports.rs`
- Modify: `backend/src/infrastructure/persistence/scylla.rs`
- Modify: `backend/src/infrastructure/persistence/mongodb.rs`
- Modify: `backend/src/infrastructure/repositories/memo.rs`
- Test: existing Scylla/MongoDB unit/integration test modules in those files

**Interfaces:**
- Consumes: `MemoListPage` and `MemoRepository::list_page_by_user_id` from Task 1.
- Produces: `MemoAuthoritativeStore::list_page_by_user_id(...)` with the same owner/after/limit semantics.
- Produces: bounded ScyllaDB and plaintext MongoDB implementations.

- [ ] **Step 1: Write failing store tests for the page boundary**

For both ScyllaDB and plaintext MongoDB integration coverage, seed at least four UUIDv4 memos for one owner and one memo for another owner. With logical limit 2, assert:

- first page contains the two greatest owner UUIDs in descending order;
- `has_more` is true;
- second page, using the last ID from page 1 as exclusive cursor, contains the remaining owner IDs only;
- no ID is duplicated across pages;
- the other owner's memo never appears.

Also add a repository-layer test proving the adapter delegates to the bounded store method rather than an all-owner read.

- [ ] **Step 2: Run focused tests and verify RED**

Run the repository/unit tests plus the existing MongoDB transactional integration command used by CI. For Scylla-specific tests, run the existing backend test target or integration harness that is available in CI/local Compose.

Expected: failures because the bounded store methods/prepared statements/index do not exist.

- [ ] **Step 3: Implement bounded ScyllaDB paging**

Add prepared statements for first/subsequent pages using the existing `(user_id)` partition and `id` clustering key. Execute with physical limit `logical_limit + 1`, truncate to logical limit, and compute `has_more`. Do not issue an unpaged all-owner request in the normal list method.

- [ ] **Step 4: Implement bounded plaintext MongoDB paging**

Add/ensure `{ user_id: 1, _id: -1 }`, filter by owner and optional `_id < cursor`, sort `_id: -1`, limit to `logical_limit + 1`, convert only those documents, and compute `has_more`.

- [ ] **Step 5: Wire the repository adapter**

Replace `find_all_by_user_id` delegation with the new bounded page contract.

- [ ] **Step 6: Run tests and verify GREEN**

Run:

```bash
cd backend && cargo test --locked
```

and the MongoDB integration gate used by CI.
Expected: all backend tests pass; page-boundary tests pass.

- [ ] **Step 7: Commit**

```bash
git add backend/src/infrastructure/persistence/ports.rs backend/src/infrastructure/persistence/scylla.rs backend/src/infrastructure/persistence/mongodb.rs backend/src/infrastructure/repositories/memo.rs
git commit -m "feat: bound plaintext memo list reads"
```

---

### Task 3: Bound the encrypted authoritative route without new plaintext metadata

**Files:**
- Modify: `backend/src/infrastructure/persistence/ports.rs`
- Modify: `backend/src/infrastructure/persistence/mongodb.rs`
- Modify: `backend/src/infrastructure/high_memo_authoritative.rs`
- Test: `backend/src/infrastructure/high_memo_authoritative.rs` test module
- Test: MongoDB encrypted integration tests

**Interfaces:**
- Consumes: `MemoAuthoritativeStore::list_page_by_user_id` from Task 2.
- Produces: `HighEncryptedMemoAuthoritativeStore::page_envelopes_by_owner(owner_partition, after, limit)`.
- Guarantees: HIGH list page decrypts only the bounded envelope page.

- [ ] **Step 1: Write failing encrypted-route tests**

Extend `FakeEncryptedStore` with a bounded page method and instrumentation. Seed more envelopes than the page size and assert:

- output IDs follow UUID-descending cursor-v1 order;
- only one owner is returned;
- `has_more` is correct;
- cryptography/decrypt call count is no greater than the bounded records needed for one page;
- the normal list path never invokes an all-owner loader.

Add MongoDB coverage for owner + `_id` cursor filtering.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
cd backend && cargo test --locked high_memo_authoritative -- --nocapture
```

Expected: failures because the bounded encrypted-store method is absent.

- [ ] **Step 3: Implement bounded encrypted MongoDB paging**

Add `{ owner_partition: 1, _id: -1 }` to encrypted index setup, page by owner and optional `_id < cursor`, sort descending, and apply the physical bounded limit.

- [ ] **Step 4: Implement HIGH adapter page decryption**

Fetch only the bounded envelope page, decrypt only those envelopes, validate identity/version with the existing `decrypt_checked`, and return `MemoListPage`. Remove request-path use of `find_all_envelopes_by_owner`.

- [ ] **Step 5: Run focused and full backend tests**

Run:

```bash
cd backend && cargo test --locked high_memo_authoritative -- --nocapture
cd backend && cargo test --locked
```

Expected: all pass and bounded-decrypt assertions are green.

- [ ] **Step 6: Commit**

```bash
git add backend/src/infrastructure/persistence/ports.rs backend/src/infrastructure/persistence/mongodb.rs backend/src/infrastructure/high_memo_authoritative.rs
git commit -m "feat: bound encrypted memo list reads"
```

---

### Task 4: Expose cursor-v1 and bounded legacy compatibility over REST

**Files:**
- Modify: `backend/src/application/memo/service.rs`
- Modify: `backend/src/interfaces/rest/memo.rs`
- Test: service tests and REST/full-stack tests
- Modify: `docs/authentication.md` only if list API contract is documented there; otherwise use the repository's API/README documentation location discovered during implementation

**Interfaces:**
- Consumes: bounded repository pages from Tasks 2–3.
- Produces: `GET /api/v1/memos?pagination=cursor-v1[&limit=N][&cursor=v1.UUID]`.
- Preserves: unversioned array response for complete owner sets of at most 100 memos.

- [ ] **Step 1: Write failing service/REST tests for both response modes**

Assert:

- paginated request returns `{ pagination: "cursor-v1", items, limit, next_cursor }`;
- page 2 uses the exclusive cursor;
- bad pagination version/cursor/limit returns 400;
- unversioned request with <=100 memos returns the old JSON array sorted `updated_at DESC`;
- unversioned request whose bounded page reports `has_more=true` returns 400 with stable message `Memo list pagination is required; use pagination=cursor-v1`;
- the legacy path calls the repository only once with logical limit 100.

- [ ] **Step 2: Run focused tests and verify RED**

Run the relevant service/REST tests. Expected: failures because `list_memos` does not accept/query pagination yet.

- [ ] **Step 3: Implement paginated and compatibility service methods**

Add a v1 method that validates pagination input, calls one bounded repository page, and derives `next_cursor` from the last returned item only when `has_more` is true.

Keep a separate legacy compatibility method that requests logical limit 100, errors on `has_more`, otherwise sorts the complete <=100 set by `updated_at DESC` and returns the old array.

- [ ] **Step 4: Update REST query parsing**

Use optional `pagination`, `cursor`, and `limit` fields. An entirely query-free request takes the legacy branch. Any pagination field present requires `pagination=cursor-v1` and returns the new shape.

- [ ] **Step 5: Run full backend tests**

Run:

```bash
cd backend && cargo fmt --check
cd backend && cargo check --locked --all-features
cd backend && cargo clippy --locked --all-targets --all-features -- -D warnings
cd backend && cargo test --locked
```

Expected: all exit 0.

- [ ] **Step 6: Commit**

```bash
git add backend/src/application/memo/service.rs backend/src/interfaces/rest/memo.rs docs
git commit -m "feat: expose bounded memo list pagination"
```

---

### Task 5: Extend the BFF query contract for memo-list pagination

**Files:**
- Modify: `frontend/src/lib/server/memoProxyQuery.ts`
- Modify: `frontend/src/lib/server/memoProxyQuery.test.ts` (or the existing test file that owns this module)

**Interfaces:**
- Consumes: server public query keys `pagination`, `cursor`, `limit` from Task 4.
- Preserves: search query keys `query`, `tag`, `page`, `limit`; item routes accept no query.

- [ ] **Step 1: Write failing query-allowlist tests**

Cover:

- `memos?pagination=cursor-v1&limit=20&cursor=...` accepted;
- each allowed key accepted at most once;
- unknown list keys rejected;
- duplicate list keys rejected;
- `memos/:id` still rejects all query parameters;
- existing `memos/search` cases remain unchanged.

- [ ] **Step 2: Run the focused Vitest file and verify RED**

Run the test file directly with `pnpm test:unit -- --run <path>`.
Expected: paginated list queries fail under the current search-only allowlist.

- [ ] **Step 3: Implement route-specific list query keys**

Keep BFF responsibility at key-shape validation only. Do not duplicate cursor-value parsing in TypeScript.

- [ ] **Step 4: Run frontend unit suite and verify GREEN**

Run:

```bash
cd frontend && pnpm test:unit -- --run
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/server/memoProxyQuery.ts frontend/src/lib/server/*test*
git commit -m "security: allowlist memo list pagination query"
```

---

### Task 6: Migrate the frontend memo list to cursor-v1 with load-more UX

**Files:**
- Modify: `frontend/src/lib/api/types.ts`
- Modify: `frontend/src/lib/api/memo.ts`
- Modify: frontend API unit tests
- Modify: `frontend/src/routes/memos/+page.svelte`
- Modify: component/page tests as appropriate
- Modify: `scripts/ui-fixture-server.py`
- Modify: `frontend/tests/e2e/memo-bff.spec.ts`

**Interfaces:**
- Consumes: cursor-v1 HTTP contract from Task 4.
- Produces: `fetchMemosPage({ cursor?, limit? }) -> Promise<MemoListPage>`.
- Produces: explicit load-more UI driven by `next_cursor`.

- [ ] **Step 1: Write failing API-client tests**

Assert first-page and next-page URLs, response type expectations, and that cursor text is URL-encoded.

- [ ] **Step 2: Write failing page/component tests**

Assert:

- initial page renders page-1 items;
- `さらに読み込む` appears only with `next_cursor`;
- clicking it appends page-2 items without replacing page 1;
- the control is disabled while the request is pending;
- load-more 401 enters the current auth-required state without clearing already loaded memos;
- terminal page removes the control.

- [ ] **Step 3: Verify RED with frontend unit tests**

Run focused tests. Expected: failures because `fetchMemos` still expects an array and the page has no load-more state.

- [ ] **Step 4: Implement the API client and page state**

Use default page size 20. Keep explicit button-based pagination; do not add infinite scroll. Deduplicate by immutable memo ID defensively when appending, without changing server ordering.

- [ ] **Step 5: Extend the deterministic fixture and Browser E2E**

Make the fixture return at least two cursor-v1 pages. Add a Playwright path proving page 1 renders, `さらに読み込む` requests page 2 through the real BFF, and page 2 appends without duplicates.

- [ ] **Step 6: Run frontend verification**

Run:

```bash
cd frontend && pnpm check
cd frontend && pnpm lint
cd frontend && pnpm test:unit -- --run
cd frontend && pnpm build
cd frontend && pnpm test:e2e
```

Expected: all exit 0.

- [ ] **Step 7: Commit**

```bash
git add frontend scripts/ui-fixture-server.py
git commit -m "feat: page memo list in the frontend"
```

---

### Task 7: Prove cross-route semantics, remove unbounded request-path methods, and document rollout

**Files:**
- Modify: any remaining fake/test implementations of `MemoRepository`, `MemoAuthoritativeStore`, and `HighEncryptedMemoAuthoritativeStore`
- Modify: `README.md`
- Modify: `docs/authentication.md` only for BFF/API-boundary details if appropriate
- Create or modify: API/migration pagination documentation under `docs/`
- Modify: issue #135 tracking text/comment after merge

**Interfaces:**
- Consumes: all prior tasks.
- Produces: no remaining normal request-path `find_all_by_user_id` / `find_all_envelopes_by_owner` dependency.
- Produces: documented cursor-v1 compatibility and route-cutover contract.

- [ ] **Step 1: Add cross-route contract tests**

Using identical owner UUIDv4 memo IDs, prove legacy/plaintext and encrypted repositories return the same page IDs, `has_more`, and next-page boundary. Include exact max page size and owner-isolation cases.

- [ ] **Step 2: Search for unbounded request-path callers**

Run repository code search for `find_all_by_user_id` and `find_all_envelopes_by_owner`. Remove obsolete request-path trait methods/callers after distinguishing operational migration/integrity traversal APIs.

Expected: no normal list request can call an all-owner materializer.

- [ ] **Step 3: Document public and migration semantics**

Document:

- cursor-v1 syntax and examples;
- 20 default / 100 max;
- UUIDv4-descending order and change from legacy recency order;
- <=100 legacy compatibility and explicit >100 error;
- concurrent-insert non-snapshot semantics;
- no new HIGH plaintext sort metadata;
- route-cutover consistency requirement.

- [ ] **Step 4: Run repository-wide verification**

Run backend and frontend suites from Tasks 4 and 6 plus Compose/full-stack smoke. On the PR, require the normal CI workflow; because frontend files changed, UI Diff and Lighthouse must also run and pass.

Expected: zero failures and no relevant warnings.

- [ ] **Step 5: Commit**

```bash
git add backend frontend docs README.md scripts
git commit -m "docs: finalize memo list pagination v1 rollout"
```

## Final verification before merge

- [ ] Re-read issue #135 and map every acceptance criterion to a passing test or documented contract.
- [ ] Verify `git diff main...HEAD` contains no authentication-service implementation or unrelated refactor.
- [ ] Verify latest PR head has successful CI / UI Diff / Lighthouse where applicable.
- [ ] Verify the legacy route is physically bounded even when the user owns more than 100 memos.
- [ ] Verify encrypted list paging adds no plaintext timestamp/sort metadata.
- [ ] Verify first-party frontend no longer calls the legacy array list path.
- [ ] Squash merge only after the latest head is fully green.
