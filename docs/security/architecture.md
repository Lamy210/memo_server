# Encryption-First Security Architecture

Status: Accepted target architecture  
Baseline: memo_server Encryption-First High-Assurance Architecture Proposal v5  
Last reviewed: 2026-09-21

## 1. Purpose

memo_server adopts an **Encryption-First / HIGH-by-Default / VAULT-for-Zero-Access** security model.

The design goal is not to maximize the number of cryptographic mechanisms. The goal is to minimize where plaintext and plaintext-capable keys exist.

This document is the architectural source of truth for security-sensitive implementation work. It describes the target architecture. It does **not** imply that every component described here is already implemented.

## 2. Current state and migration target

The current application on `main` uses:

- Rust / Actix Web backend
- SvelteKit 2 / Svelte 5 frontend
- MongoDB as the Compose/CI authoritative-store target with ScyllaDB retained as an explicit migration fallback
- Valkey 9.1 as a disposable cache (runtime cutover complete; a ciphertext-only HIGH envelope adapter is staged, while existing request paths still use the legacy plaintext cache contract)
- Manticore Search as the Compose/CI rebuildable search projection with Elasticsearch retained as an explicit migration fallback
- an independent JWT issuing authentication service boundary

The target storage stack is:

- MongoDB as authoritative storage
- Valkey as disposable cache
- Manticore Search as rebuildable search projection

Storage migration and cryptographic migration are separate concerns. MongoDB and Manticore migration adapters still handle plaintext domain data during this phase; that does not satisfy the HIGH encrypted-storage target. Existing ScyllaDB deployments must remain explicitly selected until verified backfill/cutover is complete.

The MongoDB encryption migration stages encrypted envelopes in a separate `memos_encrypted_v1` collection. That collection is deliberately **non-authoritative** until application-level crypto orchestration, a production key-wrapping provider, ciphertext-only cache semantics, and protected search projection are ready. Request paths continue to use the existing authoritative collection during this staging phase. The encrypted staging collection must not contain semantic plaintext fields and must not emit the existing plaintext projection intents. The staging path is orchestrated through a dedicated application service: existing envelopes are decrypted and compared with the authoritative memo before an idempotent rerun is accepted; new envelopes are read back and decrypted after persistence before migration progress is counted. The staging crypto port is deliberately separate from request-path crypto so a PLANNED suite cannot become authoritative by accident.

## 3. Security profiles

### HIGH

HIGH is the default profile for all normal memos.

Properties:

- server-side envelope encryption
- AES-256-GCM payload encryption
- a fresh 256-bit DEK for every memo version
- managed KMS / KEK based key wrapping
- encrypted authoritative storage
- ciphertext-only cache
- protected search projection using keyed blind tokens
- server can decrypt only when an authorized use case requires it

HIGH is intended to ensure that a database, cache, search-index, or backup dump does not reveal memo plaintext.

### VAULT

VAULT is the zero-access profile for highly sensitive memos.

Properties:

- client-side content encryption
- device-bound key material
- a Vault Root Key (VRK)
- a fresh memo DEK per version
- client-local encrypted search
- no server-side possession of VRK plaintext, memo DEK plaintext, or memo plaintext

The server must not be able to recover VAULT content if all trusted devices and the offline recovery key are lost.

## 4. Responsibility boundaries

The existing dependency direction remains mandatory:

```text
domain
  ↑
application
  ↑
infrastructure
  ↑
interfaces
```

More precisely:

### domain

Owns business invariants and storage-independent contracts.

The domain must not depend on:

- Actix Web
- MongoDB / ScyllaDB
- Valkey / Redis
- Manticore / Elasticsearch
- a specific KMS SDK
- HTTP or JWT types

Security-related domain concepts may exist only when they express actual business/security invariants used by a concrete use case.

### application

Owns use-case orchestration.

It decides when an operation needs:

- encryption
- decryption
- key resolution
- search-token derivation
- security-profile validation

It must consume abstractions, not vendor SDKs.

### infrastructure

Owns concrete crypto and I/O adapters, including:

- AEAD implementation
- KMS-backed key wrapping
- persistence serialization
- cache representation
- search projection generation
- cryptographic configuration loading

Vendor-specific types must not leak into domain entities.

### interfaces

Owns:

- HTTP request/response mapping
- authentication extraction
- input validation
- security-profile selection exposed through APIs

HTTP handlers must not implement cryptography directly.

## 5. Plaintext minimization

Plaintext persistence is deny-by-default.

A field may remain plaintext only if it is documented in `docs/security/plaintext-whitelist.md` and has a concrete operational reason.

The target encrypted memo record is conceptually:

```text
EncryptedMemoDocument {
    memo_id,
    owner_partition,
    ciphertext,
    nonce,
    wrapped_dek,
    version,
    crypto_suite_id,
    key_version,
    schema_version,
    operational_state
}
```

