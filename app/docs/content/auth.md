# Authentication

`eth-tools` uses opaque, scoped API keys issued through the dashboard.

## Issuing a key

1. Sign in at [`/dashboard/keys`](/dashboard/keys) with GitHub.
2. Click **Create key**, give it a label (e.g. "claude-dev"), and pick scopes
   (`read` is the default; `write` is gated behind paid-plan rails).
3. Copy the key — it's shown **once**. The dashboard stores only an
   Argon2id hash plus a four-character prefix so we can recognise the
   key in logs without storing it.

## Using a key

Pass it as a Bearer token on every request:

```
authorization: Bearer etk_…
```

Keys are checked at the edge against the `api_keys` table; revoked or
expired keys return `401`. Per-key rate limits are enforced at the API
gateway and surfaced in the response headers (`x-ratelimit-remaining`).
