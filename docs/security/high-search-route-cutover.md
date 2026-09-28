# HIGH Search Route Cutover Runbook

Status: operator-only guarded cutover/rollback. The shared route defaults to `legacy`.

## Purpose

`cutover_high_search_route` is the only repository-owned operator path that may switch the shared user-visible HIGH-search route.

Protected cutover performs all of the following while the MongoDB maintenance barrier is held:

1. validates the reviewed cutover approval against the current HIGH-search environment,
2. drains foreground memo mutations, background projection reconciliation, and admitted HIGH-search queries,
3. mechanically requires the shared route to still be `legacy`,
4. clears derived-key cache state,
5. resets and fully rebuilds `memos_high_v1`,
6. verifies authoritative/projection convergence,
7. compares the operator-observed route generation,
8. CAS-switches `legacy -> protected`,
9. rereads the route while maintenance is still held,
10. releases the barrier only after the exact protected target generation is confirmed.

There is no protected-to-legacy fallback inside the HTTP request path. Rollback is a separate operator action.

## Cutover approval

Protected cutover requires a `high-search-cutover-approval-v1` JSON artifact. It embeds the reviewed workload approval and binds the approval to the exact deployment configuration:

- KMS key ARN,
- AWS Region,
- provider seed version,
- derived-key cache TTL,
- derived-key cache max entries,
- cache sweep interval,
- HIGH-search term/byte budgets.

The artifact also requires explicit attestations for:

- KMS key/IAM review,
- credential-provider validation,
- cache-capacity review,
- rotation/reindex rehearsal,
- rollback rehearsal,
- security migration gate review,
- all application replicas being route-aware and participating in the shared maintenance barrier,
- all replica HIGH-search runtime settings having been reviewed.

The artifact is an operator attestation, not a cryptographic signature.

## Validate approval before change

Validate the JSON structure only:

```bash
cargo run --locked --bin validate_high_search_cutover_approval -- \
  --input /path/to/high-search-cutover-approval.json
```

Validate it against the current deployment environment:

```bash
cargo run --locked --bin validate_high_search_cutover_approval -- \
  --input /path/to/high-search-cutover-approval.json \
  --against-env
```

No MongoDB, Manticore, or KMS network operation is performed by the validator.

## Inspect current route

```bash
cargo run --locked --bin cutover_high_search_route -- --status
```

Record both fields immediately before the change:

```text
current.route=legacy
current.generation=7
```

Do not reuse a route generation captured before a deployment, maintenance operation, recovery, prior cutover, or rollback.

## Protected cutover

Example:

```bash
cargo run --locked --bin cutover_high_search_route -- \
  --apply-protected \
  --confirm-protected-cutover \
  --approval /path/to/high-search-cutover-approval.json \
  --page-size 500 \
  --expected-route-generation 7
```

Expected successful transition:

```text
cutover.previous.route=legacy
cutover.previous.generation=7
cutover.current.route=protected
cutover.current.generation=8
```

The command performs a fresh protected projection reset/reindex while the route is drained and still legacy. A prior staging run is not treated as sufficient proof for the final cutover.

## Rollback

Rollback deliberately does not require the cutover approval artifact. It must remain available when protected dependencies are unhealthy, but it must not route traffic to a stale plaintext projection.

While the protected route is active, background reconciliation no longer writes to the legacy plaintext Manticore table. Rollback therefore runs a bounded full legacy rebuild under the same maintenance barrier before switching the route:

1. require the operator-observed search route generation,
2. drain writers, queries, and memo data-path requests,
3. require the MEMO data route to still be `legacy_plaintext`,
4. `TRUNCATE TABLE memos` while traffic remains drained,
5. rebuild every row from the plaintext authoritative MongoDB source,
6. verify source count, visited count, and legacy projection count converge,
7. revalidate search and memo route snapshots,
8. CAS-switch `protected -> legacy`,
9. release the maintenance barrier.

Inspect the current generation first, then run:

```bash
cargo run --locked --bin cutover_high_search_route -- \
  --apply-legacy \
  --confirm-legacy-rollback \
  --page-size 500 \
  --expected-route-generation 8
```

Expected transition:

```text
rollback.previous.route=protected
rollback.previous.generation=8
rollback.rebuild.source_count=...
rollback.rebuild.projected_visited=...
rollback.rebuild.projection_count=...
rollback.current.route=legacy
rollback.current.generation=9
```

If the search route is already `legacy` at the exact expected generation, the explicit rollback command still rebuilds and verifies the legacy projection, then leaves the route generation unchanged.

Rollback is intentionally rejected when the MEMO data route is `encrypted`. Once encrypted authoritative data can diverge from the plaintext MongoDB source, rebuilding legacy search from that plaintext source would be unsafe; reverse synchronization/decryption-driven rollback must be implemented before that state can support legacy search rollback.

## Ambiguous switch outcome

The route update and route reread are both performed while the maintenance permit is held.

If the operator cannot prove that the persisted route is exactly the intended target generation, the command returns failure **without explicitly releasing the maintenance barrier**. This is intentional fail-closed behavior.

In that state:

1. do not restart or bypass traffic manually,
2. run the maintenance recovery status command,
3. inspect mode, writer epoch, route, route generation, active leases, and holder token,
4. determine the actual persisted route,
5. follow the maintenance recovery runbook using the exact observed snapshot.

Do not assume that a CLI/network error means the route update did not commit.

## Post-cutover checks

After a successful protected cutover:

1. inspect the route and confirm the expected protected generation,
2. run controlled authenticated searches covering English, Japanese, tag-only, and multi-term AND semantics,
3. verify protected reader/KMS/Manticore health,
4. verify no unexpected maintenance lease accumulation,
5. retain the exact prior generation and rollback procedure for incident response.

After rollback, verify user-visible searches are served by the legacy route and investigate the protected failure before attempting another cutover.
