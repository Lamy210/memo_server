# Threat Model

Status: Security baseline  
Last reviewed: 2026-09-21

## 1. Scope

This threat model covers memo confidentiality, integrity, authentication boundaries, persistence, search, cache, backup, client-side VAULT behavior, and the software supply chain.

It is intentionally implementation-oriented. New security mechanisms must state which threat or trust boundary they address.

## 2. Security objectives

### Confidentiality

- Stored memo content is encrypted by default.
- Storage-system administrators cannot recover HIGH plaintext from storage alone.
- Backend, KMS, and operators cannot recover VAULT plaintext by design.
- Authentication secrets and encryption secrets are separated.

### Integrity

- Ciphertext substitution across users, memo IDs, versions, schemas, and suites is detected.
- Persistence projections converge from the authoritative store.
- Audit history is tamper-evident.
- crypto-suite downgrade is rejected.

### Availability

- cache/search failures do not unnecessarily break core CRUD.
- cryptographic service failure never triggers plaintext fallback.
- recovery procedures do not weaken VAULT confidentiality.

## 3. Trust boundaries

```text
Browser
  |
  | same-origin requests
  v
SvelteKit BFF
  |
  | short-lived authenticated API call
  v
memo_server
  |
  +--> authoritative storage
  +--> disposable cache
  +--> rebuildable search projection
  +--> managed KMS (HIGH only)

VAULT:
Browser/device crypto boundary
  |
  +--> ciphertext only to BFF/backend/storage
```

The browser is not treated as a uniformly trusted environment. Same-origin JavaScript compromise is a first-class VAULT threat.

## 4. Assets

Highest-value assets:

- memo plaintext
- VAULT plaintext
- per-version memo DEKs
- HIGH wrapping keys / KMS permissions
- VRK
- VAULT recovery key
- device wrapping keys
- authentication signing keys
- passkey private keys
- browser session credentials
- access tokens and DPoP private keys
- search keys
- backup keys
- audit signing/checkpoint keys

Lower-sensitivity but still protected metadata:

- user-to-memo relationships
- memo counts
- access frequency
- timestamps
- search equality/frequency patterns
- device membership
- recovery activity

## 5. Threats and required controls

### T1: Authoritative database dump theft

Risk:

An attacker obtains a logical or physical dump.

Required controls:

- encrypted HIGH payloads
- client-encrypted VAULT payloads
- wrapped DEKs only
- no semantic plaintext fields unless whitelisted
- encrypted backups
- key material stored outside the database

Residual risk:

Operational metadata and relationship patterns may remain visible.

### T2: Database administrator compromise

Risk:

A privileged storage operator can query all stored records.

Required controls:

- same controls as T1
- KMS separation from database administration
- audited privileged access
- no plaintext fallback maintenance mode

Residual risk:

HIGH can still be decrypted by an authorized backend path that has KMS access. VAULT must not have that property.

### T3: Cache memory or persistence leak

Risk:

Valkey process memory, RDB, AOF, snapshot, or filesystem is exposed.

Required controls:

- ciphertext-only cache entries
- plaintext test assertions
- persistence disabled by default unless justified
- no decrypted preview cache

### T4: Search-index compromise

Risk:

Manticore index files or query access are exposed.

Required controls for HIGH:

- keyed blind tokens
- per-user/domain-separated search keys
- no title/body/tag plaintext

Required controls for VAULT:

- no private-content server-side index

Residual risk for HIGH:

- equality
- frequency
- relationship
- access pattern
- query pattern

Blind indexing must not be described as zero knowledge.

### T5: Backup theft

Risk:

An attacker obtains backup objects or backup credentials.

Required controls:

- encrypted backups
- backup keys separated from live keys
- immutable/versioned storage
- separate credentials
- no plaintext VAULT material
- restore testing

Legacy plaintext backups are a migration risk until retention expires.

### T6: JWT or access-token theft

Risk:

A valid bearer token is copied from a browser, proxy, log, or compromised process.

Required controls:

- short token lifetime
- no token logging
- BFF-held browser credentials
- DPoP for selected HIGH/VAULT requests after replay behavior is tested
- step-up for sensitive operations

Residual risk:

A compromised BFF process may use credentials available to that process.

### T7: Browser session theft

Risk:

An attacker obtains the long-lived browser session.

Required controls:

- Secure + HttpOnly + SameSite cookies
- no refresh token in localStorage
- CSRF/same-origin protection
- session rotation
- explicit logout/invalidation
- step-up for security-critical changes

### T8: XSS or malicious same-origin JavaScript

Risk:

Injected or maliciously deployed JavaScript can read plaintext after client decryption and can potentially access VAULT key operations.

Required controls:

- strict CSP
- minimal third-party scripts
- dependency review
- deterministic/verified builds where practical
- Trusted Types where practical
- release provenance/signing
- no unnecessary plaintext lifetime in browser state

Residual risk:

