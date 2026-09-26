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
