# Distributed Browser Refresh Coordination — Timing Amendment

**Date:** 2026-10-14  
**Status:** review-required  
**Amends:** `docs/superpowers/specs/2026-10-14-distributed-refresh-coordination-design.md`

## Reason for amendment

The approved design correctly notes that the OIDC client applies a 10-second deadline to identity HTTP requests, but it treated refresh as if there were only one such request.

The current `frontend/src/hooks.server.ts` refresh path performs these operations serially:

1. `discoverIdentity(config, fetchFn)`;
2. `refreshAccessToken(metadata, config, fetchFn, ...)`.

`frontend/src/lib/server/identity/oidcClient.ts` applies the 10-second identity HTTP deadline to each request. Therefore a healthy but slow refresh can legitimately spend close to 20 seconds in network I/O before sealing and publishing the new session.

A 15-second leader lease can consequently expire during a valid refresh. That does not permit a second redemption because the persistent attempt marker remains, but it would make followers classify a healthy leader as lost and would prevent reliable result convergence.

## Superseding timing constants

The following values replace the corresponding values in the original design everywhere they appear:

```text
Leader lease TTL:      30000 ms
Follower wait deadline: 25000 ms
Result handoff TTL:    120000 ms  (unchanged)
```

The attempt replay-guard calculation remains unchanged:

```text
max(1000 ms, sessionExpiresAt - now) + 60000 ms
```

## Timing invariant

The timing relationship is now:

```text
per-request identity HTTP deadline = 10000 ms
maximum serial identity HTTP budget = 20000 ms
follower wait deadline              = 25000 ms
leader lease TTL                    = 30000 ms
```

This gives the leader up to 10 seconds beyond the maximum serial HTTP budget for scheduling, JSON parsing, cryptographic sealing/MAC work, and Valkey result publication.

The follower deadline remains strictly shorter than the leader lease, so a follower never interprets its own wait timeout as permission to take leadership.

## Failure semantics remain unchanged

This amendment does not weaken any correctness rule from the original design:

- the attempt marker exists before the upstream token flow begins;
- lease expiry is never permission to retry the old refresh credential;
- an attempt without a usable result and without an active lease is indeterminate;
- follower timeout fails closed for that request and does not cancel the leader;
- publish failure after upstream success is ambiguous and never causes a retry;
- Valkey outage never permits process-local-only refresh in coordination-enabled mode;
- expired, legacy, or indeterminate sessions require re-authentication as specified.

## Required tests added by this amendment

The implementation must include deterministic timing tests that prove:

1. a leader taking slightly more than 15 seconds but less than 30 seconds is not classified as lost solely because the old 15-second value elapsed;
2. a follower can receive a result after more than 20 seconds but before the 25-second follower deadline;
3. a follower reaching 25 seconds fails closed and never becomes leader;
4. a lease reaching 30 seconds without a result becomes indeterminate, with the attempt marker still blocking a retry;
5. no test or production constant retains the superseded `15000 ms` lease or `12000 ms` follower deadline.

## Scope

Only coordination timing is amended. The selected architecture, V2 session schema, HKDF/HMAC domain separation, Valkey key model, atomic scripts, failure handling, deployment stages, and operational trust assumptions remain unchanged.