For web-based VAULT, same-origin application compromise is a fundamental high-impact threat. A future signed native client may provide a stronger distribution boundary.

### T9: Backend remote-code execution

Risk:

An attacker gains arbitrary code execution in memo_server.

HIGH impact:

- plaintext being processed at that moment may be exposed
- available cached data keys may be exposed
- KMS-authorized unwrap operations may be invoked

VAULT requirement:

- backend still lacks VRK and memo DEK plaintext by design

Controls:

- least privilege
- minimal key-cache TTL
- read-only/non-root container hardening
- dependency scanning
- no core dumps
- outbound-network restriction where practical

### T10: Managed KMS compromise or over-broad KMS permission

Risk:

An attacker gains HIGH key-unwrapping capability.

Required controls:

- key hierarchy and scope separation
- audited KMS operations
- least-privilege service identity
- key-versioning and rotation
- short-lived workload identity

VAULT requirement:

KMS compromise must not provide VAULT plaintext.

### T11: Operator abuse

Risk:

An authorized operator attempts to inspect user content.

Required controls:

- plaintext-minimized observability
- no ad-hoc plaintext admin endpoints
- least privilege
- audited administrative actions
- explicit break-glass procedure for HIGH if ever introduced

VAULT requirement:

No operator recovery path exists for plaintext.

### T12: CI/CD compromise

Risk:

An attacker changes source, build inputs, release artifacts, deployment configuration, or frontend JavaScript.

Required controls:

- protected branches
- review
- CI status gates
- immutable Action pinning where practical
- OIDC/workload identity
- artifact provenance
- release signing
- secret scanning
- dependency review

VAULT impact is critical because a malicious frontend can exfiltrate plaintext after decryption.

### T13: Dependency supply-chain attack

Risk:

A compromised Rust, npm, GitHub Action, container, or system dependency executes in CI/build/runtime.

Required controls:

- lockfiles
- SCA
- SBOM
- dependency review
- minimized dependencies
- provenance verification where available
- reproducibility/determinism improvements
- explicit review of cryptographic libraries

### T14: Recovery abuse

Risk:

An attacker uses account recovery to bypass VAULT encryption.

Required controls:

- account recovery != VAULT recovery
- offline high-entropy recovery secret or trusted-device approval
- no email-only or SMS-only VAULT recovery
- audited device/recovery changes
- step-up before adding recovery methods

### T15: Key/algorithm downgrade

Risk:

An attacker or stale client requests an obsolete or weaker suite.

Required controls:

- versioned crypto-suite registry
- lifecycle states
- new writes only with ACTIVE suites
- reject unknown/rejected suites
- no automatic plaintext or legacy fallback

### T16: Ciphertext substitution

Risk:

A ciphertext is copied to another memo, owner, version, or schema.

Required controls:

Bind AAD to:

- owner partition
- memo ID
- version
- schema version
- crypto suite ID

Authentication failure must be terminal.

### T17: AES-GCM nonce/key misuse

Risk:

A nonce is reused under the same key or a key is reused beyond policy.

Required controls:

- fresh random 256-bit DEK per memo version
- cryptographically secure randomness
- explicit nonce format/version
- tests asserting fresh DEKs and successful tamper detection

### T18: Search-key compromise

Risk:

An attacker obtains a HIGH search key and can compute tokens.

Required controls:

- per-user/domain-separated keys
- independent rotation/version
- never reuse memo-encryption keys as search keys
- avoid logging normalized tokens before MAC

### T19: Future quantum cryptanalysis

Risk:

Long-lived public-key encrypted or signed data becomes vulnerable to future cryptanalysis.

Required controls:

- crypto agility
- standardized hybrid PQ key establishment where supported
- staged ML-KEM/ML-DSA adoption
- symmetric payload encryption retained
- no custom PQ protocol

This threat does not justify replacing all cryptography simultaneously.

## 6. Abuse cases

Security tests must include at least:

- cross-user memo access
- wrong owner AAD
- wrong memo ID AAD
- wrong version AAD
- ciphertext mutation
- unknown suite
- rejected suite
- repeated/stale DPoP proof once DPoP exists
- recovery without a trusted VAULT recovery factor
- storage/cache/search inspection for known plaintext markers
- restoration from encrypted backup
- projection outage/recovery

## 7. Non-goals

The architecture does not claim to hide all metadata.

It does not protect HIGH plaintext from a fully compromised authorized backend while that backend is performing legitimate decryption.

It does not make browser-delivered VAULT immune to malicious same-origin JavaScript.

It does not call blind-index search zero knowledge.

It does not invent new cryptographic primitives or protocols.

## 8. Review triggers

Re-run threat-model review when any of the following changes:

- new plaintext field
- new external service receiving memo-derived data
- new AI/OCR/embedding feature
- new recovery path
- new cryptographic suite
- key hierarchy change
- new browser third-party script
- new KMS provider
- new backup system
- search tokenization change
- attachment support
- client-native application introduction