Semantic user content such as title, body, tags, folder names, attachment names, previews, summaries, and user-visible timestamps must not be introduced as plaintext storage fields by default.

`owner_partition` is an internal authorization/partition identifier. The current JWT `sub` may be used directly only if the dedicated authentication service guarantees that it is an opaque memo principal identifier generated for this boundary and not an upstream identity-provider subject. If that guarantee does not hold, an explicit identity-mapping layer must translate the authentication subject before persistence.

## 6. Payload encryption

HIGH and VAULT payloads use authenticated encryption.

Initial suite:

```text
AEAD: AES-256-GCM
DEK:  256-bit CSPRNG output
KDF:  HKDF-SHA-384 where derivation is required
MAC:  HMAC-SHA-384 where keyed deterministic tokens are required
```

A new DEK is generated for every memo version.

DEKs must never be reused across memo versions.

### AAD binding

The AEAD associated data must bind at least:

```text
owner_partition
memo_id
version
schema_version
crypto_suite_id
```

This prevents a valid ciphertext from being silently transplanted across owners, memo IDs, versions, or incompatible schemas.

## 7. Key hierarchy

### HIGH

Conceptual hierarchy:

```text
Managed KMS
    |
Domain / Root KEK
    |
User or partition KEK
    |
Per-version Memo DEK
```

Long-lived root key material must not be stored in application configuration or source control.

### VAULT

Conceptual hierarchy:

```text
Device-bound secret source
       |
       +--> WebAuthn PRF output, when explicitly supported
       |    by the selected credential/authenticator
       |
       +--> independent device secret otherwise
       |
HKDF-SHA-384
       |
Vault KEK
       |
Vault Root Key
       |
       +--> per-version Memo DEK
       +--> Search Key
```

A passkey private key is an authentication credential and must not be treated as the memo encryption key. Passkey authentication by itself does not expose key material. If the WebAuthn Level 3 `prf` extension is used for VAULT key derivation, the client must capability-detect it and keep PRF outputs client-side.

A PRF output is associated with a WebAuthn credential and must not be assumed to be device-bound. Its assurance inherits the credential/authenticator and synchronization model. When VAULT requires an explicitly device-bound wrapper, use a separate device secret protected by the platform or hardware keystore rather than treating a synced-passkey PRF output as device-bound.

## 8. Multi-device VAULT

The VRK is wrapped independently for authorized recovery paths:

```text
VRK
├── wrapped for Device A
├── wrapped for Device B
├── wrapped for Device C
└── wrapped for Offline Recovery Key
```

Adding a device adds a wrapper. It must not require re-encrypting every memo.

Account recovery and Vault recovery are separate ceremonies.

## 9. Search

Search is an explicit confidentiality trade-off.

### HIGH

Manticore stores keyed blind tokens rather than plaintext content.

Conceptually:

```text
normalized token
      |
HMAC-SHA-384 with per-user search key
      |
opaque deterministic token
      |
Manticore
```

Blind indexing does not provide zero knowledge. Depending on the scheme and attacker visibility, equality, frequency, document relationship, access patterns, and query patterns may leak.

### VAULT

Private VAULT content is not projected to server-side search.

The client:

1. decrypts authorized content locally,
2. maintains a local search index,
3. encrypts the local index at rest.

## 10. Cache and projection rules

Valkey is disposable.

Allowed:

- ciphertext
- wrapped-key metadata
- opaque operational metadata

Forbidden:

- plaintext title
- plaintext body
- plaintext tags
- decrypted previews

Persistence should be disabled unless there is a demonstrated operational need. If persistence is enabled, ciphertext-only invariants still apply.

The staged HIGH cache port accepts only `HighEncryptedMemoEnvelope` values and derives cache keys internally from `owner_partition + memo_id`. Reads reject cached envelopes whose authenticated identity metadata does not match the requested cache key. This adapter is not yet wired into normal request paths; the legacy plaintext cache contract remains active until encrypted authoritative storage and protected search are ready for coordinated cutover.

Manticore is a rebuildable projection, never a second source of truth.

## 11. Authentication boundary

Authentication remains an independent service.

Target responsibilities of the dedicated authentication service include:

- WebAuthn / passkey registration and authentication
- browser session lifecycle
- device management
- recovery
- step-up authentication
- access-token issuance
- signing-key rotation
- JWKS publication
- authentication audit

memo_server remains a resource server and must not own password or refresh-token storage.

The browser integration continues to use the SvelteKit BFF boundary. Long-lived browser credentials stay in Secure + HttpOnly + SameSite cookies; browser JavaScript does not receive refresh tokens.

## 12. Access-token evolution

Migration order:

```text
RS256
  ↓
ES384
  ↓
optional ML-DSA for selected long-lived trust use cases
```

PQC is not an excuse for an immediate algorithm flag day.

