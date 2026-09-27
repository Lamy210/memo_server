# Plaintext MongoDB -> HIGH Encrypted Staging Runbook

Status: batch migration pipeline staged; production execution is not enabled.

## Purpose

This migration converts the current plaintext MongoDB authoritative memo records into
verified HIGH encrypted envelopes in the isolated `memos_encrypted_v1` collection.

The encrypted collection remains **non-authoritative**. Request-path CRUD continues to
use the current authoritative collection until the staged AWS KMS key-wrapping provider is
deployment-configured and request-path wired, ciphertext-only cache semantics are activated,
the protected search projection is approved, and encrypted-store cutover is implemented and reviewed.

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
- a guarded migration command or operational job that constructs the operator-only KMS staging runtime,
- HIGH Valkey request-path wiring and retirement of the legacy plaintext cache contract,
- a production language-aware analyzer and production search-key provider for the staged protected Manticore orchestration,
- an operator-guarded invocation of the protected-search reindex/verification service,
- encrypted authoritative-store request-path integration,
- final cutover/rollback rehearsal.

The ciphertext-only Valkey adapter is implemented but deliberately not wired into normal CRUD yet. `MEMO-HIGH-1` remains runtime-ineligible until the remaining dependencies are implemented and its inventory status is deliberately changed to DEPLOYED.
