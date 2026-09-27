# Staged HIGH Search Reindex Validation Runbook

Status: operator-only staging command. Route-aware request handling is installed, but the shared route must remain `legacy` for this destructive rebuild.

## Purpose

The staged reindex command exercises the production-shaped protected-search path without switching user search traffic:

```text
AWS KMS DescribeKey / GenerateMac
        |
        v
owner-scoped HKDF search keys
        |
        v
blind-token projection
        |
        v
memos_high_v1 in Manticore
```

The command uses the same MongoDB maintenance barrier as memo mutations, background projection reconciliation, and protected HIGH shadow queries, clears the derived-key cache, resets the isolated `memos_high_v1` protected projection, performs a bounded full rebuild, verifies source/projection convergence, revalidates the maintenance permit, and explicitly releases it. The normal outbox/reconciler now mirrors HIGH projection mutations when the runtime is enabled; the explicit reset remains part of staged full-rebuild validation so the operator starts from a known empty protected generation while the shared barrier excludes foreground and background writers.

It does **not** switch the shared query route. The HTTP request path may be route-aware, but this command requires the drained route to be `legacy` and leaves it unchanged.

## Preconditions

Before `--apply`:

1. Deploy only application versions that use the shared MongoDB memo-mutation barrier for create/update/delete.
2. Confirm `AUTHORITATIVE_BACKEND=mongodb`.
3. Confirm `SEARCH_BACKEND=manticore`.
4. Set `HIGH_SEARCH_MODE=aws-kms` and all required HIGH search cache/analysis settings.
5. Build the command with the `aws-kms-search` feature.
6. Ensure the AWS identity has the documented `kms:DescribeKey` and `kms:GenerateMac` permissions for the pinned HMAC_384 key.
7. Have the maintenance recovery runbook available in case the operator process is cancelled after closing the barrier.

The command no longer accepts a self-asserted "request path inactive" flag. After the maintenance barrier has drained all writer/query leases, it reads the shared query-route snapshot from the live permit and requires `route=legacy` before the protected projection reset can begin.

The command intentionally does not accept a claim that mixed old/new application replicas are safe. If any running writer can bypass the shared barrier, do not run the reindex.

## Plan

Plan mode validates local configuration only. It starts no KMS, MongoDB, or Manticore network operation.

From `backend/`:

```bash
cargo run --locked --features aws-kms-search \
  --bin reindex_high_search_staged -- \
  --plan \
  --page-size 250
```

## Apply

```bash
cargo run --locked --features aws-kms-search \
  --bin reindex_high_search_staged -- \
  --apply \
  --page-size 250 \
  --confirm-all-writers-guarded
```

The page size must be within the repository migration bound (1..=1000).

## Success criteria

A successful run reports equal:

- authoritative source count,
- protected projection count,
- projected visited count,
- verified visited count.

The maintenance barrier is released only after reindex convergence and permit revalidation succeed. Protected request routing remains unchanged.

## Failure behavior

- Invalid page size or disabled/incompatible configuration fails before provider/database work.
- Failure to acquire the maintenance barrier prevents projection reset or reindex from starting.
- Projection reset occurs only after the barrier is held, all writer/query leases have drained, and the shared query route is mechanically verified as `legacy`.
- If the shared route is `protected`, the operation fails before cache clear, projection reset, or reindex. Roll back the route through the guarded cutover procedure before attempting a destructive staged rebuild.
- If reset succeeds but rebuild later fails, the protected projection may be incomplete; the route remains `legacy` and the command must be rerun after resolving the failure.
- New memo mutations are rejected while the barrier is closed.
- Existing mutation/reconciliation writer leases and protected-query leases must drain before reindex begins.
- New protected HIGH shadow queries are rejected while the barrier is closed, so reset/rebuild cannot race an in-flight protected read.
- KMS, analyzer, projection, or convergence failures keep the operation failed.
- Rotation orchestration clears cached target-generation keys on reindex failure.
- Barrier release is explicit and awaited.
- If cancellation or release failure leaves state closed, use `docs/security/high-search-maintenance-recovery.md`; do not manually edit MongoDB collections.

## Non-goals

This staging command does not establish that SEARCH-HIGH-1 is ready for protected user traffic. Production workload approval, deployment/KMS review, a guarded shared-route cutover procedure, rollback rehearsal, and other remaining migration gates still apply.
