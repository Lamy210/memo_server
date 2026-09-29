# Plaintext MongoDB -> HIGH Encrypted Staging Runbook

Status: guarded staging and encrypted-route cutover tooling implemented; production execution remains operator-controlled and disabled by default.

## Purpose

This migration converts the current plaintext MongoDB authoritative memo records into
verified HIGH encrypted envelopes in the isolated `memos_encrypted_v1` collection.

The encrypted collection remains **non-authoritative by default** because the shared memo route starts at `legacy_plaintext`. Request-path CRUD can select the already-composed encrypted repository only after the guarded cutover command atomically advances the shared memo route.

The repository now also contains a staged encrypted-authoritative persistence port and
`HighMemoAuthoritativeAdapter`. It can satisfy the existing domain store contract from
`memos_encrypted_v1` while preserving owner scoping, optimistic version checks, ordered bulk
hydration, and atomic projection intents. Because `updated_at` remains inside the encrypted
payload, list ordering is performed after decryption rather than by adding plaintext sort metadata.
When `HIGH_MEMO_CRYPTO_MODE=aws-kms` is enabled, the encrypted authoritative adapter and ciphertext-only cache are now startup-composed as a standby repository. The shared route still defaults to `legacy_plaintext`, so startup composition does not make the staging collection authoritative.

The shared MongoDB maintenance singleton now also stages an independent memo data route (`legacy_plaintext` or `encrypted`) with a monotonic generation plus memo-access leases. Normal memo CRUD/list/search hydration now enters through that memo-access guard and keeps the admitted route snapshot alive across cache/authoritative work. The maintenance barrier therefore drains active memo data-path requests before route CAS, preventing an old plaintext read from refilling the legacy Redis namespace after a future ciphertext-only cutover purge.

The route still defaults to `legacy_plaintext`. When HIGH memo runtime configuration is enabled, the request service now receives both the legacy repository and a standby encrypted repository backed by the encrypted MongoDB collection plus ciphertext-only Redis namespace. Repository selection remains exclusively driven by the admitted memo-route snapshot; there is no encrypted-to-plaintext fallback. Legacy and encrypted authoritative adapters use route-scoped reconcilers over the shared durable MongoDB projection-intent collection. A reconciler acquires the memo-access route lease before touching an intent and leaves it unacknowledged when its authoritative route is inactive. The final cutover requires this shared outbox to be empty before changing ownership.

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

`HighMemoStagingRuntimeHandle` pins the AWS SDK Region, uses the standard refreshable AWS credential provider chain, and runs `DescribeKey` preflight for every configured historical/current key. Server startup now constructs this runtime when MEMO-HIGH-1 is explicitly enabled and injects a standby encrypted repository, but the persisted memo route remains `legacy_plaintext` until an explicit operator cutover.

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
6. delete all documents from the isolated, non-authoritative `memos_encrypted_v1` staging collection,
7. revalidate the permit,
8. perform the full bounded migration and decrypt/compare verification passes,
9. revalidate the permit again,
10. explicitly release maintenance.

Resetting the staging collection is intentional. It removes stale envelopes for source memos
deleted since a prior rehearsal and makes each frozen final pass a complete rebuild from the
authoritative plaintext source. The plaintext `memos` collection is never deleted or rewritten
by this operator.

If migration fails after staging reset, the operator releases maintenance when possible and
returns failure. The partially rebuilt encrypted collection remains non-authoritative and may be
discarded/rebuilt on the next attempt. A simultaneous migration failure and maintenance-release
failure is reported as a combined fail-closed error and requires the maintenance recovery runbook.

## Guarded encrypted-route cutover

The final data-route activation is a separate operator step from staging and remains non-destructive to the plaintext authoritative MongoDB collection.

Inspect the current shared routes first:

```bash
cargo run --locked --features aws-kms-memo,aws-kms-search --bin cutover_high_memo_route -- --status
```

Protected search must already be active. Record both route generations, verify all replicas are running the route-aware encrypted standby repository, and verify a recoverable plaintext authoritative backup.

Apply:

