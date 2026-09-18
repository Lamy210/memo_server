# memo_server revival design

Date: 2026-09-18
Status: approved for implementation planning
Branch: `fix/revive-memo-server`

## 1. Purpose

Revive `memo_server` from its unfinished 2024 state into a locally usable and continuously verifiable memo application while preserving the existing Rust/Actix + SvelteKit direction.

The first delivery is intentionally smaller than the historical design document. The goal is a reliable memo MVP that can later support authentication, attachments, sharing, and collaboration without forcing those unfinished features into the critical path now.

## 2. Current-state findings

The repository currently contains several independent failure modes rather than one isolated bug:

- Docker Compose keeps the frontend and backend containers alive with shells instead of starting the applications.
- The backend persistence model and Scylla queries are inconsistent. The existing table uses `PRIMARY KEY ((user_id), id)` while some queries attempt access by `id` alone.
- `MemoRepositoryImpl::new` is asynchronous even though construction is purely local, and its caller does not await it.
- Backend startup uses `expect` around infrastructure initialization, which turns recoverable configuration or connection failures into panics.
- Frontend `package.json` points `svelte-check` at `jsconfig.json`, while the repository contains `tsconfig.json`.
- Frontend imports a non-existent `./types/search` path from `src/lib/api/memo.ts`.
- `editorStore.ts` uses `get(...)` without importing it.
- Vite rewrites `/api/v1/...` to `/v1/...`, while the backend routes are registered under `/api/v1/...`.
- The frontend update request omits the backend-required optimistic-locking `version` field.
- Navigation exposes routes and features that are not implemented, including `/memos`, `/memos/new`, profile/logout behavior, sharing, and notifications.
- Svelte 5 runes and legacy component/event patterns are mixed.
- Several Rust modules are empty placeholders and increase structural noise without providing behavior.
- There is no CI protecting backend compilation, frontend type checking, or application tests.

## 3. Scope

### 3.1 In scope

The revival release will provide:

1. A repeatable local development environment.
2. A backend that compiles and starts cleanly.
3. Working memo CRUD endpoints.
4. Working memo search.
5. Optimistic concurrency using memo version numbers.
6. A SvelteKit UI for memo listing, creation, editing, searching, and deletion.
7. Markdown editing and preview using the existing frontend direction.
8. Explicit loading, empty, success, error, unsaved, saving, and conflict states.
9. Responsive desktop/mobile navigation.
10. Automated backend and frontend quality gates in GitHub Actions.
11. Developer documentation and coding conventions reflecting the actual implementation.

### 3.2 Out of scope for this revival

The following historical-design items remain future work:

- OAuth/OIDC login
- MFA
- user registration/profile management
- RBAC/team management
- file attachments/object storage
- shared memos
- real-time collaboration
- CRDT
- WebSocket/WebRTC
- CQRS/Event Sourcing
- microservice decomposition
- API gateway
- webhook/integration system

UI controls for out-of-scope features must not appear active in the MVP.

## 4. Architectural direction

The backend remains a modular monolith with four meaningful layers:

```text
interfaces  -> application -> domain
      |             |
      +------> infrastructure
```

More precisely:

- `domain`: memo entity, invariants, repository contracts.
- `application`: memo use cases and DTO mapping.
- `infrastructure`: Scylla, Redis, Elasticsearch and repository implementations.
- `interfaces`: Actix HTTP routes, request parsing and response mapping.

The revival does not introduce microservices. Empty files that only suggest future use-case decomposition should be removed unless they immediately own behavior.

The frontend uses:

```text
route/page -> store/use-case state -> API client -> backend
        \-> feature/UI components
```

Direct HTTP requests from arbitrary components or stores should be eliminated. Network behavior belongs in `src/lib/api`.

## 5. Backend design

### 5.1 Application startup

Startup should:

1. Load configuration from environment variables.
2. Validate configuration.
3. Connect to required infrastructure with typed errors.
4. Construct repositories and services synchronously where possible.
5. Bind Actix.
6. Expose liveness and readiness separately where useful.

