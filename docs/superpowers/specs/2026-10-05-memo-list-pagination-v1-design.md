# Memo List Pagination v1 Design

**Date:** 2026-10-05  
**Tracks:** issue #135  
**Status:** implementation design

## Goal

Bound every normal `GET /api/v1/memos` request by an explicit server-side page size without weakening the MEMO-HIGH-1 encrypted authoritative-store boundary.

The design must stop one request from materializing an owner's complete memo set in ScyllaDB, plaintext MongoDB, encrypted MongoDB, memo_server, the SvelteKit BFF, or the browser.

## Current problem

The current list path is unbounded:

`GET /api/v1/memos` -> `MemoService::get_user_memos` -> `MemoRepository::find_all_by_user_id` -> `MemoAuthoritativeStore::find_all_by_user_id` -> authoritative store -> JSON array -> BFF -> browser.

The service then sorts the complete result by `updated_at DESC`.

That ordering is inexpensive for plaintext MongoDB because `updated_at_ms` is stored and indexed, but it is intentionally unavailable to the encrypted route. `EncryptedMemoDocument` keeps timestamps inside ciphertext, so `HighMemoAuthoritativeAdapter` currently has to decrypt every owner envelope before it can sort by `updated_at`.

Adding plaintext `updated_at` to `memos_encrypted_v1` merely to retain the existing list order is explicitly out of scope: it would weaken the encrypted-storage metadata boundary for a presentation concern.

## Chosen approach

Introduce a versioned cursor contract named **`cursor-v1`** whose stable ordering is memo ID descending.

Memo IDs are currently created with `Uuid::new_v4()`. For `cursor-v1`, memo IDs are therefore part of the pagination contract:

- accepted memo IDs and cursors are canonical UUIDv4 values;
- ordering is descending by the UUID's 128-bit byte value;
- MongoDB stores canonical lowercase UUID text in `_id`, whose fixed-width lexical order matches the UUID byte order;
- ScyllaDB already stores `id uuid` as the clustering column and declares `CLUSTERING ORDER BY (id DESC)`;
- encrypted MongoDB already exposes memo ID and owner partition as envelope metadata, so this adds **no new plaintext sort metadata**.

This deliberately changes list ordering from `updated_at DESC` to stable UUIDv4-descending order for the new paginated API. The trade-off is accepted for v1 because bounded authoritative reads and the existing HIGH metadata boundary take precedence over preserving recency ordering.

A future pagination version may introduce a reviewed ordering projection or protected sort metadata, but it must not silently change `cursor-v1` semantics.

## Why not the alternatives

### Keep `updated_at DESC` by adding plaintext timestamp metadata

Rejected. It weakens MEMO-HIGH-1 by exposing update timing that is currently encrypted.

### Use the search projection as the list source

Rejected. Redis/Manticore/search projections are rebuildable secondary data and may be degraded while core CRUD remains available. The normal memo list must remain an authoritative-data operation.

### Offset/page pagination

Rejected for the primary contract. Offset pagination requires store-specific skip semantics, becomes increasingly expensive at high offsets, and is more sensitive to concurrent inserts/deletes. Keyset pagination is bounded and maps cleanly to all three authoritative routes.

### Opaque per-user recency sequence

Deferred. A per-user monotonic sequence could preserve relative update order without a plaintext timestamp, but it adds new authoritative metadata, transactional counter state, migration work, leakage of relative update activity, and a larger failure surface. That design should be evaluated only if product requirements later make recency ordering mandatory.

## Public HTTP contract

### New paginated request

```text
GET /api/v1/memos?pagination=cursor-v1&limit=20
GET /api/v1/memos?pagination=cursor-v1&limit=20&cursor=v1.550e8400-e29b-41d4-a716-446655440000
```

Rules:

- `pagination` must equal `cursor-v1` when paginated query parameters are present.
- `limit` is optional and defaults to **20**.
- `limit` must be in **1..=100**. Values outside the range are rejected; they are not silently clamped.
- `cursor` is optional for the first page.
- `cursor` format is exactly `v1.<canonical-lowercase-uuid-v4>`.
- duplicate or unknown query parameters remain rejected by the BFF.
- malformed/unsupported cursor or pagination versions return a client error before repository access.

### New paginated response

```json
{
  "pagination": "cursor-v1",
  "items": [],
  "limit": 20,
  "next_cursor": null
}
```

