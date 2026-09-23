# HIGH Search Analysis Contract

Status: staged application contract; production analyzer not selected  
Last reviewed: 2026-09-23

## Purpose

HIGH search must transform plaintext into normalized terms before the HMAC blind-token boundary without letting plaintext terms enter Manticore, Valkey, logs, traces, or durable operational metadata.

The application layer owns this orchestration. Concrete language segmentation remains replaceable.

## Boundaries

The staged flow is:

```text
Memo / user search input
        |
HighSearchTextAnalyzer
        |
normalized plaintext terms (memory only)
        |
HighSearchTokenCryptography
        |
owner-scoped SEARCH-HIGH-1 blind tokens
        |
HighMemoSearchProjection
        |
Manticore memos_high_v1
```

The analyzer and cryptography boundaries are independent. Search-key material is never exposed to the analyzer, and the projection never accepts raw normalized terms.

## Analysis version

Every projected tokenized document carries an opaque global `analysis_version`.

Tokenized queries carry the same version and Manticore filters on it. Changing normalization or segmentation therefore requires an explicit reindex rather than silently mixing old and new search semantics.

The version is a bounded ASCII operational identifier and must identify a global analyzer contract. It must not encode:

- detected content language,
- user locale,
- memo labels,
- user-authored metadata.

An empty query with no tag may span analysis versions so owner-scoped listing can remain available during reindex.

## Term handling

The application orchestration:

- validates analyzer output before HMAC,
- deduplicates normalized terms before token derivation,
- sorts only the resulting opaque blind-token values before projection, rather than persisting plaintext-derived term ordering,
- rejects empty content-term sets for projected memos,
- never logs normalized terms,
- rejects mixed search-key versions within one projection/search operation.

Deduplication intentionally avoids storing repeated blind tokens solely to preserve plaintext term frequency. SEARCH-HIGH-1 still leaks equality and cross-document frequency for equal blind tokens and is not zero knowledge.

## Language-aware analyzer requirement

Unicode default word boundaries are a useful baseline, but they are not sufficient as the sole production word-segmentation policy for all supported languages.

Unicode Standard Annex #29 explicitly notes that reliable word-boundary detection for languages including Japanese and Chinese requires more sophisticated, typically dictionary-based, handling. Therefore `unicode-segmentation::unicode_words()` alone is not accepted as the production analyzer for this project.

References:

- Unicode UAX #29: https://www.unicode.org/reports/tr29/
- unicode-segmentation: https://docs.rs/unicode-segmentation/

The production analyzer selection must be measured against representative Japanese and English memo/search corpora before cutover.

## Search semantics

Protected blind-token search is term-presence search. Phrase, prefix, fuzzy, stemming, morphology, synonym, and substring semantics are not implied by the cryptographic token format.

Any future feature that changes which plaintext terms are generated must:

1. receive a new analysis version,
2. preserve the plaintext-deny-by-default rule,
3. include index/query compatibility tests,
4. include a reindex plan,
5. document any additional leakage.

## Reindex verification

The protected projection migration uses a bounded-memory two-pass reindex.

1. Page the authoritative plaintext source by memo ID.
2. Analyze and blind-tokenize each memo through the normal protected-search orchestration.
3. Replace the protected Manticore row.
4. Immediately verify only the whitelisted routing/version metadata for that row.
5. Re-page the authoritative source without mutating the projection.
6. Recompute the expected metadata and verify each source memo still matches.
7. Require final protected-projection cardinality to equal authoritative-source cardinality.

This detects missing rows, target-only stale rows, source version changes between passes, analyzer-version changes, and search-key-version changes that would otherwise make a partial reindex look successful.

The inspector never returns blind tokens or memo plaintext. It checks exact metadata predicates inside Manticore and exposes only a boolean match result plus total projection count.

The final production reindex still requires the authoritative source to be write-frozen. This is a convergence verifier, not change-data capture.

## Runtime status

This contract is not wired into normal request-path search yet.

Runtime cutover still requires:

- a production language-aware analyzer,
- a production search-key provider,
- an operator-guarded invocation of the staged protected projection reindex/verification service,
- an explicit search-key rotation protocol that prevents old/new key-version query gaps (for example generation-based reindex plus atomic switch or verified dual-read),
- an explicit analysis-version migration protocol for tokenizer/normalizer changes,
- request-path orchestration wiring,
- rollback rehearsal.

Changing either the search-key version or analysis version in place while only one version is queried can make valid documents temporarily undiscoverable. Runtime activation must therefore treat projection generations as a coordinated migration, not as a per-request configuration flip.