No `expect`/`unwrap` should be used for normal startup failure paths.

Development defaults may be provided, but they must be valid addresses for the corresponding clients.

### 5.2 Development identity

Authentication is out of scope, but CRUD operations still require a user boundary because the current domain model is user-scoped.

The MVP will therefore use a clearly named development identity:

```text
DEVELOPMENT_USER_ID=12345678-1234-1234-1234-123456789012
```

The value is loaded from configuration and injected into request handling. It must not be hard-coded repeatedly in handlers or described as authentication.

### 5.3 Memo model

The core memo shape is:

```text
id: UUID
title: string
content: string
tags: string[]
user_id: UUID
created_at: timestamp
updated_at: timestamp
version: integer
```

Domain validation:

- title must not be blank
- content may be empty while drafting if the UI supports that flow; title remains required for persistence
- at most 10 tags
- tags must be trimmed and non-empty
- duplicate tags should be normalized away
- update increments `version`

### 5.4 API contract

Base path remains `/api/v1`.

Endpoints:

```text
GET    /api/v1/health
GET    /api/v1/ready
GET    /api/v1/memos
POST   /api/v1/memos
GET    /api/v1/memos/{id}
PATCH  /api/v1/memos/{id}
DELETE /api/v1/memos/{id}
GET    /api/v1/memos/search?q=&tag=&page=&limit=
```

Create request:

```json
{
  "title": "Title",
  "content": "Markdown",
  "tags": ["tag"]
}
```

Update request:

```json
{
  "title": "Changed title",
  "content": "Changed Markdown",
  "tags": ["tag"],
  "version": 3
}
```

The returned memo includes `version`.

Status policy:

- create: `201`
- successful read/update/list/search: `200`
- delete: `204`
- malformed input: `400`
- domain validation failure: `422`
- unknown memo: `404`
- stale version: `409`
- development-user ownership violation: `404` rather than leaking existence across user boundaries
- unexpected infrastructure failure: `500`

Error bodies use one stable envelope:

```json
{
  "error": {
    "code": "memo_version_conflict",
    "message": "The memo changed since it was loaded."
  }
}
```

### 5.5 Search response

Search returns pagination metadata even if Elasticsearch is initially capped to a simple page/limit implementation:

```json
{
  "items": [],
  "page": 1,
  "limit": 20,
  "total": 0,
  "total_pages": 0
}
```

`page` and `limit` must actually be honored. The historical implementation accepted them but ignored them.

## 6. Persistence design

### 6.1 Scylla access patterns

The existing single table cannot safely support both current access patterns with the declared primary key.

Use access-pattern-specific tables:

```text
memos_by_id
  PRIMARY KEY (id)

memos_by_user
  PRIMARY KEY ((user_id), updated_at, id)
```

`memos_by_id` supports direct lookup, optimistic update, ownership validation and deletion.

`memos_by_user` supports user-scoped ordered listing.

Writes update both tables. Because Scylla denormalization duplicates data, repository code owns the consistency policy and tests cover partial failure behavior.

A logged batch should only be used when appropriate for the partition characteristics. The implementation must not use batches merely as a substitute for ordinary single statements.

### 6.2 Redis

Redis remains a read-through cache for direct memo lookup.

Rules:

- key: `memo:{id}`
- TTL is configurable
- cache failure should not make the primary memo operation unavailable when Scylla is healthy
- successful writes invalidate or replace cache entries
- successful deletes invalidate cache entries

Redis is an optimization, not the source of truth.

### 6.3 Elasticsearch

Elasticsearch remains the search projection.

Rules:

- index documents include `id`, `user_id`, title, content, tags, timestamps and version
- search is always filtered by `user_id`
- page/limit are translated to `from`/`size`
- write-to-Scylla success followed by Elasticsearch failure must be observable and recoverable

For the MVP, Scylla is authoritative. Elasticsearch inconsistency is treated as a degraded secondary-index failure rather than rolling back already-durable primary data.