```bash
cargo run --locked --features aws-kms-memo,aws-kms-search --bin cutover_high_memo_route -- \
  --apply-encrypted \
  --confirm-encrypted-cutover \
  --confirm-all-replicas-encrypted-ready \
  --confirm-plaintext-backup-verified \
  --confirm-no-automatic-rollback \
  --page-size 500 \
  --cache-scan-count 1000 \
  --expected-memo-route-generation <memo-generation> \
  --expected-search-route-generation <search-generation>
```

Under one shared maintenance permit the command:

1. completes KMS/key-ring preflight before freezing traffic,
2. drains memo mutations, search queries, memo-access requests, and background reconciliation,
3. requires the exact protected search-route generation,
4. requires the exact legacy memo-route generation,
5. requires the shared projection outbox to be empty,
6. resets and fully rebuilds/verifies `memos_encrypted_v1`,
7. requires the projection outbox to remain empty,
8. purges exact legacy plaintext memo keys from Redis/Valkey and verifies no matching legacy memo keys remain,
9. truncates the legacy plaintext Manticore `memos` projection and verifies an exact SQL count of zero,
10. revalidates the maintenance permit plus both route snapshots,
11. CAS-switches `legacy_plaintext -> encrypted`,
12. rereads the memo/search routes and releases maintenance only when the exact encrypted target generation is proven.

If the memo route is already `encrypted` at the exact expected generation, rerunning the command does not rebuild encrypted authoritative data from the now-potentially-stale plaintext source. It only re-purges the retired legacy Redis/Manticore plaintext secondaries under maintenance and verifies the route snapshots.

There is intentionally **no automatic encrypted-to-plaintext rollback**. Once encrypted writes resume, the retained plaintext MongoDB source can diverge. Reverse synchronization/decryption-driven rollback must be designed separately before such a rollback can be supported.

This cutover still does **not** delete the plaintext authoritative MongoDB collection. Irreversible plaintext retirement is a separate post-cutover/soak operation.

## Plaintext retirement readiness

Route cutover and plaintext retirement are deliberately separate. A successful encrypted cutover starts a soak period; it does not authorize deletion of the plaintext `memos` collection.

The shared MongoDB maintenance singleton now also carries `memo_plaintext_retirement_state` with the monotonic lifecycle `available -> in_progress -> retired`. Only a live maintenance permit can advance that state. The moment retirement reaches `in_progress`, every future attempt to switch the memo route back to `legacy_plaintext` is rejected at the MongoDB CAS boundary. This is intentional: once destructive plaintext deletion can begin, rollback to a partially stale/deleted plaintext source is no longer safe.

Every actual memo-route transition now records `memo_route_changed_at` using MongoDB server time in the same atomic update that advances `memo_route_generation`. Soak verification reads `hello.localTime` from MongoDB as well, keeping both timestamps in the same clock domain. An older encrypted route with no recorded transition time is not considered retirement-ready.

Non-freezing status inspection:

```bash
cargo run --locked --features aws-kms-memo,aws-kms-search --bin verify_high_memo_retirement -- --status
```

Exact readiness verification requires a maintenance window because cache/search/outbox checks must not race active requests or reconcilers:

```bash
cargo run --locked --features aws-kms-memo,aws-kms-search --bin verify_high_memo_retirement -- \
  --verify \
  --confirm-maintenance-window \
  --confirm-post-cutover-backup-verified \
  --confirm-restore-rehearsed \
  --minimum-soak-hours 168 \
  --encrypted-page-size 500 \
  --cache-scan-count 1000 \
  --expected-memo-route-generation <encrypted-generation> \
  --expected-search-route-generation <protected-generation>
```

The verifier is non-destructive. Under the maintenance barrier it requires:

1. plaintext retirement state exactly `available`,
2. exact `encrypted` memo route generation,
3. exact `protected` search route generation,
4. a recorded memo-route transition timestamp,
5. the configured minimum soak duration,
6. AWS KMS historical/current memo key-ring preflight,
7. protected search runtime preflight,
8. a bounded full traversal of `memos_encrypted_v1` where every envelope validates structurally, decrypts successfully, and matches its envelope memo/owner/version identity,
9. exact encrypted traversal count equality before/after the scan,
10. exact projection outbox count of zero,
11. zero exact legacy plaintext Redis memo keys,
12. exact legacy Manticore `memos` document count of zero,
13. final route/maintenance revalidation before release.

