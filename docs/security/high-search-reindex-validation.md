# Staged HIGH Search Reindex Validation Runbook

Status: operator-only staging command. SEARCH-HIGH-1 request routing remains inactive.

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

The command uses the same MongoDB maintenance barrier as memo mutations, clears the derived-key cache, resets the isolated `memos_high_v1` protected projection, performs a bounded full rebuild, verifies source/projection convergence, revalidates the maintenance permit, and explicitly releases it. Resetting the projection is necessary because normal HIGH projection mutation wiring is not installed yet; without it, a memo deleted after an earlier staged run could otherwise remain as a stale protected row.

It does **not** install SEARCH-HIGH-1 into the HTTP request path and does not perform a production routing cutover.

## Preconditions

Before `--apply`:

1. Deploy only application versions that use the shared MongoDB memo-mutation barrier for create/update/delete.
2. Confirm `AUTHORITATIVE_BACKEND=mongodb`.
3. Confirm `SEARCH_BACKEND=manticore`.
4. Set `HIGH_SEARCH_MODE=aws-kms` and all required HIGH search cache/analysis settings.
5. Build the command with the `aws-kms-search` feature.
6. Ensure the AWS identity has the documented `kms:DescribeKey` and `kms:GenerateMac` permissions for the pinned HMAC_384 key.
7. Confirm protected HIGH search request routing is still inactive.
8. Have the maintenance recovery runbook available in case the operator process is cancelled after closing the barrier.

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
  --confirm-all-writers-guarded \
  --confirm-request-path-inactive
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
- Projection reset occurs only after the barrier is held and only while protected request routing is operator-confirmed inactive.
- If reset succeeds but rebuild later fails, the protected projection may be incomplete; request routing remains inactive and the command must be rerun after resolving the failure.
- New memo mutations are rejected while the barrier is closed.
- Existing mutation leases must drain before reindex begins.
- KMS, analyzer, projection, or convergence failures keep the operation failed.
- Rotation orchestration clears cached target-generation keys on reindex failure.
- Barrier release is explicit and awaited.
- If cancellation or release failure leaves state closed, use `docs/security/high-search-maintenance-recovery.md`; do not manually edit MongoDB collections.

## Non-goals

This staging command does not establish that SEARCH-HIGH-1 is ready for user traffic. Analyzer corpus approval, encrypted authoritative-store request-path integration, protected-search request routing, rollback rehearsal, and other remaining migration gates still apply.
