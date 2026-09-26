# HIGH Search Key Derivation

Status: staged contract; not runtime-enabled  
Last reviewed: 2026-09-24

## Purpose

SEARCH-HIGH-1 requires stable, owner-scoped search keys for deterministic blind tokens without reusing memo-encryption DEKs.

The staged design separates two responsibilities:

1. a protected provider resolves one versioned, owner-scoped 384-bit seed,
2. memo_server derives the local search key with HKDF-SHA-384.

```text
managed key boundary
        |
        | owner-scoped PRF/HMAC result
        v
versioned 384-bit search seed
        |
        | HKDF-SHA-384
        | domain + owner + seed version
        v
per-user 384-bit search key
        |
        | HMAC-SHA-384(normalized term)
        v
blind token
```

## Security properties

- long-lived search-root key material is not part of the `SearchKeyProvider` contract,
- the seed provider receives `owner_partition` and must return an owner-scoped seed,
- per-user search keys are deterministic for the same owner and seed version,
- persisted derived-key versions include the HKDF protocol version (for example `hkdf384-v1:<seed-version>`) so derivation changes cannot masquerade as the same generation,
- owner changes produce different derived keys,
- the same owner + `search_key_version` must resolve to stable seed material,
- changing seed material requires a new application-owned `search_key_version` and projection reindex,
- plaintext key/seed material uses zeroizing wrappers,
- debug formatting redacts key/seed bytes,
- search keys remain independent from memo-encryption DEKs,
- one document/query tokenization batch resolves the owner-scoped search key once rather than once per term,
- empty token batches do not resolve key material.

## Managed-KMS direction

A staged `ManagedPrfSearchKeySeedProvider` now adapts a managed HMAC/PRF operation into the seed-provider boundary without exporting the long-lived KMS/HSM key.

The adapter sends only a canonical message containing the protocol domain, the versioned PRF seed generation, and the opaque `owner_partition`. The returned seed generation is `prf384-v1:<provider-seed-version>`, so a future PRF protocol change cannot silently produce different seed bytes under the same version. It accepts only a 48-byte HMAC-SHA-384 result and wraps that output in zeroizing seed storage. Memo plaintext, normalized search terms, and blind tokens never cross this managed-PRF boundary.

memo_server then applies HKDF-SHA-384 locally for a second protocol-separation layer before using the result as the blind-token HMAC key.

The provider-specific KMS key ID/ARN must remain configuration/provider state. The provider-specific client receives an application-owned provider-seed rotation alias, not a cloud resource locator. The managed adapter promotes it to `prf384-v1:<provider-seed-version>`; HKDF then produces the persisted projection generation `hkdf384-v1:prf384-v1:<provider-seed-version>`, binding provider rotation, managed-PRF protocol, and local HKDF protocol.

A provider-specific PRF client must never let a mutable cloud alias silently change seed bytes while returning the same application `search_key_version`. It must bind to immutable provider key material (or otherwise detect provider-key revision changes) and require a new application seed version whenever that material changes. The generic adapter intentionally does not persist a cloud key ID/ARN/version.

A staged AWS KMS implementation uses `GenerateMac` with `HMAC_SHA_384` through `aws-sdk-kms = 1.114.0`. Startup uses `aws-config = 1.12.0` only when HIGH search is enabled, explicitly pins the SDK Region to the Region already validated against the KMS key ARN, and leaves credentials to the standard refreshable AWS credential-provider chain. Before retaining the runtime, startup performs a fail-closed `DescribeKey` preflight and requires the returned ARN to match the pinned key, `Enabled=true`, `KeyState=Enabled`, `KeySpec=HMAC_384`, `KeyUsage=GENERATE_VERIFY_MAC`, and advertised `HMAC_SHA_384` support. It accepts only a pinned KMS **key ARN**; alias identifiers and bare key IDs are rejected so alias retargeting cannot silently change seed material. The runtime still checks every `GenerateMac` response for the same key ARN, algorithm, and 48-byte MAC width. AWS credentials, Region selection, and the key ARN remain deployment configuration and are not persisted in memo/search metadata.

Application orchestration batches all content/tag terms for one document or query into one cryptographic operation. The ring adapter resolves the owner-scoped search key once for that batch and reuses only the in-memory HMAC key for its terms.

## Bounded derived-key cache

A staged in-process cache may wrap the final `SearchKeyProvider`. It stores only the final per-owner derived search key plus its application-owned generation identifier. It does **not** cache the long-lived root or provider-resolved seed.

The cache requires both:

- a positive TTL,
- a positive maximum entry count.

Expired entries are lazily removed before cache lookup/insertion. Capacity eviction removes the earliest-expiring entry. Removing an entry drops its `Zeroizing` key storage. Rotation/deployment orchestration can explicitly invalidate one owner or clear the whole cache.

