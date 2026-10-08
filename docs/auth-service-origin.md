# Dedicated authentication service origin boundary

The future SvelteKit integration for the dedicated authentication service tracked by #10 and #13 must configure its service base as a strict trusted HTTP(S) origin and reuse `frontend/src/lib/server/serviceOrigin.ts`.

`AUTH_SERVICE_URL` must therefore be an absolute `http://` or `https://` origin with a host and no URL credentials, path prefix, query, fragment, backslash, control character, or ambiguous whitespace. Endpoint paths must be constructed by the server integration rather than embedded in the configured origin.

This document does not define the authentication service's login, session, refresh, or logout protocol. Those contracts remain intentionally deferred until the dedicated service repository and deployment location are chosen.