`next_cursor` is non-null only when another item exists beyond the returned page. No total count is returned; obtaining an exact total must not require an unbounded count/read on the normal request path.

### Cursor semantics

For a request with cursor `v1.<id>`, stores return memo IDs strictly **less than** `<id>` under the `cursor-v1` UUID ordering.

Each store requests at most `limit + 1` records. If the extra record exists:

1. return only the first `limit` items;
2. set `next_cursor` from the last returned item's ID.

If no extra record exists, `next_cursor` is `null`.

The maximum physical read for one list page is therefore **101 memos/envelopes**.

## Legacy array compatibility

The unversioned request:

```text
GET /api/v1/memos
```

remains temporarily supported so existing clients do not receive a silently truncated array.

Its implementation is also bounded:

1. read at most 101 memos through the same bounded repository primitive;
2. if at most 100 exist, the service has the complete owner set and may preserve the legacy `updated_at DESC` array response exactly;
3. if a 101st memo exists, return `400 Bad Request` with a stable migration message instructing the client to use `pagination=cursor-v1`.

This compatibility path prevents a breaking shape change for small existing clients while ensuring no legacy request can materialize an unbounded set.

The first-party frontend moves to `cursor-v1` immediately. Removal of the legacy array form is a separate future API-deprecation decision.

## Domain and repository contracts

Replace request-path use of unbounded `find_all_by_user_id` with a bounded page primitive.

Recommended domain shape:

```rust
#[derive(Debug)]
pub struct MemoListPage {
    pub items: Vec<Memo>,
    pub has_more: bool,
}

async fn list_page_by_user_id(
    &self,
    user_id: Uuid,
    after: Option<Uuid>,
    limit: usize,
) -> AppResult<MemoListPage>;
```

Contract:

- `limit` is the logical page size and must already be `1..=100` when entering the repository.
- repository/store implementations may internally request exactly `limit + 1`.
- returned `items` contain at most `limit` memos in UUID-descending order.
- `has_more` says whether the extra row existed.
- `after` is exclusive.
- no request-path implementation may fall back to `find_all_by_user_id`.

The unbounded repository/store methods should be removed from request-path interfaces once callers are migrated. Operational migration/integrity traversals retain their separate bounded paging APIs.

## Store-specific implementation

### ScyllaDB legacy route

The existing schema already uses:

```sql
PRIMARY KEY ((user_id), id)
WITH CLUSTERING ORDER BY (id DESC)
```

Prepare two bounded statements:

- first page: owner partition, descending clustering order, `LIMIT ?`;
- later page: owner partition plus exclusive `id < ?`, `LIMIT ?`.

The request limit is `page_size + 1` and never exceeds 101.

### Plaintext MongoDB route

Use owner + `_id` keyset pagination:

- filter `{ user_id: owner }`;
- when a cursor exists, add `{ _id: { $lt: cursor } }`;
- sort `{ _id: -1 }`;
- limit to `page_size + 1`.

Add/ensure a compound `{ user_id: 1, _id: -1 }` index. The existing `{ user_id: 1, updated_at_ms: -1 }` index may remain because other/migration behavior does not need to change in this task.

### Encrypted MongoDB / MEMO-HIGH-1 route

Add a bounded envelope method:

```rust
async fn page_envelopes_by_owner(
    &self,
    owner_partition: Uuid,
    after: Option<Uuid>,
    limit: usize,
) -> AppResult<Vec<HighEncryptedMemoEnvelope>>;
```

MongoDB implementation:

- filter `{ owner_partition: owner }`;
- optional `_id < cursor`;
- sort `_id: -1`;
- limit to at most 101;
- ensure compound `{ owner_partition: 1, _id: -1 }` index.

`HighMemoAuthoritativeAdapter` decrypts only the bounded envelope page. It must never call `find_all_envelopes_by_owner` for the normal list route.

No timestamp, title, tag, content, or other new memo metadata is exposed outside ciphertext.

## Route cutover compatibility

`cursor-v1` is defined above the storage-route choice. `MemoService` validates the cursor once, then sends the UUID keyset to whichever `HighMemoDataRoute` is active.

Both `LegacyPlaintext` and `Encrypted` must therefore return the same UUID-descending order and cursor boundary for the same owner set. Switching routes must not reinterpret an existing `cursor-v1` token.

Migration/integrity tests must include a page boundary that produces the same IDs from plaintext MongoDB and encrypted MongoDB.

## Concurrent mutation semantics

`cursor-v1` is keyset pagination over immutable memo IDs, not a snapshot transaction.

