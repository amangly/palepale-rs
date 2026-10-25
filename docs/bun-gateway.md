# Bun gateway compatibility

The client reads the public model catalog at `/v1/models`, accepts decimal-string retail rates, and uses each model's advertised default output budget. It refuses to select a model marked unavailable. Displayed token-cost estimates are not wallet charges; the gateway's ledger is authoritative, including supplier minimums and rounding.

Set `PALEPALE_API_KEY` to an inference key (`pk_`) issued by the Bun gateway. Owner credentials (`po_`) manage accounts and cannot authorize chat. The client reads its wallet at `/v1/account/summary` and confirmed usage at `/v1/account/usage/summary?days=0`. Both endpoints permit inference keys to read their own account only.

Each model turn sends one completion request with a new `Idempotency-Key`. It never retries automatically, including after a timeout, HTTP 429/503 or a stream that produces no text. The provider may already have accepted a request before the failure reaches the terminal. Check account usage before using `/retry`, which creates a new paid operation.

The client ignores SSE heartbeat comments and treats an error frame as failure even when `[DONE]` follows it. Tool calls run locally under the client's permissions. The gateway only forwards tool definitions and results.

Device login starts at `POST /v1/pair/start` and polls `POST /v1/pair/poll`, keeping the device code in the body. The client follows the server's polling interval and expiry. After owner approval it collects an inference key once and saves it in the existing protected local credential store. The website's `/pair` page previews the requesting device and accepts approval or denial through an expiring browser session. These changes require matching backend and website releases.

Run `palepale signup` to create an account through `POST /v1/accounts`. It saves the owner credential in `owner.json`, then issues an inference key through `POST /v1/account/keys` and saves it in `auth.json`. Neither secret is printed. Windows encrypts both files with DPAPI; other platforms use owner-only file permissions. Both files are bound to the configured API origin. Protect the owner file as account recovery material; a Windows copy is tied to the Windows user who created it. To sign in on the website or make a portable password-manager backup, run `palepale keys export-owner <file> --yes` from the command line. Choose a private destination outside your repository. The command creates a plaintext file, refuses to overwrite existing files, and never prints the credential. Delete the export after storing or using it.

`palepale keys list`, `palepale keys create <label> --use`, and `palepale keys revoke <id> --yes` use the saved owner credential or `PALEPALE_OWNER_CREDENTIAL`. The owner environment variable takes precedence for management only. Normal model requests use `PALEPALE_API_KEY` or `auth.json`. The create command replaces the local inference key; it does not revoke older keys. Inspect and revoke unused keys explicitly.

Signup never overwrites a saved owner credential, including with `--force`. If client key setup fails after account creation, the owner file remains available: inspect `keys list` and run `keys create <label> --use` to connect. A failed signup response is not retried automatically. `logout` removes only `auth.json` and retains the owner file. Use a separate `PALEPALE_HOME` for another account.

Server profiles still need client migration. Model tool support and larger context limits must be qualified per provider route before the catalog advertises them.

These changes require matching client and gateway releases. Local fixtures and database tests do not establish live inference or payment acceptance.
