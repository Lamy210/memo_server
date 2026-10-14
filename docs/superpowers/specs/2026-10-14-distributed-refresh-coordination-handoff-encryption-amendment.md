# Distributed Refresh Coordination Handoff Encryption Amendment

**Date:** 2026-10-14  
**Status:** corrective security amendment  
**Scope:** result handoff confidentiality in the shared refresh coordinator  
**Supersedes:** the `Authenticated result envelope` representation in `2026-10-14-distributed-refresh-coordination-design.md`

## Reason for the amendment

The approved design contained two incompatible statements:

1. the Valkey result value was described as the exact already-sealed browser session plus a context-binding MAC;
2. Valkey key/value state was required not to contain a raw browser cookie.

The sealed SvelteKit session is itself the browser cookie value. Even though that value is AES-GCM protected internally, copying it unchanged into Valkey would let any principal that can read the coordination store replay that value as a valid browser session cookie. A MAC protects integrity and generation binding, but it does not remove that replay capability.

The implementation therefore keeps the existing MAC requirement and adds a second coordination-layer encryption envelope around the sealed browser session before it is written to Valkey.

## Corrected result envelope

Derive a third coordination-only key from `IDENTITY_SESSION_KEY` with HKDF-SHA-256:

```text
memo_server/identity-refresh/v1/result-encryption
```

The existing derivation domains remain:

```text
memo_server/identity-refresh/v1/key-id
memo_server/identity-refresh/v1/result-mac
```

For refreshed sealed browser session `S1`, generate a fresh 96-bit nonce and encrypt `S1` with AES-256-GCM using the `result-encryption` key.

Additional authenticated data is exactly:

```text
"v1\0" || coordination_id
```

Valkey stores only:

```ts
type RefreshResultEnvelope = {
  version: 1;
  nonce: string;       // unpadded base64url
  ciphertext: string;  // AES-GCM ciphertext + tag, unpadded base64url
  mac: string;         // lowercase hex HMAC-SHA-256
};
```

The MAC input is exactly:

```text
"v1\0" || coordination_id || "\0" || nonce || "\0" || ciphertext
```

Followers MUST verify the HMAC with Web Crypto before decrypting the ciphertext. AES-GCM decryption MUST use the same coordination identifier as additional authenticated data. Any malformed envelope, MAC failure, AES-GCM authentication failure, cross-generation copy, or invalid decrypted session fails closed.

## Security properties

This amendment preserves all prior coordination invariants and adds confidentiality against a read-only Valkey compromise:

- Valkey does not receive the replayable browser-cookie value;
- Valkey does not receive plaintext access or refresh credentials;
- result ciphertext is bound to one coordination generation by both the HMAC context and AES-GCM AAD;
- the encryption key and MAC key are independently HKDF-domain-separated;
- each publication uses a fresh nonce, so publishing the same S1 twice does not create the same Valkey value;
- a Valkey reader cannot turn the stored result value directly into `__Host-schnee_session`.

This does not make Valkey untrusted for availability or key retention. Deletion or premature eviction of the persistent attempt marker can still violate the no-retry invariant, so the no-eviction and single-authoritative-write requirements remain unchanged.

## Result freshness admission

A cryptographically valid handoff is not sufficient if its access token has already expired while the result was waiting in Valkey.

When a follower decrypts and opens S1 it MUST require:

```text
S1.sessionExpiresAt == S0.sessionExpiresAt
S1.accessTokenExpiresAt > now
```

If either condition fails, the browser session is cleared and S0 is not redeemed again. This prevents a 120-second result handoff TTL from causing an already-expired access token to be adopted.

## Verification requirements

The dedicated refresh-coordination gate must cover all of the following:

- the serialized Valkey envelope does not contain the S1 browser-cookie value or a `sealedSession` field;
- independent reference HKDF/AES-GCM decryption succeeds only with the `result-encryption` domain and the original coordination identifier;
- repeated encryption of the same S1 uses fresh nonces;
- ciphertext or MAC tampering fails closed;
- cross-generation result substitution fails closed;
- an authenticated but access-token-expired S1 is rejected without upstream refresh;
- the existing two-replica real-Valkey test still performs exactly one upstream redemption and both replicas converge on the same S1.

## Unchanged behavior

This amendment does not change:

- the 30-second leader lease;
- the 25-second follower deadline;
- the 120-second result TTL;
- V2 fixed `sessionExpiresAt`;
- attempt replay-guard retention;
- provider-neutral OIDC/JWT/JWKS boundaries;
- the one-active-replica production restriction before a real deployment canary succeeds.
