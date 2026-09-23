# Plaintext Metadata Whitelist

Status: Security contract  
Last reviewed: 2026-09-23

## 1. Rule

Persistent plaintext is **deny-by-default**.

A new field may be persisted outside an authenticated ciphertext only when all of the following are true:

1. the server needs the field before payload decryption or to route/repair storage,
2. the field cannot reasonably be represented as ciphertext-only data,
3. the confidentiality leakage is documented,
4. the field is added to this whitelist in a reviewed security PR,
5. tests prove that user content does not escape into storage/cache/search representations.

A database schema containing a field does not make that field approved plaintext.

## 2. Authoritative memo storage

The target encrypted memo record may persist the following plaintext metadata.

| Field | Allowed | Purpose | Constraints |
| --- | --- | --- | --- |
| `memo_id` | yes | stable lookup/routing identifier | random opaque UUID/identifier; must not embed user content |
| `owner_partition` | yes | authorization/partition lookup | internal opaque principal/partition ID, not external IdP subject |
| `version` | yes | optimistic concurrency and AAD binding | monotonic operational version only |
| `ciphertext` | yes | encrypted memo payload | authenticated ciphertext only |
| `nonce` | yes | AEAD nonce | non-secret; suite-specific validation required |
| `wrapped_dek` | yes | encrypted per-version DEK | never store plaintext DEK |
| `crypto_suite_id` | yes | crypto agility | must resolve to a known non-rejected suite |
| `key_version` | yes | unwrap/rotation routing | bounded ASCII internal routing version; no key material, cloud account identifier, provider key ID, or KMS ARN |
| `schema_version` | yes | serialization/AAD compatibility | numeric or opaque schema identifier |
| `operational_state` | conditional | lifecycle/repair state | must not contain user-authored labels/content |
| `projection_state` | conditional | reconciliation progress | opaque operational state only |

## 3. Fields not approved as plaintext

The following are **not** whitelisted for persistent plaintext storage by default:

- title
- body/content
- tags
- folder name
- workspace display name
- attachment filename
- attachment description
- attachment user-visible MIME metadata when it is user-derived or sensitive
- memo preview
- memo summary
- custom metadata
- user-authored labels
- user-visible `created_at`
- user-visible `updated_at`
- search terms
- normalized search tokens before MAC
- AI prompts/outputs derived from memo plaintext

If an operational timestamp is required outside ciphertext, it must be explicitly distinguished from the encrypted user-visible timestamp and separately reviewed.

## 4. Identity metadata

Upstream identity-provider subjects must not become durable memo ownership identifiers.

The dedicated authentication service may place its own opaque memo principal identifier in JWT `sub`. In that case, `sub` may be used as `owner_partition` directly only when the auth contract guarantees that the value is randomly generated/internal, stable for the memo authorization boundary, and does not expose or reuse an upstream provider subject.

Otherwise the required boundary is:

```text
authentication subject
       |
identity mapping
       |
internal random principal ID
       |
owner_partition
```

The mapping mechanism is an authentication/identity concern. Storage adapters receive only the internal identifier required by the repository boundary.

## 5. Valkey / cache

Allowed:

- encrypted memo representation
- ciphertext
- nonce
- wrapped key metadata
- opaque memo ID / internal owner partition
- opaque cache bookkeeping

Forbidden:

- plaintext title
- plaintext body
- plaintext tags
- decrypted preview
- normalized search term
- plaintext VAULT content

Cache keys must preserve the user/owner boundary.

## 6. Manticore / search

### HIGH

Allowed:

- memo ID / opaque owner partition required for authorization filtering
- memo-derived opaque sort keys only when they reproduce an already-whitelisted opaque identifier
- version/projection bookkeeping
- `analysis_version` for normalization/tokenization compatibility; it must identify a global analyzer contract, must not encode detected language, user locale, or content-derived metadata, and is restricted to a bounded ASCII operational identifier
- `search_key_version` for blind-token key-rotation routing; it is a bounded ASCII opaque version identifier and must contain no key material
- keyed blind tokens
- non-content operational metadata explicitly approved here

Forbidden:

- plaintext title
- plaintext body
- plaintext tags
- raw normalized terms
- raw query text persisted as an index field

Blind tokens are deterministic leakage-bearing values and must be treated as sensitive metadata, even though they are not plaintext.

### VAULT

Private content is not indexed server-side.

Only non-content operational records required to know that a VAULT memo exists may be stored, subject to this whitelist.

## 7. Backup

Backups may contain only representations already approved for the source system plus backup-container metadata.

A backup mechanism must not decrypt records before writing them.

Backup encryption keys are separate from live-data keys.

## 8. Logs, traces, metrics, and audit

These are persistence systems too. The same deny-by-default rule applies.

Forbidden values include:

- title/body/tags
- raw request payload
- decrypted memo object
- DEK / KEK / VRK
- recovery key
- bearer token
- session cookie
- DPoP private key
- WebAuthn private key
- WebAuthn PRF output
- raw search term where it may contain memo content

Identifiers in audit events should be pseudonymized with a dedicated audit key when direct resource correlation is not required.

## 9. HTTP error responses

Error responses must not disclose:

- ciphertext internals
- key IDs beyond what a client genuinely needs
- KMS/provider errors containing secret metadata
- decrypted content
- authentication token details

Internal logs may contain a sanitized error category and correlation ID.

## 10. Change procedure

Any PR that adds a new persistent plaintext field must include:

- the exact field name,
- the component(s) that store it,
- why decryption cannot precede its use,
- leakage analysis,
- retention,
- deletion behavior,
- backup behavior,
- a security test that detects accidental content leakage.

If the justification is weak, put the field inside the encrypted payload instead.