The repository should emit an error/log that makes reindexing possible. A future outbox is preferable to pretending multi-system writes are atomic.

## 7. Frontend design

### 7.1 Svelte version style

Use one Svelte 5 style consistently:

- `$props()` for component properties
- `$state()` for local reactive state
- `$derived()` where derived values are appropriate
- modern event attributes (`onclick`, `oninput`, etc.)
- no new `export let` in runes-mode components

Legacy components touched by the revival should be migrated rather than increasing the mixture.

### 7.2 TypeScript

Keep strict TypeScript.

Canonical domain API types live in one location, for example:

```text
src/lib/api/types.ts
```

`Memo` includes `version`.

Search types use a valid import path and should not be duplicated across unrelated folders.

Browser timer types use browser-compatible typing rather than `NodeJS.Timeout`.

### 7.3 Routes

Required routes:

```text
/                  -> dashboard/recent memos
/memos             -> memo list
/memos/new         -> new memo editor
/memos/[id]/edit   -> existing memo editor
/memos/search      -> search results
```

Unsupported profile/shared/settings actions should either be removed from primary navigation or represented as explicitly disabled/future functionality. No dead navigation links should remain.

### 7.4 API access

Frontend API requests remain same-origin `/api/v1/...` in development.

Vite proxies `/api` to the backend **without stripping `/api`**.

The API client is responsible for:

- method/path
- JSON serialization
- response decoding
- stable typed frontend errors
- conflict detection

Stores/components should not repeat low-level response handling.

### 7.5 Editor UX

The memo editor exposes visible persistence state:

```text
Unsaved -> Saving -> Saved
             |
             +-> Error
             +-> Conflict
```

Requirements:

- title
- markdown body
- tags
- preview toggle or split preview, depending on viewport
- manual save
- Cmd+S / Ctrl+S
- autosave after a short debounce when content is dirty
- local draft recovery for a new memo
- server `version` preserved and included on update
- conflict response does not silently overwrite server content
- navigation warning while dirty

The existing 30-second interval autosave should be replaced by dirty-state-aware debounce logic so a memo is not needlessly saved on a fixed timer.

### 7.6 List/search UX

Memo lists show:

- title
- short content excerpt
- tags
- last-updated timestamp

States are explicit:

- skeleton/loading
- empty list
- error with retry
- populated list

Search supports text and tag filters and preserves them in the URL.

### 7.7 Accessibility

Minimum acceptance:

- visible keyboard focus
- semantic buttons/links
- accessible form labels
- navigation controls with accessible names
- color is not the only error/status signal
- mobile sidebar can be closed and does not trap inaccessible content

## 8. Docker and local development

`docker compose up --build` must start a usable development stack rather than shell containers.

Expected services:

```text
frontend
backend
scylla
redis
elasticsearch
```

Kibana becomes optional through a Compose profile.

Add health checks where supported and use service dependencies based on health rather than arbitrary sleeps.

Expected host ports should be documented and internally consistent. The frontend proxy must target the backend service address from inside Compose.

## 9. Coding conventions

### Rust

