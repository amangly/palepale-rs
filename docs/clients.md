# Codex and Claude Code

Palepale can configure Codex and Claude Code to use your Palepale account in API
mode. Install the client first, then run:

```sh
palepale codex
palepale claude
```

These commands work from PowerShell, Bash and Zsh. They sign in to Palepale when
needed, check account access and fetch the gateway's supported models. Setup
does not send an inference request. The gateway must expose `/docs/clients.json`
and the corresponding protocol before setup can succeed.

The commands use your current Palepale model when available, otherwise the first
compatible model. To choose one, run `palepale codex <model-id>` or
`palepale claude <model-id>`. Restart the target app afterward. Requests use your
Palepale account's paid API access and spending limits. Trial credit stays in
the Palepale CLI. The other app's cost display can be an estimate; Palepale's
account usage records determine the charge.

## Configuration

Codex uses `CODEX_HOME`, or `~/.codex`. Setup merges `config.toml`, selects the
Palepale provider and saves `palepale-models.json`. Other providers, project trust,
approval settings and saved subscription credentials remain. An active default
profile and model-specific overrides are cleared so they cannot keep the old
provider or context limits active. Named profiles remain available.

Claude Code uses `CLAUDE_CONFIG_DIR`, or `~/.claude`. Setup merges `settings.json`,
selects the Palepale model and points the built-in model aliases and subagents at
that model. It clears competing provider credentials in the settings environment,
sets context/output limits, and disables extended-thinking and deferred-tool
features that the adapter does not support. Existing permissions, hooks and
unrelated settings remain. Managed or project settings can take precedence;
check `/status` if the client still selects another provider.

Both integrations store an inference credential in `palepale.key` beside the
client's config. Codex's credential command and Claude Code's `apiKeyHelper` read
it through the installed Palepale executable. Keys are absent from the main
settings, model catalog and setup output. Keep Palepale installed at that path;
reconnect after moving it or replacing a revoked key. The credential helper
works without loading project configuration or contacting the API.

Every changed existing file gets a sibling `*.palepale-*.bak` backup. Reconnecting
with unchanged settings makes no new backups. To undo setup, close the client and
restore the config backup. Backups of credential files are private too. Existing
subscription credentials are not deleted.

## Supported behavior

The gateway adapts Responses for Codex and Messages for Claude Code to Palepale's
existing completion service. Text streams incrementally. User images, function
tools, matching tool results, cancellation and usage accounting use the same
admission and billing rules as the native CLI. Codex function namespaces retain
their identities across turns. Tool results currently support text only.

The Responses path is stateless: clients replay history. Stored responses,
`previous_response_id`, hosted tools, strict function-schema guarantees and
verbosity controls are unsupported. Setup disables Codex web search and
WebSockets. Prompt-cache affinity hints are accepted without guaranteeing a cache.

The Messages path supports custom tools and token counting. It does not implement
signed thinking, deferred tool discovery or Anthropic cache-placement guarantees.
Setup lets the model use its default effort. Unsupported requests fail explicitly
before inference. Anthropic does not officially support non-Claude models in
Claude Code; Palepale's adapter compatibility is tested separately.

Local fixture verification used Codex CLI 0.166.0 and Claude Code 2.1.317 on
Windows, including a tool call, its result and the final reply. These checks use
fake models and do not establish live provider quality or production deployment.
Rerun setup to refresh Codex's model catalog after model availability changes.

References: [Codex gateway setup](https://learn.chatgpt.com/docs/enterprise/connect-to-a-gateway),
[Codex protocol](https://learn.chatgpt.com/docs/enterprise/gateway-compatibility),
[Claude Code gateway setup](https://code.claude.com/docs/en/llm-gateway-connect),
[Claude Code protocol](https://code.claude.com/docs/en/llm-gateway-protocol).
