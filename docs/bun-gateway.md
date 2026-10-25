# Bun gateway compatibility

The client reads the public model catalog at `/v1/models`, accepts decimal-string retail rates, and uses each model's advertised default output budget. It refuses to select a model marked unavailable. Displayed token-cost estimates are not wallet charges; the gateway's ledger is authoritative, including supplier minimums and rounding.

Set `PALEPALE_API_KEY` to an inference key (`pk_`) issued by the Bun gateway. Owner credentials (`po_`) manage accounts and cannot authorize chat. The client reads its wallet at `/v1/account/summary` and confirmed usage at `/v1/account/usage/summary?days=0`. Both endpoints permit inference keys to read their own account only.

Each model turn sends one completion request with a new `Idempotency-Key`. It never retries automatically, including after a timeout, HTTP 429/503 or a stream that produces no text. The provider may already have accepted a request before the failure reaches the terminal. Check account usage before using `/retry`, which creates a new paid operation.

The client ignores SSE heartbeat comments and treats an error frame as failure even when `[DONE]` follows it. Tool calls run locally under the client's permissions. The gateway only forwards tool definitions and results.

Device pairing, signup, owner key management and server profiles still need the corresponding Bun contract and client migration. Their legacy endpoints are not covered by this compatibility batch. Model tool support and larger context limits must be qualified per provider route before the catalog advertises them.

These changes require matching client and gateway releases. Local fixtures and database tests do not establish live inference or payment acceptance.
