# Browser authentication operations

## Scope

This document records deployment constraints for the provider-neutral SvelteKit browser authentication boundary. It complements `docs/authentication.md`; it does not change the OAuth/OIDC/JWT contract used by `memo_server`.

## Refresh coordination

The current BFF keeps the sealed browser session in a `Secure` + `HttpOnly` cookie and performs refresh-token rotation from the server-side request hook. Concurrent requests carrying the same sealed session are coalesced with a process-local single-flight map so one SvelteKit process does not redeem the same refresh token more than once at the same time.

That coordination is intentionally process-local. It does **not** coordinate refresh work between multiple SvelteKit processes or replicas.

For identity providers that rotate refresh tokens or reject refresh-token reuse, the currently supported production topology is therefore **one active SvelteKit BFF replica for browser-session traffic**. Running multiple replicas without additional coordination can allow two replicas to redeem the same refresh token concurrently; one request may rotate the credential while another receives `invalid_grant`, causing the latter request to invalidate its browser session.

Sticky routing can reduce the probability of this race but is not a portable correctness guarantee because failover, deployment replacement, or routing changes can move concurrent requests between replicas.

## Requirement before horizontal scaling

Before browser-session traffic is horizontally scaled beyond one active BFF replica, implement and verify one of these coordination models:

1. a shared server-side session store with atomic compare-and-swap or lease semantics for refresh rotation;
2. a distributed single-flight/lock keyed by an opaque session identifier, with bounded lock lifetime and fail-closed recovery semantics; or
3. an explicitly documented identity-provider refresh-rotation grace/reuse contract that is tested under concurrent multi-replica refresh and accepted as part of the deployment contract.

Provider-specific grace behavior is not part of the provider-neutral application contract, so options 1 or 2 are preferred for portable horizontal scaling.

## Verification expectations

A future multi-replica implementation must add deterministic tests covering at least:

- two replicas receiving the same near-expiry sealed session concurrently;
- exactly one logical refresh redemption when strict rotation is enabled;
- refresh-token rotation propagation to subsequent requests;
- lock/session-store timeout and failure behavior;
- replica termination or failover while refresh coordination is in progress;
- no fallback to an expired access token after refresh failure;
- no browser exposure of access or refresh credentials.

Until those tests and coordination semantics exist, keep browser-session BFF traffic on one active replica.