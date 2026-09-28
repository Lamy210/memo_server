# HIGH Search Analysis Contract

Status: ICU4X production candidate staged; synthetic representative gate active; production workload/cutover validation pending  
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
- applies an application-owned work budget before invoking cryptography,
- limits unique document content terms,
- limits unique query content terms,
- limits normalized term byte length for content and tags,
- sorts only the resulting opaque blind-token values before projection, rather than persisting plaintext-derived term ordering,
- rejects empty content-term sets for projected memos,
- never logs normalized terms,
- rejects mixed search-key versions within one projection/search operation.

Deduplication intentionally avoids storing repeated blind tokens solely to preserve plaintext term frequency. SEARCH-HIGH-1 still leaks equality and cross-document frequency for equal blind tokens and is not zero knowledge.

### Analysis work budget

`HighSearchProjectionService` requires an explicit `HighSearchAnalysisBudget`. No production numerical defaults are embedded in the service. Runtime configuration supplies these values explicitly as `HIGH_SEARCH_MAX_DOCUMENT_CONTENT_TERMS`, `HIGH_SEARCH_MAX_QUERY_CONTENT_TERMS`, and `HIGH_SEARCH_MAX_NORMALIZED_TERM_BYTES`. The caller must supply positive limits for:

- unique normalized content terms per document,
- unique normalized content terms per query,
- bytes per normalized term.

Document tags remain additionally bounded by the memo-domain tag-count invariant. These checks happen after normalization/canonicalization but before blind-token derivation, so an analyzer implementation cannot bypass the budget and force unbounded HMAC/KMS/projection work.

The budget is an operational admission-control policy rather than token semantics: changing only a budget does not change blind tokens for inputs that remain accepted, so it does not by itself require a new `analysis_version`. However all indexing, reindex, and query workers in one deployment must use the same reviewed budget policy. Production values must be selected from representative corpus measurements and projection-size/load testing rather than guessed defaults.

The local `measure_high_search_workload` operator command supports that sizing step without KMS/Manticore access. It accepts only a bounded local JSON corpus and emits aggregate analyzer measurements: document/query term-count distributions, tag-count distribution, normalized-term byte-length distribution, and zero-term counts. It never emits input plaintext, identifiers, normalized terms, blind tokens, or key material. The input corpus must be generated or separately approved/sanitized; production/user content must not be committed to the repository.

## Language-aware analyzer candidate

The staged production candidate uses ICU4X components pinned exactly to 2.3.0:

- `icu_segmenter = =2.3.0`
- `icu_normalizer = =2.3.0`
- `icu_casemap = =2.3.0`

The global analyzer generation is:

`icu4x-2.3.0-uax29-17-nfkc-fold-dict-v1`

Its pipeline is deterministic:

1. reject NUL,
2. Unicode NFKC,
3. locale-independent Unicode case fold,
4. NFKC again,
5. trim surrounding whitespace,
6. ICU dictionary word segmentation for content/title,
7. retain only word-like segments,
8. deduplicate normalized content terms in memory before the cryptographic boundary,
9. keep normalized tags as exact terms rather than segmenting them.

The second NFKC pass makes compatibility normalization explicit after case folding. Title and content use identical segmentation and are merged into one protected content-token set; no plaintext title/content distinction reaches Manticore. Pre-deduplication is semantics-preserving because application orchestration already canonicalizes to unique normalized terms, but it reduces transient memory and HMAC work for highly repetitive memo content.

ICU4X's dictionary word segmenter supplies compiled dictionary handling for complex scripts including Japanese, while non-complex text follows its Unicode word-boundary implementation. The dependency versions are exact-pinned because a tokenizer/data upgrade can alter blind-token inputs even when application code does not change.

This remains a **production candidate**, not an activated production analyzer. A versioned synthetic conformance corpus at `backend/testdata/high_search_analysis_corpus_v1.json` locks known normalization/segmentation cases to the analyzer generation and is executed by unit tests. It intentionally contains only repository-owned synthetic strings and is safe to commit.

The small synthetic conformance corpus is a compatibility regression gate, **not** representative production-corpus approval.

A second repository-owned synthetic representative workload at `backend/testdata/high_search_representative_corpus_v1.json` now exercises end-to-end analyzer semantics across English, Japanese, width/case normalization, exact tags, positive/negative multi-term queries, and tag-only search. CI analyzes both the memo side and query side, requires the expected protected-search result set under AND/exact-tag semantics, and locks the observed document/query term-count distributions plus maximum normalized-term byte length. Because blind-token HMAC preserves equality, this validates analyzer/index/query compatibility without persisting plaintext-derived tokens or requiring KMS/Manticore access.

This representative gate deliberately contains no user data and is suitable for source control. It approves the staged semantic contract and supplies concrete lower-bound sizing evidence for analysis budgets; it does **not** claim that its term distributions represent production traffic. Before runtime cutover, sanitized or generated workload measurements representative of the intended deployment still need to confirm production term-count/term-size distributions and capacity assumptions without committing user content. Any change to ICU version, normalization order, segmentation mode, filtering, tag semantics, or expected corpus output requires an explicit review; changes to token semantics require a new global `analysis_version` and a verified reindex.

