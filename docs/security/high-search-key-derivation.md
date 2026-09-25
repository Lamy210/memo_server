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

A staged AWS KMS implementation uses `GenerateMac` with `HMAC_SHA_384` through `aws-sdk-kms = 1.114.0`. It accepts only a pinned KMS **key ARN**; alias identifiers and bare key IDs are rejected so alias retargeting cannot silently change seed material. The adapter requests only HMAC-SHA-384, checks that the response reports the same key ARN and MAC algorithm, and returns only the raw MAC bytes through the zeroizing managed-PRF boundary. AWS credentials, Region selection, and the key ARN remain deployment configuration and are not persisted in memo/search metadata.

Application orchestration batches all content/tag terms for one document or query into one cryptographic operation. The ring adapter resolves the owner-scoped search key once for that batch and reuses only the in-memory HMAC key for its terms.

## Bounded derived-key cache

A staged in-process cache may wrap the final `SearchKeyProvider`. It stores only the final per-owner derived search key plus its application-owned generation identifier. It does **not** cache the long-lived root or provider-resolved seed.

The cache requires both:

- a positive TTL,
- a positive maximum entry count.

Expired entries are lazily removed before cache lookup/insertion. Capacity eviction removes the earliest-expiring entry. Removing an entry drops its `Zeroizing` key storage. Rotation/deployment orchestration can explicitly invalidate one owner or clear the whole cache.

The TTL is a **reuse TTL**, not by itself a hard wall-clock memory-residency guarantee: an idle expired entry can remain allocated until another cache operation, an explicit expiry sweep, cache clear, or process teardown. The provider therefore exposes an explicit expired-entry sweep boundary for runtime maintenance. A deployment that requires a tighter plaintext-key memory-residency window must schedule that sweep at an interval consistent with its threat model.

The cache mutex is not held while awaiting the underlying provider. This prevents one slow KMS/provider call from serializing key resolution for unrelated owners. Concurrent misses for the same owner may therefore duplicate an idempotent provider call; once one result is cached, a concurrent resolver prefers the established cached generation and drops its unused resolved key.

Invalidation and clear operations also advance a cache epoch. A key resolution that started before that epoch change is rejected when it returns and is never served or reinserted. This makes rotation/deployment invalidation a fail-closed fence against stale in-flight provider results.

No production TTL, capacity, or sweep interval is selected by this staged contract. Those values must be chosen from the deployment threat model, provider latency/rate limits, and acceptable plaintext-key reuse/residency windows.

## Runtime boundary

This code remains staged and runtime-unreachable.

Before SEARCH-HIGH-1 can become DEPLOYED:

- provision/review the staged AWS KMS HMAC_384 key, IAM/key policy, credentials/Region configuration, and startup wiring,
- choose deployment TTL/capacity for the staged bounded derived-key cache and wire invalidation into the production rotation protocol,
- prove rotation/reindex behavior,
- complete protected projection reindex verification,
- select and version the production Japanese/English analyzer,
- run the full security migration gates.

No environment variable containing a long-lived raw search-root secret should be introduced as the production mechanism.