- updates do not move existing memos between pages;
- deletes may reduce later page sizes;
- a memo created after page 1 may sort before the current cursor and therefore appear only after a refresh, or sort after it and appear on a later page;
- duplicates caused solely by updates are avoided because the key is immutable.

The frontend provides refresh/new navigation rather than promising snapshot-consistent traversal.

## Frontend behavior

Replace `fetchMemos(): Promise<Memo[]>` with a paginated client call, for example:

```ts
interface MemoListPage {
  pagination: 'cursor-v1';
  items: Memo[];
  limit: number;
  next_cursor: string | null;
}

fetchMemosPage({ cursor?, limit? }): Promise<MemoListPage>
```

List page behavior:

- initial page size: 20;
- append subsequent pages to the existing grid;
- show `さらに読み込む` only when `next_cursor` is non-null;
- while loading another page, disable the control and expose an accessible loading state;
- preserve the current explicit authentication-required handling;
- a refresh restarts from the first cursor page.

No infinite-scroll observer is added in v1; an explicit load-more control is simpler, testable, and avoids background request amplification.

## BFF boundary

The existing query allowlist currently permits query parameters only for `memos/search`.

Extend it so exact path `memos` permits only:

- `pagination`
- `limit`
- `cursor`

Unknown and duplicate parameters remain rejected locally. `memos/:id` continues to permit no query parameters.

The BFF does not parse cursor semantics beyond the query-key boundary; memo_server remains authoritative for pagination value validation.

## API migration and documentation

Document:

- old array compatibility and its 100-memo ceiling;
- `cursor-v1` request/response examples;
- page-size maximum;
- stable UUID-descending ordering;
- concurrent create/delete semantics;
- ordering change from legacy `updated_at DESC`;
- HIGH profile metadata rationale;
- no guarantee that future cursor versions share ordering with v1.

## Security and reliability properties

- authorization remains owner-scoped before/inside every store query;
- cursor values never select another owner's partition;
- one normal list request reads/decrypts at most 101 authoritative records;
- no new plaintext memo metadata is introduced in MEMO-HIGH-1;
- no secondary search/cache dependency becomes authoritative;
- malformed cursors fail before storage access;
- the BFF still rejects unsupported/duplicate query surfaces;
- browser response cache remains `no-store`.

## Test requirements

### Cursor/domain tests

- accepts canonical lowercase UUIDv4 cursor;
- rejects missing prefix, unsupported version, uppercase/non-canonical UUID text, non-v4 UUID, controls, and trailing data;
- rejects limit 0 and >100;
- first page has no cursor.

### Repository/store tests

For every authoritative route:

- returns at most requested page size;
- detects `has_more` using only one extra record;
- owner isolation holds;
- cursor is exclusive;
- order is UUID-descending;
- page 1 + page 2 has no duplicate IDs;
- encrypted route decrypts no more than `limit + 1` envelopes.

Integration coverage must exercise ScyllaDB and MongoDB with enough records to cross a page boundary.

### REST/service tests

- `pagination=cursor-v1` returns versioned response shape;
- malformed cursor/limit returns client error;
- legacy list with <=100 memos keeps array shape and `updated_at DESC`;
- legacy list with >100 memos returns explicit migration error and does not read the complete set;
- route cutover produces identical page IDs/cursors.

### Frontend/BFF tests

- list query allowlist accepts the three v1 keys and rejects unknown/duplicates;
- API client serializes cursor-v1 request and response type;
- list initially renders page 1;
- load-more appends page 2;
- button disappears at `next_cursor=null`;
- unauthorized page/load-more behavior preserves the current auth-required state;
- Browser E2E crosses at least two pages through the real SvelteKit BFF.

## Rollout

1. Land the contract and bounded store primitives.
2. Land server `cursor-v1` plus bounded legacy compatibility.
3. Migrate the first-party frontend to cursor-v1 in the same implementation series before considering issue #135 complete.
4. Keep the legacy array form only as a bounded compatibility bridge.
5. A future deprecation PR may remove the legacy array form after external clients, if any, are migrated.

## Non-goals

- preserving `updated_at DESC` in cursor-v1;
- adding plaintext sort timestamps to encrypted storage;
- making search infrastructure authoritative for normal list reads;
- exact total counts;
- infinite scrolling;
- changing memo IDs away from UUIDv4 inside this pagination version;
- implementing the dedicated authentication service or frontend auth-session integration.
