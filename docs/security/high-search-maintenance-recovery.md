# HIGH Search Maintenance Recovery Runbook

Status: break-glass operator tooling; no automatic lease expiry.

## Purpose

The MongoDB HIGH search maintenance barrier intentionally fails closed. A cancelled memo mutation/background reconciliation can leave a writer lease behind, a cancelled protected query can leave a query lease behind, and an interrupted rotation can leave the shared maintenance barrier closed. The recovery command exists to restore service only after an operator has independently established that the abandoned work is no longer running.

Do not use this command as a normal rotation or deployment step.

## Safety prerequisites

Before any `--apply` recovery:

1. Stop every memo_server application replica and any operator job that can acquire memo mutation or protected-query leases.
2. Verify those processes are no longer running.
3. Keep MongoDB available as the authoritative recovery source.
4. Run status inspection and record the reported mode, writer epoch (the legacy-named shared activity generation), active writer/query lease counts, and holder token.
5. Do not reuse an older snapshot after any application process has restarted.

Recovery never relies on lease age or a timeout. If the operator cannot establish that all writers and protected-query workers are stopped, leave the state fail-closed.

## Inspect

From `backend/`:

```bash
MONGODB_URI='mongodb://...' \
MONGODB_DATABASE='memo_app' \
cargo run --locked --bin recover_high_search_maintenance -- --status
```

With no arguments, the command is also status-only.

Example fields:

```text
current.mode=maintenance
current.writer_epoch=42
current.active_writer_leases=0
current.active_query_leases=0
current.holder_token=...
```

The holder token is a compare-and-swap recovery value, not an authentication credential. Treat it as operationally sensitive and avoid copying it into persistent logs or tickets unless required.

## Recover an abandoned maintenance barrier

If status reports `mode=maintenance`, recovery requires both the exact writer epoch and holder token observed immediately before recovery:

```bash
MONGODB_URI='mongodb://...' \
MONGODB_DATABASE='memo_app' \
cargo run --locked --bin recover_high_search_maintenance -- \
  --apply \
  --confirm-app-stopped \
  --expected-writer-epoch 42 \
  --expected-holder-token '<observed-holder-token>'
```

The command transactionally clears stale writer and protected-query leases, changes the gate back to `open`, removes the holder token, and increments the legacy-named `writer_epoch` activity generation.

## Recover abandoned activity leases while the gate is open

If status reports `mode=open` with a non-zero writer or protected-query lease count, omit the holder token:

```bash
MONGODB_URI='mongodb://...' \
MONGODB_DATABASE='memo_app' \
cargo run --locked --bin recover_high_search_maintenance -- \
  --apply \
  --confirm-app-stopped \
  --expected-writer-epoch 42
```

Recovery is rejected when the gate is already open and neither writer nor protected-query leases exist.

## Compare-and-swap behavior

Recovery is executed in a majority-write MongoDB transaction. The transaction is accepted only when the current maintenance mode, writer epoch, and holder token still match the inspected snapshot. Any intervening writer/query acquisition or maintenance ownership change increments/changes the guarded state and causes a conflict, leaving the newer state untouched.

After a conflict, run status inspection again. Never substitute a newly observed token or epoch without re-establishing the safety prerequisites.

## Restart

After successful recovery:

1. inspect status again and confirm `mode=open`, `active_writer_leases=0`, and `active_query_leases=0`,
2. start one application replica,
3. verify health and a controlled memo mutation,
4. restore the remaining replicas gradually,
5. investigate the cancellation/crash that created the abandoned state before attempting another rotation.