The TTL is a **reuse TTL**, not by itself a hard wall-clock memory-residency guarantee: an idle expired entry can remain allocated until another cache operation, an explicit expiry sweep, cache clear, or process teardown. The provider therefore exposes an explicit expired-entry sweep boundary for runtime maintenance. A deployment that requires a tighter plaintext-key memory-residency window must schedule that sweep at an interval consistent with its threat model.

The global cache-state mutex is not held while awaiting the underlying provider. Concurrent misses for the same owner are serialized behind a per-owner async resolution gate: one successful provider call populates the cache and queued same-owner callers consume that established generation. Different owners use independent gates and can still resolve in parallel, so a slow KMS/provider call for one owner does not serialize unrelated owners. Provider failures are not cached; a queued same-owner caller may retry after the failed leader releases the gate. Cancelling a resolver releases the async gate guard, so later callers are not permanently blocked.

Invalidation and clear operations also advance a cache epoch. A key resolution that started before that epoch change is rejected when it returns and is never served or reinserted. This makes rotation/deployment invalidation a fail-closed fence against stale in-flight provider results.

No production TTL, capacity, or sweep interval is selected by this staged contract. Those values must be chosen from the deployment threat model, provider latency/rate limits, and acceptable plaintext-key reuse/residency windows.

## Fail-closed configuration contract

HIGH protected search remains disabled unless `HIGH_SEARCH_MODE=aws-kms` is explicitly selected. Merely supplying KMS/cache variables does not activate it.

When `aws-kms` is selected, configuration parsing requires all of the following before application startup can proceed:

- `AUTHORITATIVE_BACKEND=mongodb`,
- the binary was built with the `aws-kms-search` Cargo feature,
- `SEARCH_BACKEND=manticore`,
- `HIGH_SEARCH_AWS_KMS_KEY_ARN` as a pinned KMS key ARN, never an alias or bare key ID,
- `HIGH_SEARCH_AWS_REGION`, matching the Region encoded by the KMS key ARN,
- `HIGH_SEARCH_SEED_VERSION`, constrained so the final `hkdf384-v1:prf384-v1:<provider-seed-version>` identifier remains within the persisted version bound,
- positive `HIGH_SEARCH_KEY_CACHE_TTL_SECONDS`,
- positive `HIGH_SEARCH_KEY_CACHE_MAX_ENTRIES`,
- positive `HIGH_SEARCH_KEY_CACHE_SWEEP_SECONDS`,
- positive `HIGH_SEARCH_MAX_DOCUMENT_CONTENT_TERMS`,
- positive `HIGH_SEARCH_MAX_QUERY_CONTENT_TERMS`,
- positive `HIGH_SEARCH_MAX_NORMALIZED_TERM_BYTES`.

The MongoDB requirement is deliberate: the accepted HIGH storage target, plaintext-to-encrypted staging source, and protected-search reindex source are MongoDB-backed. ScyllaDB remains a migration fallback for deployments that have not completed authoritative cutover, but such deployments cannot claim the staged HIGH search runtime.

There are deliberately no production defaults for key-cache TTL, capacity, sweep cadence, or analysis work budgets. These values remain an explicit deployment/security decision. A fully valid AWS KMS configuration is still rejected when the running binary lacks the `aws-kms-search` build feature, preventing configuration from claiming a capability that was compiled out. This configuration contract does not itself wire HIGH search into request handling.

## Staged runtime composition

`HighSearchRuntimeStack` composes the accepted pieces behind one boundary without installing them into request handling:

```text
HighSearchConfig
+ managed PRF client
        |
        v
ManagedPrfSearchKeySeedProvider
        |
        v
HKDF-SHA-384 owner key
        |
        v
bounded derived-key cache
        |
        v
HMAC-SHA-384 blind-token cryptography
        ^
        |
ICU4X analyzer
        |
        v
HighSearchProjectionService
        |
        v
protected Manticore projection
```

Disabled configuration returns no stack and does not require provider dependencies. Enabled configuration fails closed when its managed PRF client is absent. It also compares the client's declared provider and immutable key reference with the configured AWS KMS key ARN, preventing dependency injection from silently binding SEARCH-HIGH-1 to a different provider or key. The factory revalidates cache policy and analysis work budgets even though configuration parsing already validates them.

The stack also exposes explicit cache sweep, owner invalidation, and global clear operations so later startup/rotation wiring does not need to reach through cryptographic internals.

## Rotation / reindex protocol

Key-generation rotation is staged behind an application-owned orchestration service. The protocol requires a concrete `HighSearchOfflineWindowGuard` backed by an enforced maintenance/write-freeze mechanism; a boolean operator assertion is intentionally insufficient. The guard must acquire a live `HighSearchOfflineWindowPermit` whose lifetime keeps that exclusion active.

