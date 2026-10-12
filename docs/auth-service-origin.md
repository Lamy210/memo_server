# Provider-neutral identity origin boundary

The SvelteKit browser authentication boundary is configured with the provider-neutral `IDENTITY_ISSUER` setting. The application contract does not use a provider-specific service URL or private session API.

`IDENTITY_ISSUER` must be an absolute HTTPS issuer URL with a host and no URL credentials, query, fragment, or ambiguous surrounding whitespace. The configured issuer identifier is retained exactly for comparison with the issuer returned by OIDC discovery.

OIDC discovery is resolved from the configured issuer. Standards-declared authorization, token, and optional end-session endpoints may use different origins, but each endpoint must also use HTTPS and must not contain URL credentials or fragments. Identity discovery and token requests do not follow redirects and use a bounded request deadline.

The browser callback URI is derived from the memo application's own request origin as `/auth/callback`; it is not taken from identity-provider metadata or browser-controlled redirect input. Post-login and logout return targets are separately restricted to safe internal application paths.

Local application development remains independent of production identity infrastructure through `AUTH_MODE=development` and server-private `DEVELOPMENT_USER_ID`. The dedicated provider-neutral browser-auth E2E fixture uses ephemeral TLS so the production identity configuration path is tested without adding an HTTP issuer exception to runtime code.

See `docs/authentication.md` for the complete resource-server and browser-authentication contract and `docs/browser-auth-operations.md` for the current refresh-coordination scaling constraint.