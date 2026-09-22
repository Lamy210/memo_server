# ScyllaDB -> MongoDB Backfill Runbook

Status: migration tooling available; production cutover remains an operator-controlled action.

## Purpose

This runbook moves existing memo rows from the legacy ScyllaDB authoritative store into MongoDB while preserving:

- memo ID
- owner/user ID
- title/content/tags
- created/updated timestamps at the stores' millisecond precision
- optimistic-concurrency version

Every newly imported memo is committed together with a MongoDB projection intent for its current version. This lets the normal reconciler repopulate rebuildable secondary stores after MongoDB becomes authoritative.

The tool does **not** delete or mutate source memo rows.

## Safety model

The backfill intentionally fails closed.

- Running without arguments is a dry run and only counts source rows.
- Writes require an explicit `--apply`.
- `SCYLLA_URI` must be explicitly configured.
- `MONGODB_URI` and `MONGODB_DATABASE` must be explicitly configured for `--apply`.
- MongoDB must be a replica set or sharded cluster because the memo row and projection intent are committed atomically.
- Re-running against an identical destination row is idempotent and reports it as already present.
- If the destination already contains the same memo ID with different data, the migration stops with a conflict instead of overwriting it.

Do not run multiple backfill writers concurrently.

## Consistency requirement

The current migration is an offline/frozen-write cutover primitive, not dual-write replication.

Before the final `--apply` pass:

1. Stop memo writes or place the application in maintenance mode.
2. Wait for in-flight source mutations to finish.
3. Keep ScyllaDB available and unchanged until the MongoDB cutover has been verified.

Running an initial copy while writes are still active is acceptable only for rehearsal. Because an already-imported row is never overwritten automatically, the final authoritative backfill must run from a write-frozen source.

## 1. Backup

Take a verified ScyllaDB backup/snapshot before migration. Keep the old deployment configuration and data until post-cutover verification is complete.

## 2. Dry run

From `backend/`:

```bash
export SCYLLA_URI='scylla-host:9042'

cargo run --locked --bin backfill_mongodb
```

The command reports the number of source memos discovered. Scylla reads use driver-managed paging, so the migration does not require loading the full table into memory at once.

## 3. Prepare MongoDB

Use a new/empty MongoDB database whenever possible.

For local rehearsal, MongoDB still needs transaction support; the project Compose configuration provides a single-node replica set for this purpose. Production should use a redundant replica set or sharded cluster.

Set the destination explicitly:

```bash
export MONGODB_URI='mongodb://mongo1.example:27017,mongo2.example:27017/?replicaSet=rs0'
export MONGODB_DATABASE='memo_app'
```

## 4. Freeze writes and apply

After writes are frozen:

```bash
cargo run --locked --bin backfill_mongodb -- --apply
```

A successful result prints:

```text
Backfill complete: visited=<n> inserted=<n> already_present=<n>
```

The command aborts on the first source/destination conflict.

## 5. Secondary projections

Valkey is disposable and Manticore is rebuildable. Do not treat their existing contents as authoritative migration state.

Before or during cutover, either:

- start with isolated/empty Valkey and Manticore instances, or
- explicitly clear/rebuild the old projections.

Each imported MongoDB memo has a current-version projection intent, so the normal reconciler can repopulate present memo state after cutover. Starting from a clean projection also prevents stale search/cache entries for memos that were deleted before the migration.

## 6. Cut over

Only after the backfill has completed successfully:

```bash
export AUTHORITATIVE_BACKEND='mongodb'
```

Start the application with the same `MONGODB_URI` and `MONGODB_DATABASE`, then verify:

- `/api/v1/health/ready` reports `checks.authoritative == "ok"`
- representative users can list and open their memos
- versions and timestamps are preserved
- create/update/delete operations work
- Manticore search converges
- Valkey/Manticore outages still leave core CRUD available

## Rollback

Do not delete ScyllaDB during the first MongoDB release window.

If verification fails before new writes are accepted on MongoDB, switch `AUTHORITATIVE_BACKEND` back to `scylla` and restore the prior secondary-store topology.

If MongoDB has already accepted new writes, do **not** blindly switch back: ScyllaDB will no longer contain those mutations. At that point rollback requires an explicit reverse migration or restoration plan.

## Known boundary

This tool migrates memo rows, not a live change stream. It deliberately avoids automatic overwrite/merge behavior because silently reconciling concurrent source and destination writes could lose data. A future online migration would need dual-write or change-data-capture semantics plus a verified convergence protocol.