Required quality gates:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo check
```

Conventions:

- `rustfmt` is authoritative formatting
- no routine `unwrap`/`expect` in application/infrastructure request paths
- typed errors at boundaries
- domain behavior belongs in domain types
- infrastructure-specific types do not leak into the domain API without need
- remove unused imports and dead placeholder modules
- prefer straightforward code over speculative abstractions

### Frontend

Required quality gates:

```text
pnpm install --frozen-lockfile
pnpm check
pnpm lint
pnpm test:unit -- --run
pnpm build
```

Conventions:

- Prettier is authoritative formatting
- ESLint errors fail CI
- strict TypeScript remains enabled
- avoid `any`; when unavoidable at external boundaries, isolate and document it
- network access stays in the API layer
- component files should remain focused; extract behavior when a file owns unrelated concerns
- remove debugging/tutorial comments from production components

## 10. Test strategy

### 10.1 Backend unit tests

Cover:

- memo creation invariants
- tag normalization/validation
- update version increment
- stale-version rejection
- ownership boundaries
- application service behavior using a fake repository

### 10.2 Backend integration tests

Cover at least:

- health endpoint
- create -> get
- create -> list
- create -> update
- stale update -> 409
- create -> delete -> 404
- search by text
- search by tag

Infrastructure-specific tests may use Compose services and should be separated from fast unit tests.

### 10.3 Frontend tests

Unit/component tests cover:

- API error mapping
- editor save payload contains version
- dirty/saving/saved/conflict state transitions
- list empty/error states

Playwright smoke flow:

```text
open app
create memo
see memo in list
edit memo
search memo
delete memo
confirm empty/not-found state
```

## 11. CI design

Add GitHub Actions with backend and frontend jobs that can run independently.

Fast static/unit checks run for pull requests and pushes to the main development branches.

A heavier integration job may start the backing services only where needed.

No PR is considered ready to merge while required checks are red.

## 12. Documentation

Add or refresh:

- root `README.md`
- `.env.example`
- `CONTRIBUTING.md`
- architecture notes describing the implemented modular monolith, not the historical future-state microservice plan
- local development commands
- quality commands
- API summary
- explicit future-work list

The historical Japanese design documents may remain for reference, but README/architecture docs must distinguish aspirational features from implemented ones.

## 13. Delivery sequence

Implementation is split into reviewable phases:

1. Baseline quality gates and deterministic startup.
2. Backend compilation and configuration cleanup.
3. Scylla access-pattern correction and repository tests.
4. API contract, validation, pagination and conflict semantics.
5. Frontend type/build repair and API-layer normalization.
6. Required route completion and editor/list/search UX.
7. Compose end-to-end startup and smoke tests.
8. Documentation cleanup and final quality review.

Each phase should leave the branch no worse than before and should include tests for behavior it introduces or repairs.

## 14. Acceptance criteria

The revival is complete when all of the following are true:

1. A fresh checkout can start the required services with documented commands.
2. `docker compose up --build` starts the application rather than idle shells.
3. Backend format, clippy, tests and check pass.
4. Frontend check, lint, unit tests and build pass.
5. The browser can create, list, retrieve, update, search and delete memos.
6. Update requests implement optimistic concurrency and expose conflicts instead of overwriting silently.
7. Data survives application container restart.
8. Search is user-scoped and supports text/tag filters.
9. No primary navigation item leads to an unimplemented route presented as working.
10. Loading, empty, error, saving, saved, dirty and conflict states are visible where applicable.
11. A Playwright smoke flow covers the primary memo lifecycle.
12. GitHub Actions protect the same critical quality gates used locally.
13. README and development documentation match the actual repository behavior.

## 15. Key risks and mitigations

### Scylla driver/API drift

The repository was last developed against older crate APIs. Compile failures may require targeted migration to supported APIs.

Mitigation: make backend compile/check the first implementation gate and avoid unrelated upgrades until behavior is stable.

### Multi-store consistency

Scylla, Redis and Elasticsearch cannot be made transactionally atomic through the current repository pattern.

Mitigation: treat Scylla as source of truth, Redis as disposable cache, Elasticsearch as rebuildable projection; log/index failures clearly. Consider an outbox in a later phase.

### Scope expansion

Historical documentation describes a much larger collaborative application.

Mitigation: keep OAuth, attachments, sharing, CRDT and microservices out of this revival PR series.

### UI migration churn

Svelte 5 migration can become a broad rewrite.

Mitigation: migrate components that participate in MVP flows first and delete/disable dead feature surfaces rather than modernizing unused features.

## 16. Implementation principle

Restore correctness before adding sophistication:

```text
compile -> test -> start -> CRUD -> search -> UX -> harden
```

The revival should prefer a small, demonstrably working system over retaining unfinished abstractions simply because they appear in the historical design.