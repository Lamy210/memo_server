# HIGH Search Workload Measurement Runbook

Status: local aggregate-only sizing tool; production budget selection remains operator-reviewed.

## Purpose

`measure_high_search_workload` measures analyzer work for a local sanitized or generated memo/query corpus before SEARCH-HIGH-1 cutover.

It does not contact AWS KMS, Manticore, MongoDB, or the application HTTP API. It outputs only aggregate counts and distributions. It must not be used to export raw production content.

## Input

The input is a local JSON file:

```json
{
  "documents": [
    {
      "title": "Synthetic title",
      "content": "Synthetic content",
      "tags": ["Synthetic tag"]
    }
  ],
  "queries": [
    {
      "query": "synthetic query",
      "tag": null
    }
  ]
}
```

Use repository-owned generated data or a separately approved sanitized corpus. Do not commit production/user content to the repository.

The command rejects files larger than 16 MiB. The measurement library also caps the corpus at 10,000 documents and 10,000 queries.

## Run

From `backend/`:

```bash
cargo run --locked --bin measure_high_search_workload -- \
  --input /path/to/approved-corpus.json
```

The command prints JSON containing only:

- analyzer version,
- document/query counts,
- counts of zero-content-term documents/queries,
- count of tagged queries,
- document content-term distribution,
- document tag-count distribution,
- query content-term distribution,
- normalized-term byte-length distribution.

Each distribution reports sample count, min, nearest-rank p50, nearest-rank p95, and max.

The report intentionally contains no input title/content/tag/query strings, memo IDs, owner IDs, normalized terms, blind tokens, or key material.

## Budget selection

Use the report together with load testing to choose explicit deployment values for:

- `HIGH_SEARCH_MAX_DOCUMENT_CONTENT_TERMS`,
- `HIGH_SEARCH_MAX_QUERY_CONTENT_TERMS`,
- `HIGH_SEARCH_MAX_NORMALIZED_TERM_BYTES`.

Do not set these directly to a single corpus maximum without considering headroom, rejection policy, KMS/HMAC cost, projection size, and expected growth. Record the reviewed values in deployment configuration and retain only the aggregate report where policy permits.

The committed representative corpus is a semantic CI gate. This measurement tool is the sizing mechanism for production-like sanitized/generated workloads. Neither one alone authorizes request-path cutover.


## Reviewed approval manifest

Measurement output does not authorize deployment by itself. After the corpus and projection load test have been reviewed, record the result in a separate approval JSON artifact using schema `high-search-workload-approval-v1`.

Example shape:

```json
{
  "schema_version": "high-search-workload-approval-v1",
  "approval_id": "prod-like-2026-09-27",
  "workload_source": "sanitized_production_like",
  "production_like_corpus_reviewed": true,
  "projection_load_test_completed": true,
  "measurement": {
    "analysis_version": "icu4x-2.3.0-uax29-17-nfkc-fold-dict-v1",
    "documents": 1000,
    "queries": 1000,
    "documents_with_zero_content_terms": 0,
    "queries_with_zero_content_terms": 2,
    "queries_with_tag": 200,
    "document_content_terms": {"samples":1000,"min":1,"p50":8,"p95":40,"max":70},
    "document_tag_terms": {"samples":1000,"min":0,"p50":2,"p95":5,"max":8},
    "query_content_terms": {"samples":1000,"min":0,"p50":2,"p95":8,"max":12},
    "normalized_term_bytes": {"samples":20000,"min":1,"p50":6,"p95":24,"max":64}
  },
  "selected_budgets": {
    "max_document_content_terms": 128,
    "max_query_content_terms": 32,
    "max_normalized_term_bytes": 128
  }
}
```

The selected budgets must be positive and must not be lower than the measured maxima. The manifest validator also requires the analyzer version to equal the build's current HIGH-search analyzer version.

Validate the artifact alone:

```bash
cargo run --locked --bin validate_high_search_workload_approval -- \
  --input /path/to/approval.json
```

Validate it against the deployment environment as well:

```bash
cargo run --locked --bin validate_high_search_workload_approval -- \
  --input /path/to/approval.json \
  --against-env
```

`--against-env` parses the normal application configuration and requires the reviewed `HIGH_SEARCH_MAX_DOCUMENT_CONTENT_TERMS`, `HIGH_SEARCH_MAX_QUERY_CONTENT_TERMS`, and `HIGH_SEARCH_MAX_NORMALIZED_TERM_BYTES` values to match the manifest exactly.

The approval file contains aggregate measurements only and should still be handled as deployment-control metadata. It is an operator attestation, not a cryptographic signature and not proof that the input corpus was sanitized correctly. Existing corpus governance and review remain required. A valid approval manifest is a prerequisite artifact for a future protected-query cutover gate; it does not authorize cutover by itself.