The backup and restore-rehearsal flags are operator attestations. This verifier does not inspect backup media or prove restoration integrity itself.

### Non-destructive plaintext retirement plan

Before any future destructive retirement implementation, run the repository-owned planner. It reacquires the same maintenance barrier, reruns the readiness/integrity checks against the exact expected memo/search generations, and adds an exact count of remaining plaintext authoritative memo documents. It does **not** transition `memo_plaintext_retirement_state` and performs no deletes.

```bash
cargo run --locked --features aws-kms-memo,aws-kms-search \
  --bin plan_high_memo_plaintext_retirement -- \
  --plan \
  --confirm-maintenance-window \
  --confirm-post-cutover-backup-verified \
  --confirm-restore-rehearsed \
  --minimum-soak-hours 168 \
  --encrypted-page-size 500 \
  --cache-scan-count 1000 \
  --expected-memo-route-generation <memo-generation> \
  --expected-search-route-generation <search-generation>
```

The planner reports `plan.destructive_changes=0` and `plan.plaintext_documents=<count>`. It can also inspect an existing `in_progress` retirement state after break-glass recovery, while keeping all checks non-destructive.

A `readiness.ready=true` or `plan.ready=true` result still does **not** delete plaintext data.

### Irreversible plaintext retirement

The destructive phase is a separate command and must use the exact plaintext count from a fresh planner run:

```bash
cargo run --locked --features aws-kms-memo,aws-kms-search \
  --bin retire_high_memo_plaintext -- \
  --apply \
  --confirm-irrevocable-plaintext-delete \
  --confirm-maintenance-window \
  --confirm-post-cutover-backup-verified \
  --confirm-restore-rehearsed \
  --confirm-legacy-backup-retention-reviewed \
  --minimum-soak-hours 168 \
  --encrypted-page-size 500 \
  --cache-scan-count 1000 \
  --expected-memo-route-generation <memo-generation> \
  --expected-search-route-generation <search-generation> \
  --expected-plaintext-documents <plan.plaintext_documents>
```

The operator reacquires the maintenance window and reruns the same encrypted-route, protected-search, soak, KMS/search runtime, encrypted full-decrypt, projection-outbox, legacy-cache, and legacy-search checks. It then requires the plaintext document count to exactly match the planner value **before any retirement-state transition or delete**.

For an initial retirement, the command advances `memo_plaintext_retirement_state` from `available` to `in_progress` before issuing the plaintext delete. That transition permanently fences legacy memo-route rollback. The plaintext delete is intentionally resumable rather than transaction-sized: a network error or partial/ambiguous delete leaves the state `in_progress`. Re-run the non-destructive planner to obtain the remaining exact count, then rerun the destructive command with that new count.

The command advances `in_progress -> retired` only after:
- the delete result equals the count observed under the same maintenance permit,
- a post-delete exact count is zero,
- the maintenance barrier is still enforced.

If deletion reaches zero but the final state transition fails, the next invocation can resume from `in_progress` with expected plaintext count zero. An already-`retired` state is accepted only when the exact plaintext count is zero.

There is no command that moves `in_progress` or `retired` back to `available`.

The live MongoDB deletion is a **logical dataset retirement**, not a secure-media erase. It does not prove removal from historical database backups, snapshots, storage-engine free pages, replicas that are no longer part of the deployment, or external exports. The destructive command therefore requires an explicit confirmation that legacy plaintext backup retention/disposal has been reviewed. Storage/media sanitization and historical-backup lifecycle remain deployment responsibilities outside this application command.

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

## Deployment / retirement boundary

The repository now contains guarded staging, standby encrypted request-path composition, protected-search routing, and an explicit encrypted memo-route cutover operator. None of those code paths automatically change a deployment from legacy to encrypted storage.

Production still requires deployment-specific KMS/IAM approval, rehearsed operator execution, verified backups, and deliberate route cutover. After cutover, plaintext authoritative retirement remains separately gated by the non-destructive readiness verifier/planner and the explicitly confirmed resumable retirement command above. The command is never invoked automatically.

`MEMO-HIGH-1` remains runtime-ineligible in the crypto inventory until those operational gates are satisfied and the suite status is deliberately advanced.
