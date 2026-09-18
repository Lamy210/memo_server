# Memo Server Revival Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore `memo_server` as a working, testable memo CRUD/search application with deterministic local startup, coherent Rust/Actix persistence behavior, and a usable SvelteKit UI.

**Architecture:** Keep the existing modular-monolith direction. Scylla is authoritative storage, Redis is a disposable lookup cache, and Elasticsearch is a rebuildable search projection. The frontend keeps all HTTP behavior in `src/lib/api`, with route/store/component state layered above it.

**Tech Stack:** Rust 2021, Actix Web, ScyllaDB, Redis, Elasticsearch, SvelteKit 2, Svelte 5, TypeScript, Vite, Tailwind CSS, Vitest, Playwright, Docker Compose, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-18-memo-server-revival-design.md`

## Global Constraints

- Preserve `/api/v1` as the API base path.
- Keep `DEVELOPMENT_USER_ID=12345678-1234-1234-1234-123456789012` as the explicit local-development identity; do not present it as authentication.
- Scylla is the source of truth; Redis and Elasticsearch failures must not be modeled as distributed transactions.
- Update requests require the current memo `version`; stale updates return HTTP 409.
- Domain validation returns 422; malformed transport input returns 400; ownership-sensitive misses return 404.
- No active UI for OAuth, attachments, sharing, real-time collaboration, CRDT/WebRTC, CQRS/Event Sourcing, or microservice features in this revival.
- Rust gates: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo check`.
- Frontend gates: `pnpm install --frozen-lockfile`, `pnpm check`, `pnpm lint`, `pnpm test:unit -- --run`, `pnpm build`.
- Prefer focused repairs and delete dead placeholders rather than preserving speculative abstractions.

---

## File map

### Backend

- `backend/src/main.rs`: executable entrypoint only; no duplicate local module tree.
- `backend/src/startup.rs`: configuration-driven application construction and Actix server wiring.
- `backend/src/config.rs`: environment parsing/validation, including development user ID.
- `backend/src/error.rs`: stable HTTP error mapping/envelope.
- `backend/src/domain/memo/entity.rs`: infrastructure-free memo invariants and version behavior.
- `backend/src/domain/memo/repository.rs`: user-scoped repository contract.
- `backend/src/application/memo/dto.rs`: stable request/response/search DTOs.
- `backend/src/application/memo/service.rs`: memo use cases, ownership, validation, conflict semantics.
- `backend/src/infrastructure/persistence/scylla.rs`: access-pattern tables and primary persistence operations.
- `backend/src/infrastructure/persistence/redis.rs`: cache implementation.
- `backend/src/infrastructure/persistence/elasticsearch.rs`: search projection and pagination.
- `backend/src/infrastructure/repositories/memo.rs`: orchestration of authoritative storage/cache/search projection.
- `backend/src/interfaces/rest/memo.rs`: HTTP extraction/status mapping only.
- `backend/src/interfaces/routes.rs`: `/api/v1` routing, liveness and readiness.

### Frontend

- `frontend/src/lib/api/types.ts`: canonical memo/Create/Update/search types.
- `frontend/src/lib/api/memo.ts`: all memo HTTP calls, including delete and typed API errors.
- `frontend/src/lib/stores/memoStore.ts`: list/search state without direct fetch calls.
- `frontend/src/lib/stores/editorStore.ts`: dirty/save/version/conflict state.
- `frontend/src/lib/components/features/editor/*`: focused editor/preview/tag controls.
- `frontend/src/routes/+page.ts`: root redirect or deterministic landing behavior.
- `frontend/src/routes/memos/+page.svelte`: memo list.
- `frontend/src/routes/memos/new/+page.svelte`: new memo editor.
- `frontend/src/routes/memos/[id]/edit/+page.svelte`: existing memo editor.
- `frontend/src/routes/memos/search/+page.svelte`: URL-backed search.
- `frontend/src/routes/+layout.svelte`: working navigation only.
- `frontend/vite.config.js`: proxy `/api` without path stripping.
- `frontend/tsconfig.json`, `frontend/package.json`, `frontend/svelte.config.js`: deterministic Svelte/TS checks.

### Operations/docs

- `.github/workflows/ci.yml`: backend/frontend CI gates.
- `docker-compose.yml`: actually starts application services, health-gated dependencies, optional Kibana profile.
- `docker/backend/Dockerfile`, `docker/frontend/Dockerfile`: deterministic dev commands/tooling.
- `.env.example`, `README.md`, `CONTRIBUTING.md`: setup, commands, architecture, API and coding conventions.

---

### Task 1: Establish executable quality gates