The concrete MongoDB maintenance guard uses a singleton gate document plus per-mutation writer leases. Acquiring a writer lease and closing the maintenance gate both update the same singleton inside majority-write transactions, so a writer/maintenance race is serialized by MongoDB before a writer lease can become visible. Once the maintenance barrier is closed, new create/update/delete operations fail with service-unavailable and the guard waits for all already-acquired writer leases to drain before returning its offline permit. Writer leases do not auto-expire: a crashed writer or abruptly cancelled mutation intentionally leaves maintenance blocked until operator recovery is performed, preferring fail-closed unavailability over an unsafe time-based assumption that a mutation has stopped. Likewise, dropping an offline-window permit without its explicit release path leaves the maintenance barrier closed. The protected HIGH search request path is not installed yet; when it is, it must reuse the same maintenance barrier before SEARCH-HIGH-1 can be marked deployed.

Operator recovery is explicit and compare-and-swap guarded. The recovery command is non-destructive by default and only mutates state when `--apply`, `--confirm-app-stopped`, and the operator-observed `writer_epoch` are supplied. An active maintenance barrier additionally requires its exact holder token. Recovery clears stale writer leases and reopens the barrier in one majority-write MongoDB transaction, then increments `writer_epoch`; if mode, holder ownership, or writer epoch changed after inspection, the transaction aborts with a conflict. This is a break-glass recovery path, not a lease-expiry mechanism, and it must only be run after every application replica capable of acquiring a writer lease has been stopped.

The sequence is:

1. validate the reindex page size before touching key state,
2. acquire the offline-window permit,
3. keep the permit alive while clearing the derived-key cache and running the bounded reindex + convergence verification,
4. revalidate the permit's backing lease after reindex,
5. return a must-use `HighSearchRotationReady` value that still owns the permit,
6. let the caller perform its cutover while that value remains alive,
7. call `finish_after_cutover` to revalidate the lease once more and release the permit, or `abort` to clear target-generation cached keys before releasing it.

If reindex or the pre-cutover permit check fails, the derived-key cache is cleared again while the permit is still held and the operation remains failed. Permit release is explicit and awaited after cutover, abort, or fail-closed cleanup. If cache cleanup or barrier release also fails, the result is promoted to service-unavailable with the combined failure context; a failed release leaves the shared gate closed rather than silently resuming writes. The protocol does not itself change provider configuration or switch request routing; those remain caller/operator responsibilities, but the permit now spans that caller-owned cutover window instead of being dropped immediately after reindex.

## Staged startup wiring

`HighSearchRuntimeHandle` now owns provider/startup composition without exposing HIGH search to routes. When HIGH search is disabled it returns an empty handle and does not load AWS configuration. When `HIGH_SEARCH_MODE=aws-kms` is selected, it loads AWS shared configuration with the already-validated Region explicitly overridden, constructs one shared KMS client, binds it to the pinned key ARN, and builds `HighSearchRuntimeStack`.

The handle retains the stack for the application lifetime and runs the configured derived-key expiry sweep with a weak runtime reference. Dropping the handle aborts that maintenance task. In AWS KMS mode, startup also requires `kms:DescribeKey` in addition to the runtime `kms:GenerateMac` permission; missing credentials, missing authorization, unavailable metadata, or an incompatible key configuration fail startup. The `DescribeKey` preflight completes before the HIGH search stack is retained and before its cache-sweeper task is spawned, so an invalid KMS deployment cannot leave a partially active staged runtime behind. The runtime is still not installed into request handling, so startup composition alone does not activate protected search or change the legacy search path.

## Runtime boundary

The provider/runtime stack is now startup-composed when explicitly enabled. Its protected projection service is also connected to the durable projection outbox/reconciler so create/update/delete events mirror into `memos_high_v1` while HIGH search is enabled, but protected search **query routing remains request-path-inactive**.

The reconciler acquires the same MongoDB maintenance writer lease used by foreground memo mutations before touching legacy search, protected HIGH search, or cache projections. It releases that lease before acknowledging the durable projection intent. HIGH projection failure or lease-release failure therefore leaves the outbox intent available for idempotent retry, and the staged reindex barrier drains/blocks both foreground mutations and background projection retries.

A staged operator command, `reindex_high_search_staged`, can now compose that runtime with the MongoDB authoritative source, the shared maintenance barrier, and protected Manticore reindex/convergence verification. Its `--apply` mode requires explicit confirmation that all memo writers participate in the barrier and that protected request routing is still inactive. While that offline permit is held, the staging runner resets the isolated `memos_high_v1` projection before the full rebuild so repeated validation runs cannot retain rows for memos deleted since an earlier staged run. Because there is no protected routing generation to switch yet, successful staging performs no request cutover; it only revalidates and releases the maintenance permit after convergence succeeds.

Before SEARCH-HIGH-1 can become DEPLOYED:

- provision/review the staged AWS KMS HMAC_384 key and IAM/key policy, and validate deployment credential-provider behavior,
- choose deployment TTL/capacity for the staged bounded derived-key cache and wire invalidation into the production rotation protocol,
- prove rotation/reindex behavior,
- complete protected projection reindex verification,
- select and version the production Japanese/English analyzer,
- run the full security migration gates.

No environment variable containing a long-lived raw search-root secret should be introduced as the production mechanism.