DPoP may be introduced for HIGH/VAULT access tokens, with the proof key managed by the BFF. Replay protection must be independently tested before it is relied on as a security property.

## 13. Post-quantum strategy

PQC is introduced where public-key cryptography has long-lived confidentiality or trust impact:

- transport key establishment
- key establishment
- long-lived signatures
- audit checkpoints
- backup manifests
- release/security-configuration signing

Payload encryption remains symmetric AEAD.

Do not design a custom ML-KEM protocol.

Use standardized hybrid TLS mechanisms when the complete network path supports them.

## 14. Crypto agility

Algorithms are selected through versioned suites rather than scattered constants.

A suite has a lifecycle:

```text
ACTIVE
  ↓
READ_ONLY
  ↓
DEPRECATED
  ↓
REJECTED
```

Only ACTIVE suites may be used for new writes.

Old ciphertext may remain readable during a controlled migration window. Unknown or rejected suites must fail closed.

The machine-readable inventory lives at:

`docs/security/crypto-inventory.yaml`

## 15. Failure policy

- Authoritative database unavailable: writes fail.
- Cache unavailable: CRUD continues from authoritative storage.
- Search unavailable: CRUD continues and search degrades.
- KMS unavailable in HIGH: operations requiring unwrap/encrypt fail closed.
- VAULT key unavailable: decryption fails; the server must not recover plaintext.
- Unknown crypto suite: reject.
- Cryptographic authentication failure: reject and do not fall back to plaintext.

## 16. Logging and observability

Logs, metrics, traces, panic reports, and audit events must never contain:

- memo body
- memo title unless explicitly security-reviewed
- DEK
- KEK
- VRK
- recovery key
- passkey private material
- raw bearer token
- DPoP private material
- WebAuthn PRF output

Required security telemetry should be metadata-only, for example:

```text
memo_encrypt_total
memo_decrypt_total
memo_decrypt_failure_total
crypto_suite_usage_total
deprecated_crypto_usage_total
vault_unlock_total
vault_unlock_failure_total
dpop_replay_rejected_total
```

## 17. Runtime and supply-chain baseline

Backend production containers should progress toward:

- non-root execution
- read-only root filesystem
- dropped Linux capabilities
- `no-new-privileges`
- seccomp
- resource limits
- disabled core dumps

CI/CD should maintain:

- SAST
- SCA
- secret scanning
- SBOM
- container scanning
- dependency review
- provenance
- release signing
- OIDC/workload identity instead of long-lived cloud credentials where supported

Third-party GitHub Actions should be pinned to immutable commit SHAs when practical.

## 18. Migration sequencing

Security migration is incremental:

1. define security contracts and inventories,
2. introduce persistence boundaries required by the MongoDB/Valkey/Manticore migration,
3. migrate authoritative storage,
4. migrate cache,
5. migrate search,
6. introduce the crypto implementation at a concrete persistence use site,
7. make new HIGH writes encrypted-only,
8. migrate legacy plaintext records with verification,
9. remove plaintext cache/search projections,
10. implement passkey-first auth integration,
11. implement DPoP/step-up,
12. implement VAULT,
13. implement local VAULT search,
14. harden backup/audit,
15. stage PQ transport and selected ML-DSA uses.

Security migration and unrelated UI work should remain in separate PR series.

## 19. Acceptance invariants

The completed target architecture must demonstrate that:

- an authoritative database dump does not reveal memo content,
- a cache dump does not reveal memo content,
- a HIGH search-index dump contains no plaintext memo terms,
- a backend compromise does not yield VAULT content keys by design,
- a KMS compromise does not decrypt VAULT content,
- an authentication database breach does not expose passkey private keys,
- backups do not contain memo plaintext,
- wrong owner/memo/version AAD causes decryption failure,
- ciphertext mutation causes decryption failure,
- every new memo version uses a fresh DEK,
- server-side VAULT decryption is impossible by construction.

## 20. Standards baseline

The target architecture is aligned with current standards and staged deployments rather than proprietary cryptography.

Verified references as of 2026-09-21:

- W3C Web Authentication Level 3 Recommendation, 2026-08-25: https://www.w3.org/TR/2026/REC-webauthn-3-20260825/
- NIST FIPS 203 (ML-KEM): https://csrc.nist.gov/pubs/fips/203/final
- NIST FIPS 204 (ML-DSA): https://csrc.nist.gov/pubs/fips/204/final
- IETF RFC 10024, PQ/T hybrid key agreement for TLS 1.3 including X25519MLKEM768: https://www.rfc-editor.org/rfc/rfc10024.html
- CRYPTREC 2026 update adding ML-KEM to the e-Government Recommended Ciphers List: https://www.cryptrec.go.jp/whatsnew.html
- Cloudflare post-quantum origin TLS deployment documentation: https://developers.cloudflare.com/ssl/post-quantum-cryptography/pqc-to-origin/