References:

- Unicode UAX #29: https://www.unicode.org/reports/tr29/
- ICU4X word segmentation: https://docs.rs/icu_segmenter/2.3.0/icu_segmenter/struct.WordSegmenter.html
- ICU4X normalization: https://docs.rs/icu_normalizer/2.3.0/icu_normalizer/
- ICU4X case mapping: https://docs.rs/icu_casemap/2.3.0/icu_casemap/

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

The cardinality check uses Manticore SQL `SELECT COUNT(*)` rather than JSON `hits.total`. Manticore can report `total_relation=gte` for non-exact JSON totals, while the migration gate requires an exact count.

This detects missing rows, target-only stale rows, source version changes between passes, analyzer-version changes, and search-key-version changes that would otherwise make a partial reindex look successful.

The inspector never returns blind tokens or memo plaintext. It checks exact metadata predicates inside Manticore and exposes only a boolean match result plus total projection count.

The final production reindex still requires the authoritative source to be write-frozen. This is a convergence verifier, not change-data capture.

## Runtime status

The normal memo search request path is now wired to the shared HIGH-search query lease and route snapshot. The persisted shared route defaults to `legacy`, so user-visible search remains legacy until a separate guarded operator cutover changes that shared state. No cutover command is installed by this request-path change.

When the route snapshot is `protected`, the request path executes the protected blind-token query, then hydrates returned IDs through the owner-scoped authoritative store. It does not silently fall back to the legacy projection on protected-path failure, and it treats query-lease release failure as a failed request. This keeps protected-route faults observable and prevents traffic from escaping a deliberate cutover.

An explicit `HIGH_SEARCH_SHADOW_MODE=observe` mode can schedule the protected query path as a bounded best-effort background observation after a legacy-routed result is already available. Shadow execution is skipped when the admitted user-visible route is protected. A legacy observation also carries the route generation under which its source result was admitted; the shadow task reacquires a query lease and drops the observation before calling the protected reader if the route or generation has changed, so cutover/rollback boundaries cannot contaminate overlap metrics. Shadow work has an explicitly configured concurrency limit and timeout; capacity exhaustion drops the observation instead of queueing unbounded work or backpressuring the user request. Query/tag plaintext, owner identifiers, and result IDs are not logged. Only aggregate counters are retained/logged periodically.

Shadow comparison records aggregate total equality and ephemeral page-set overlap counts. Memo IDs are used only inside the bounded observation task to compute intersection/legacy-only/protected-only counts and are never logged or persisted. When both legacy and protected result sets fit completely on page 1, the observer additionally counts exact complete-set matches/mismatches; for paginated results, page overlap remains diagnostic because the two paths intentionally use different ordering.

These metrics are **not** by themselves a parity or cutover gate: the legacy Manticore query currently uses OR semantics across title/content and sorts by `updated_at desc`, while the protected query uses analyzer-generated blind terms with AND semantics and a deterministic opaque sort key. The representative corpus defines the staged protected semantics, and production cutover criteria must distinguish intentional semantic changes from defects rather than requiring legacy identity.

When HIGH search is enabled, the durable projection outbox mirrors authoritative create/update/delete state into `memos_high_v1`. Reconciliation acquires the shared MongoDB maintenance writer lease before reading the shared search route; route cutover cannot proceed until that lease drains, so the observed route is stable for the complete secondary mutation. While the route is `legacy`, the reconciler maintains both the legacy plaintext projection and the protected projection for staging/shadow use. Once the route is `protected`, it stops all new writes/deletes to the legacy plaintext search projection and updates only the protected projection plus the active memo cache. A protected route with no protected projection sink, or a route-read failure, fails closed and leaves the durable outbox intent unacknowledged for retry. Existing legacy plaintext projection rows remain until a separate explicit purge step.

The staged protected projection also has an operator-only `reindex_high_search_staged` command. It runs behind the MongoDB write-freeze barrier and verifies source/projection convergence, but deliberately performs no route cutover. Each staged full rebuild resets only `memos_high_v1` after the drained shared route is mechanically verified as `legacy`; the maintenance barrier also drains and blocks background reconciler/query work, so reset/rebuild cannot race an outbox retry or admitted query.

Runtime cutover still requires:

- representative-corpus validation and approval of the staged ICU4X analyzer,
- promotion of the staged AWS KMS search-key provider/operator path to the approved production deployment,
- a reviewed `high-search-cutover-approval-v1` artifact bound to the deployment KMS/cache/budget configuration,
- execution of the guarded `cutover_high_search_route` command, which reindexes to convergence and switches the shared route generation only while the maintenance permit is held,
- an explicit search-key rotation protocol for future key-version changes that prevents old/new query gaps,
- an explicit analysis-version migration protocol for future tokenizer/normalizer changes.

Changing either the search-key version or analysis version in place while only one version is queried can make valid documents temporarily undiscoverable. Runtime activation must therefore treat projection generations as a coordinated migration, not as a per-request configuration flip.
