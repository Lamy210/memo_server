# Plaintext MongoDB -> HIGH Encrypted Staging Runbook

Status: batch migration pipeline staged; production execution is not enabled.

## Purpose

This migration converts the current plaintext MongoDB authoritative memo records into
verified HIGH encrypted envelopes in the isolated `memos_encrypted_v1` collection.

The encrypted collection remains **non-authoritative**. Request-path CRUD continues to
use the current authoritative collection until the staged AWS KMS key-wrapping provider is
deployment-configured and request-path wired, ciphertext-only cache semantics are activated,
the protected search projection is approved, and encrypted-store cutover is implemented and reviewed.

The repository now also contains a staged encrypted-authoritative persistence port and
`HighMemoAuthoritativeAdapter`. It can satisfy the existing domain store contract from
`memos_encrypted_v1` while preserving owner scoping, optimistic version checks, ordered bulk
hydration, and atomic projection intents. Because `updated_at` remains inside the encrypted
payload, list ordering is performed after decryption rather than by adding plaintext sort metadata.
This adapter is not startup-wired and does not make the staging collection authoritative. A shared MongoDB authoritative-route state now defaults to `plaintext`; mutating writer permits capture that route+generation atomically with lease admission, and a future request-path router must use that permit snapshot for writes.

## Required preconditions for a final production pass

The final migration pass is an offline/frozen-write operation.

Before starting it:

1. stop memo writes or enter maintenance mode,
2. wait for in-flight mutations to finish,
3. take and verify a backup of the plaintext authoritative store,
4. keep the plaintext store unchanged until encrypted cutover verification completes,
5. use the reviewed AWS KMS MEMO-HIGH-1 provider/configuration; test providers are not acceptable.

The staged production configuration is explicit and remains disabled by default:

- `HIGH_MEMO_CRYPTO_MODE=aws-kms`,
- `HIGH_MEMO_AWS_REGION=<region>`,
- `HIGH_MEMO_ACTIVE_KEY_VERSION=<application-owned-alias>`,
- `HIGH_MEMO_AWS_KMS_KEYS_JSON=[{"key_version":"...","key_arn":"arn:...:kms:...:key/..."}]`.

The key-ring JSON may contain at most 32 versions and is capped at 64 KiB. Every key ARN must be a pinned KMS key ARN in the configured Region; KMS aliases, duplicate application aliases, duplicate KMS ARNs, and an active alias absent from the ring are rejected. The environment contains routing metadata only, never plaintext DEKs or other raw key material.

`HighMemoStagingRuntimeHandle` consumes this configuration only for operator/migration composition. It pins the AWS SDK Region, uses the standard refreshable AWS credential provider chain, and runs `DescribeKey` preflight for every configured historical/current key before exposing the staging cryptography port. Normal server startup deliberately does not construct this runtime yet.

This pipeline is **not** CDC or dual-write replication. Count checks reduce migration
risk but do not make concurrent source writes safe.

## Guarded migration operator

The repository-owned final staging command is non-destructive by default:

```bash
cargo run --locked --features aws-kms-memo --bin migrate_high_memo_staged -- --plan
```

Plan mode reports the current plaintext source count and encrypted staging count. It does not
construct the AWS KMS runtime, acquire maintenance, reset staging, or create ciphertext.

The guarded apply path is explicit:

```bash
cargo run --locked --features aws-kms-memo --bin migrate_high_memo_staged -- \
  --apply \
  --confirm-staging-reset \
  --confirm-all-writers-guarded \
  --page-size 500
```

Before apply, the operator must verify that every running server replica which can mutate MongoDB memos is deployed with shared maintenance-guard participation (HIGH memo crypto or HIGH search enabled on that replica). The CLI requires `--confirm-all-writers-guarded` because an old/unconfigured replica using the unrestricted mutation guard cannot be detected from the maintenance singleton.

Apply mode performs these steps in order:

1. validate MongoDB + HIGH memo AWS KMS deployment configuration,
2. construct the operator-only KMS runtime and complete `DescribeKey` preflight **before** traffic is frozen,
3. acquire the shared MongoDB maintenance/offline-window permit,
4. drain foreground memo writers, background reconciliation, and admitted HIGH-search queries,
5. revalidate the maintenance permit,
6. read the shared memo authoritative route and require `plaintext`,
7. delete all documents from the isolated, non-authoritative `memos_encrypted_v1` staging collection,
8. revalidate the permit,
9. perform the full bounded migration and decrypt/compare verification passes,
10. revalidate the permit again,
11. explicitly release maintenance.