**Files:**
- Create: `.github/workflows/ci.yml`
- Modify: `frontend/package.json`
- Modify: `frontend/tsconfig.json`
- Modify: `frontend/svelte.config.js`

**Interfaces:**
- Consumes: current Rust and Svelte projects.
- Produces: independent `backend` and `frontend` CI jobs that expose concrete baseline failures.

- [ ] **Step 1: Correct frontend check/test scripts before enabling CI**

Set `check` to use `./tsconfig.json`, include `src/**/*.ts` in TypeScript inputs, and keep package-manager usage consistently on pnpm.

- [ ] **Step 2: Add CI workflow**

Backend job commands:

```bash
cd backend
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo check
```

Frontend job commands:

```bash
cd frontend
corepack enable
pnpm install --frozen-lockfile
pnpm check
pnpm lint
pnpm test:unit -- --run
pnpm build
```

- [ ] **Step 3: Push the baseline gate commit and inspect GitHub Actions failures**

Expected: failures are acceptable at this point only when they identify pre-existing compile/type/lint problems that later tasks repair.

- [ ] **Step 4: Commit**

```bash
git commit -m "ci: establish backend and frontend quality gates"
```

---

### Task 2: Repair backend startup/configuration and domain compilation

**Files:**
- Create: `backend/src/config.rs`
- Modify: `backend/src/lib.rs`
- Modify: `backend/src/main.rs`
- Modify: `backend/src/startup.rs`
- Modify: `backend/src/domain/memo/entity.rs`
- Modify: `backend/src/infrastructure/repositories/memo.rs`

**Interfaces:**
- Produces: `AppConfig::from_env() -> Result<AppConfig, ConfigError>`; synchronous `MemoRepositoryImpl::new(...) -> Self`; domain `Memo` without Scylla serialization imports.

- [ ] **Step 1: Add failing configuration tests**

Cover invalid `PORT`, invalid `DEVELOPMENT_USER_ID`, and known-good defaults.

- [ ] **Step 2: Run backend tests/check and record failure**

```bash
cd backend && cargo test config && cargo check
```

- [ ] **Step 3: Implement typed configuration and simplify `main.rs`**

`main.rs` imports library modules only and does not redeclare `mod application`, `mod domain`, `mod infrastructure`, etc.

- [ ] **Step 4: Remove infrastructure serialization code from the domain entity**

Keep `Memo` as serde/domain data only; Scylla rows are serialized/deserialized in infrastructure.

- [ ] **Step 5: Make repository construction synchronous and map startup errors instead of panicking**

