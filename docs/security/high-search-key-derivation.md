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
- owner changes produce different derived keys,
- seed/key rotation changes the derived search key and therefore requires projection reindex,
- plaintext key/seed material uses zeroizing wrappers,
- debug formatting redacts key/seed bytes,
- search keys remain independent from memo-encryption DEKs,
- one document/query tokenization batch resolves the owner-scoped search key once rather than once per term,
- empty token batches do not resolve key material.

## Managed-KMS direction

A managed KMS HMAC/PRF operation can implement the seed-provider boundary without exporting the long-lived KMS key.

For example, a provider may compute a SHA-384 HMAC over a domain-separated owner identifier and return the 48-byte MAC as the owner-scoped seed. memo_server then applies HKDF-SHA-384 locally for protocol separation before using the result as the blind-token HMAC key.

The provider-specific KMS key ID/ARN must remain configuration/provider state. The persisted `search_key_version` is an application-owned rotation alias, not a cloud resource locator.

Application orchestration batches all content/tag terms for one document or query into one cryptographic operation. The ring adapter resolves the owner-scoped search key once for that batch and reuses only the in-memory HMAC key for its terms. Cross-operation caching remains a separate bounded-lifetime policy decision.

## Runtime boundary

This code remains staged and runtime-unreachable.

Before SEARCH-HIGH-1 can become DEPLOYED:

- implement and review a production seed provider,
- define bounded cache/refresh behavior for resolved per-user search keys,
- prove rotation/reindex behavior,
- complete protected projection reindex verification,
- select and version the production Japanese/English analyzer,
- run the full security migration gates.

No environment variable containing a long-lived raw search-root secret should be introduced as the production mechanism.
