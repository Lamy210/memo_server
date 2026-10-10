# Memo list pagination v1

This document defines the public list contract and rollout constraints for issue #135.

## Public cursor contract

Use the versioned list mode for every client:

```http
GET /api/v1/memos?pagination=cursor-v1
GET /api/v1/memos?pagination=cursor-v1&limit=50
GET /api/v1/memos?pagination=cursor-v1&limit=50&cursor=v1.550e8400-e29b-41d4-a716-446655440003
```

The response shape is:

```json
{
  "pagination": "cursor-v1",
  "items": [],
  "limit": 20,
  "next_cursor": null
}
```

`limit` defaults to 20 and must be between 1 and 100 inclusive. `next_cursor` is non-null only when another page is available. Clients must treat the cursor as opaque and send it back unchanged.

A `cursor-v1` value has the form `v1.<canonical-lowercase-UUIDv4>`. Uppercase/non-canonical UUID text, UUID versions other than v4, unknown cursor versions, control characters, and trailing data are rejected before authoritative storage access.

## Ordering and page boundaries

`cursor-v1` orders memo IDs by canonical UUIDv4 descending order. The cursor is exclusive: a subsequent page contains only IDs lower than the cursor ID. This gives ScyllaDB, plaintext MongoDB, and encrypted MongoDB the same deterministic page boundary without adding a mutable sort key.

The normal list path does not provide an exact total count. Each authoritative-store page is physically bounded to at most `limit + 1` rows/envelopes so the service can determine `has_more` without materializing the owner's complete memo set.

Memo IDs are immutable, so updates do not move an already-seen item across cursor boundaries. Pagination is not snapshot-consistent across concurrent inserts: a memo inserted after page 1 may fall before or after the current cursor and therefore may require a fresh traversal to observe. Clients that need a refreshed view should restart from the first page rather than reuse an old cursor indefinitely.

## Required versioned list contract

`pagination=cursor-v1` is required for every memo-list request. An unversioned request:

```http
GET /api/v1/memos
```

fails with `400 Bad Request` before the memo-list service or authoritative storage is called. The stable error guidance is:

```text
Memo list pagination requires pagination=cursor-v1
```

Supplying `cursor` or `limit` without `pagination=cursor-v1` is rejected by the same contract. The historical complete-array response is no longer exposed by the HTTP API.

## HIGH encrypted route

MEMO-HIGH-1 does not add plaintext `updated_at`, another plaintext timestamp, or any new plaintext sort metadata to make paging possible. Encrypted MongoDB uses the existing owner partition plus memo `_id` as the keyset:

- owner predicate is always present;
- `_id` is sorted descending;
- subsequent pages add `_id < cursor`;
- the database reads at most `limit + 1` encrypted envelopes;
- only the logical page envelopes are decrypted; the extra probe envelope used to determine `has_more` is not decrypted.

This intentionally changes the versioned list ordering from legacy recency order to UUIDv4-descending order in exchange for a bounded query that preserves the HIGH metadata boundary.

## Store and route-cutover consistency

All authoritative routes must interpret the same `cursor-v1` cursor identically:

| Route/store | Keyset |
| --- | --- |
| ScyllaDB migration fallback | `(user_id)` partition, `id DESC` |
| Plaintext MongoDB | `{ user_id: 1, _id: -1 }` |
| Encrypted MongoDB | `{ owner_partition: 1, _id: -1 }` |

A cutover between plaintext and encrypted authoritative routes must not change page IDs, exclusivity, `has_more`, or the next-page boundary for the same owner and immutable memo-ID set. Cross-route MongoDB integration coverage compares the plaintext and encrypted paths using the same UUIDv4 IDs.

The ScyllaDB fallback uses the same UUID comparator contract. Introducing another memo-ID UUID version requires a pagination-contract review; it must not silently inherit `cursor-v1` semantics.

## BFF and frontend

The SvelteKit BFF allows only `pagination`, `cursor`, and `limit` on the normal memo-list route, rejects duplicate/unknown query keys, and leaves cursor-value validation to the backend. Existing authentication, CSRF, request-size, deadline, response-header allowlist, identity-encoding, and `Cache-Control: no-store` controls remain unchanged.

The first-party `/memos` screen uses `cursor-v1`, defaults to 20 items, and exposes an explicit `さらに読み込む` control when `next_cursor` is present. Additional pages append to the existing list and are defensively deduplicated by immutable memo ID. A continuation failure does not clear already loaded items.

## Rollout and rollback

Before merging or enabling this behavior in production:

1. Require backend, frontend, browser E2E, Compose/full-stack smoke, and any path-matched frontend quality checks to pass on the latest PR head.
2. Confirm normal list request paths do not call all-owner materialization methods.
3. Confirm plaintext and encrypted route tests return the same cursor boundaries.
4. Confirm encrypted storage schema/index changes contain no plaintext timestamp or sort metadata.
5. Deploy backend/BFF support before depending on cursor pagination from separately deployed clients, if those components are released independently.

Rollback must preserve the versioned cursor contract across backend and frontend. Do not reintroduce the unversioned complete-array HTTP route as a partial rollback; revert the API/client pagination change together if `cursor-v1` itself must be rolled back.