Resetting the staging collection is intentional only while the shared memo route remains `plaintext`. It removes stale envelopes for source memos deleted since a prior rehearsal and makes each frozen final pass a complete rebuild from the authoritative plaintext source. Once the route is switched to `encrypted`, the same operator fails before reset so it cannot erase authoritative data. The plaintext `memos` collection is never deleted or rewritten
by this operator.

If migration fails after staging reset, the operator releases maintenance when possible and
returns failure. The partially rebuilt encrypted collection remains non-authoritative and may be
discarded/rebuilt on the next attempt. A simultaneous migration failure and maintenance-release
failure is reported as a combined fail-closed error and requires the maintenance recovery runbook.

## Batch traversal

The application migration source contract traverses plaintext memos in stable ascending
memo-ID order using a bounded page size.

The MongoDB adapter uses:

- `_id > cursor`
- ascending `_id`
- a bounded `limit`

The application layer rejects:

- page size 0,
- page sizes above the configured migration maximum,
- oversized source pages,
- duplicated/backtracking IDs,
- out-of-order pages.

This keeps memory use bounded and prevents a malformed source adapter from silently
skipping or looping over records.

## Pass 1: stage and verify each write

For every authoritative memo:

1. look for an existing encrypted staging envelope,
2. if one exists, decrypt it and compare it to the authoritative memo,
3. otherwise create a fresh per-version DEK and nonce,
4. encrypt the semantic payload,
5. atomically insert the envelope if the memo ID is absent,
6. read back the persisted winner,
7. decrypt it,
8. compare the decrypted memo to the authoritative source at current millisecond
   timestamp precision.

Parallel encryption may legitimately produce different ciphertext for identical
plaintext because every writer uses a fresh DEK and nonce. Therefore byte equality is
not the convergence criterion; authenticated decryption plus logical memo equality is.

## Pass 2: non-mutating verification

After staging, the pipeline traverses the authoritative source again.

The second pass:

- does not create ciphertext,
- does not replace ciphertext,
- requires an encrypted envelope for every authoritative memo,
- decrypts each envelope,
- compares identity, owner, version, content, tags, and timestamps to the source.

A missing or divergent envelope fails closed.

## Completion gates

A run is successful only when all of the following hold:

- source count before and after staging matches,
- migration-pass visited count matches source count,
- source count before and after verification matches,
- verification-pass visited count matches source count,
- encrypted staging count exactly matches source count,
- every staged record inspected by the passes decrypts to the corresponding source memo.

The exact staging/source cardinality check intentionally rejects target-only stale rows
left by rehearsals or incomplete earlier migrations.

These checks do not replace the write-freeze requirement. A source update can occur
between observations without changing cardinality, so production cutover still requires
writes to remain frozen for the full staging and verification window.

## Failure and rollback boundary

Before encrypted storage becomes authoritative, a migration failure is non-destructive:

- the plaintext authoritative collection remains unchanged,
- the encrypted staging collection may be discarded and rebuilt,
- request paths remain on the plaintext authoritative collection.

Do not delete plaintext records as part of this migration stage.

Once encrypted request-path writes are enabled in a later cutover, rollback requires a
separate data-convergence plan; blindly switching back to stale plaintext storage would
lose post-cutover mutations.

## Not yet enabled

This runbook does not authorize production execution yet. The following remain blockers:

- least-privilege KMS identity and key-policy review for the staged deployment configuration,
- production approval of the least-privilege KMS identity/key policy and the configured versioned key ring,
- an operator-reviewed execution/rehearsal of the guarded `migrate_high_memo_staged` command,
- HIGH Valkey request-path wiring and retirement of the legacy plaintext cache contract,
- production approval/cutover of the already-staged protected search path,
- route-aware encrypted authoritative-store request-path integration,
- an operator cutover/rollback flow that switches the shared memo route generation while maintenance is held,
- final encrypted-store cutover/rollback rehearsal.

The ciphertext-only Valkey adapter is implemented but deliberately not wired into normal CRUD yet. `MEMO-HIGH-1` remains runtime-ineligible until the remaining dependencies are implemented and its inventory status is deliberately changed to DEPLOYED.