- [ ] **Step 6: Run Rust quality gates**

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo check
```

- [ ] **Step 7: Commit**

```bash
git commit -m "fix: repair backend startup and configuration"
```

---

### Task 3: Make memo invariants and optimistic concurrency testable

**Files:**
- Modify: `backend/src/domain/memo/entity.rs`
- Modify: `backend/src/application/memo/dto.rs`
- Modify: `backend/src/application/memo/service.rs`
- Test: colocated Rust unit tests in domain/application modules

**Interfaces:**
- Consumes: `MemoRepository` trait.
- Produces: normalized tags, title validation, `UpdateMemoDto.version`, stale-version rejection, version increment.

- [ ] **Step 1: Write failing domain tests**

Tests assert blank title is rejected, tags are trimmed/deduplicated, empty tags are rejected, >10 tags are rejected, and successful update increments version exactly once.

- [ ] **Step 2: Write failing service test for stale version**

A fake repository returns version 3; update request carries version 2; service returns `AppError::Conflict` without saving.

- [ ] **Step 3: Implement the minimal domain/service changes**

Create validates before save; update validates after applying changes and before save.

- [ ] **Step 4: Run targeted then full backend tests**

```bash
cargo test memo
cargo test
```

- [ ] **Step 5: Commit**

```bash
git commit -m "fix: enforce memo validation and version conflicts"
```

---

### Task 4: Correct Scylla access patterns and repository semantics

**Files:**
- Modify: `backend/src/domain/memo/repository.rs`
- Modify: `backend/src/infrastructure/persistence/scylla.rs`
- Modify: `backend/src/infrastructure/persistence/redis.rs`
- Modify: `backend/src/infrastructure/repositories/memo.rs`

**Interfaces:**
- Produces: direct-by-id storage via `memos_by_id`; ordered user listing via `memos_by_user`; cache invalidation/update behavior; user-scoped ownership checks.

- [ ] **Step 1: Add repository-contract tests with an in-memory fake**

Demonstrate required `find_by_id`, `find_all_by_user_id`, `save`, `delete`, `search`, and `exists` semantics before changing infrastructure.

- [ ] **Step 2: Replace invalid id-only queries against the user-partitioned table**

Create access-pattern-specific tables defined in the spec:

```sql
CREATE TABLE IF NOT EXISTS memo_app.memos_by_id (... PRIMARY KEY (id));
CREATE TABLE IF NOT EXISTS memo_app.memos_by_user (... PRIMARY KEY ((user_id), updated_at, id));
```

- [ ] **Step 3: Use ordinary prepared statements for single reads/writes**

Do not wrap single SELECT/DELETE statements in batches. If a batch is used for denormalized writes, document why it is safe for the partition characteristics; otherwise execute explicit writes and surface partial-failure observability.

- [ ] **Step 4: Make Redis non-authoritative**

Cache read failures fall through to Scylla. Cache write/delete failures are logged and do not convert a successful authoritative write into a false rollback.

- [ ] **Step 5: Run backend checks and integration compilation against the pinned Scylla client version**

```bash
cargo test
cargo check
```

- [ ] **Step 6: Commit**

```bash
git commit -m "fix: align memo persistence with Scylla access patterns"
```

---

### Task 5: Stabilize HTTP contract, search pagination, and error envelope

**Files:**
- Modify: `backend/src/error.rs`
- Modify: `backend/src/application/memo/dto.rs`
- Modify: `backend/src/application/memo/service.rs`
- Modify: `backend/src/infrastructure/persistence/elasticsearch.rs`
- Modify: `backend/src/interfaces/rest/memo.rs`
- Modify: `backend/src/interfaces/routes.rs`

**Interfaces:**
- Produces: stable `{ "error": { "code", "message" } }` errors; CRUD status policy; `/api/v1/ready`; search honors `page` and `limit`.

- [ ] **Step 1: Write failing Actix response tests**

Assert create 201, delete 204, validation 422, unknown/ownership-sensitive memo 404, stale version 409, and stable JSON error envelope.

- [ ] **Step 2: Write failing pagination tests**

`page=2&limit=10` must result in Elasticsearch `from=10,size=10` semantics and response metadata reflecting page 2/limit 10.

- [ ] **Step 3: Implement handlers without hard-coded per-request UUID parsing**

Inject configured development user ID through application state.

- [ ] **Step 4: Set Elasticsearch single-node development replicas to 0 and use direct document deletion when possible**

- [ ] **Step 5: Run backend tests/checks**

- [ ] **Step 6: Commit**

```bash
git commit -m "fix: stabilize memo HTTP and search contracts"
```

---

### Task 6: Repair frontend type/build baseline and API layer

**Files:**
- Modify: `frontend/src/lib/api/types.ts`
- Modify: `frontend/src/lib/api/memo.ts`
- Modify: `frontend/src/lib/types/search.ts` or remove it after consolidation
- Modify: `frontend/src/lib/stores/memoStore.ts`
- Modify: `frontend/src/lib/stores/editorStore.ts`
- Modify: `frontend/vite.config.js`

**Interfaces:**
- Produces: `Memo.version`; explicit `CreateMemoInput`, `UpdateMemoInput`, `SearchParams`, `SearchResult`; `deleteMemo`; typed `ApiError`; API-only network access.

- [ ] **Step 1: Add failing API/store unit tests**

Cover update payload preserving `version`, 409 mapping to conflict, delete request method, and memo-store use of API functions rather than direct fetch.

- [ ] **Step 2: Correct broken import paths and canonicalize types**

- [ ] **Step 3: Import/use `get` correctly or refactor editor state so no undefined store helper remains**

Use `ReturnType<typeof setTimeout>` for browser timers.

- [ ] **Step 4: Remove the Vite `/api` rewrite**

Proxy target remains backend service, but `/api/v1/...` reaches Actix unchanged.

- [ ] **Step 5: Run frontend targeted tests/check/build**

```bash
pnpm check
pnpm test:unit -- --run
pnpm build
```

- [ ] **Step 6: Commit**

```bash
git commit -m "fix: normalize frontend memo API and state"
```

---

### Task 7: Complete working memo routes and editor UX

**Files:**
- Create: `frontend/src/routes/memos/+page.svelte`
- Create: `frontend/src/routes/memos/new/+page.svelte`
- Modify: `frontend/src/routes/memos/[id]/edit/+page.svelte`
- Modify: `frontend/src/routes/memos/[id]/edit/+page.ts`
- Modify: `frontend/src/routes/memos/search/+page.svelte`
- Modify: `frontend/src/routes/memos/search/+page.ts`
- Modify: `frontend/src/routes/+layout.svelte`
- Modify/Create focused components under `frontend/src/lib/components/features/editor/`

**Interfaces:**
- Consumes: API/store contract from Task 6.
- Produces: list/create/edit/search/delete flow with dirty/saving/saved/error/conflict states.

- [ ] **Step 1: Write component tests for list empty/error states and editor save/conflict states**

- [ ] **Step 2: Implement `/memos` list and `/memos/new` creation route**

List has loading/empty/error/retry states and a clear New memo action.

- [ ] **Step 3: Refactor edit route around one focused editor surface**

Editor includes title, tags, Markdown body, preview, explicit save, Cmd/Ctrl+S, dirty-state debounce autosave, server version preservation, and conflict UI that never silently overwrites.

- [ ] **Step 4: Implement search URL state**

Text/tag filters are represented in query parameters and survive refresh/navigation.

- [ ] **Step 5: Remove or disable dead navigation/actions**

Do not present profile/logout/shared/notification controls as working when their behavior is out of scope.

- [ ] **Step 6: Run `pnpm check`, lint, tests, build**

- [ ] **Step 7: Commit**

```bash
git commit -m "feat: complete memo workspace UX"
```

---

### Task 8: Make Docker Compose start the application deterministically

**Files:**
- Modify: `docker-compose.yml`
- Modify: `docker/backend/Dockerfile`
- Modify: `docker/frontend/Dockerfile`
- Create: `.env.example`

**Interfaces:**
- Produces: one documented `docker compose up --build` path exposing frontend, backend, Scylla, Redis and Elasticsearch; Kibana optional profile.

- [ ] **Step 1: Replace idle-shell commands with real dev server commands**

Frontend:

```bash
pnpm dev --host 0.0.0.0 --port 3000
```

Backend: run Actix (cargo-watch is optional but must not be required for correctness).

- [ ] **Step 2: Correct internal connection values**

Use Scylla node address format accepted by the pinned Rust driver, Redis service URL, Elasticsearch service URL, and explicit development user ID.

- [ ] **Step 3: Add health checks and `depends_on` health conditions where reliable**

- [ ] **Step 4: Put Kibana behind a Compose profile**

- [ ] **Step 5: Start stack and exercise health/readiness**

```bash
docker compose up --build
curl -fsS http://localhost:8083/api/v1/health
curl -fsS http://localhost:8083/api/v1/ready
```

- [ ] **Step 6: Commit**

```bash
git commit -m "fix: make compose start a usable development stack"
```

---

### Task 9: Add end-to-end memo lifecycle coverage

**Files:**
- Modify/Create: `frontend/tests/*` or existing Playwright test location
- Modify: `frontend/playwright.config.*` if needed
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: browser proof for create -> list -> edit -> search -> delete.

- [ ] **Step 1: Write Playwright smoke test before wiring it into CI**

Flow assertions:

```text
open /memos
create memo with title/body/tag
return to list and observe title
edit title/body and save
search by text/tag and observe memo
delete memo with confirmation
observe empty/not-found state
```

- [ ] **Step 2: Run test against Compose and fix only behavior required by the acceptance criteria**

- [ ] **Step 3: Add an integration CI job with required service dependencies**

- [ ] **Step 4: Commit**

```bash
git commit -m "test: cover primary memo lifecycle end to end"
```

---

### Task 10: Documentation, cleanup, and final verification

**Files:**
- Create/Modify: `README.md`
- Create/Modify: `CONTRIBUTING.md`
- Modify: historical docs only where a status note is needed
- Delete: empty/dead placeholder modules that are not referenced by the working application

**Interfaces:**
- Produces: setup and contribution docs that match actual behavior; no misleading active architecture claims.

- [ ] **Step 1: Document quick start, ports, environment, quality commands, API endpoints, storage roles and known future work**

- [ ] **Step 2: Document coding conventions and PR expectations**

- [ ] **Step 3: Remove dead placeholder files/imports verified unused by compiler and frontend checks**

- [ ] **Step 4: Run final local/CI-equivalent quality suite**

```bash
cd backend
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo check

cd ../frontend
corepack enable
pnpm install --frozen-lockfile
pnpm check
pnpm lint
pnpm test:unit -- --run
pnpm build
```

Then run the Compose + Playwright smoke flow.

- [ ] **Step 5: Inspect all GitHub Actions jobs and PR diff; resolve reviewable regressions before marking ready**

- [ ] **Step 6: Commit**

```bash
git commit -m "docs: finalize memo server revival guidance"
```

---

## Execution checkpoints

1. After Task 1: CI exposes the baseline truth.
2. After Tasks 2-5: backend compiles, starts, and implements the stable API contract.
3. After Tasks 6-7: frontend builds and the complete memo flow is usable.
4. After Tasks 8-9: Compose and browser smoke prove integration.
5. After Task 10: all gates green; Draft PR can move to ready-for-review.

No phase is considered complete based only on code inspection; it requires the corresponding executable checks.