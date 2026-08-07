# AI Usage Widget

A small cross-platform tray/menu-bar widget (built with [Tauri v2](https://v2.tauri.app)) that shows, at a glance, how much of your usage quota you've burned on **Claude Code**, **Codex**, and **Cursor** — so you can tell which one still has headroom right now.

Click the tray icon to open the popover. It polls every 5 minutes in the background; opening the popover also triggers an immediate refresh.

## How each integration works, and why

Every tool needs both a live "source of truth" number and a fast local cache, since usage can be shared across surfaces (e.g. Claude Code and claude.ai share one pool) and local logs alone can't see multi-device usage or the server's actual reset clock. **No custom OAuth is implemented anywhere in this app** — it always reuses each tool's own sanctioned login and reads the credential file/keychain entry that tool's own CLI already writes.

### Claude Code — stable

- **Credential**, in the same priority order Claude Code CLI itself uses: the `CLAUDE_CODE_OAUTH_TOKEN` environment variable (a long-lived token from `claude setup-token`) first, then macOS Keychain service `"Claude Code-credentials"` (read via the `security` CLI), then `~/.claude/.credentials.json` on Linux/Windows (respects `CLAUDE_CONFIG_DIR`). The env var only has any effect when the widget is launched from a shell that has it exported — a normal double-click/Finder launch doesn't inherit shell environment variables, a macOS limitation this app can't work around. This is specifically the Claude Code CLI's own session — it's unrelated to, and can't see, a separate Claude Desktop (claude.ai chat app) login; Anthropic keeps those credentials in genuinely separate stores, and Claude Desktop's is deliberately locked to its own signed binary so no other app (including this one) can read it.
- **Live call**: `GET https://api.anthropic.com/api/oauth/usage` — undocumented but the same endpoint the CLI itself calls, with the `anthropic-beta: oauth-2025-04-20` header.
- **Token refresh**: Claude Code's access tokens are short-lived and the CLI normally refreshes its own silently whenever it runs — but nothing triggers that if this widget is the only thing reading the credential, so a token can go stale between actual CLI uses. To avoid nagging you to `claude login` for what's really just an expired short-lived token, the widget refreshes it itself using the refresh token already in the credential, via Claude Code CLI's own public OAuth client ID against `POST https://platform.claude.com/v1/oauth/token` (`grant_type=refresh_token`) — the same narrow "extend an already-granted session" operation the CLI itself performs, not a new interactive login. The refreshed token is used in memory only for that fetch; it's never written back to Claude Code's own credential store.
- **Log-in**: the widget shells out to `claude login` (opens your browser; Claude Code writes its own credential afterward). The widget never touches your password or handles the initial interactive OAuth grant itself.
- **Offline fallback**: if there's no credential yet and no cached snapshot, it scans recent `~/.claude/projects/**/*.jsonl` session transcripts and turns recent request counts into a rough percentage against fixed baselines (50 requests ≈ 100% of the 5h window, 250 ≈ 100% of the week). This is **not** a real quota calculation — it's a best-effort estimate only used before the first successful live fetch, and is labeled "(est.)" in the UI.

### Codex — stable

- **Credential**: `~/.codex/auth.json` (respects `CODEX_HOME`), specifically `tokens.access_token` / `tokens.account_id`.
- **Live call**: `GET https://chatgpt.com/backend-api/wham/usage` with `ChatGPT-Account-Id` when available — the same undocumented endpoint the Codex CLI/TUI's `/status` view is backed by.
- **Log-in**: shells out to `codex login`, same pattern as Claude Code.

### Cursor — fragile, by far the most likely thing to break silently

Cursor has no unified login flow to shell out to. This integration tries **two independent local sessions automatically** — whichever is present and working wins, so it works whether you use the Cursor IDE, the `cursor-agent` CLI, or both, with no configuration needed.

**1. Cursor IDE**, via two things Cursor itself owns and was never meant for third parties to read:
   - **`state.vscdb`** — Cursor's own VS Code-style SQLite key/value store (`~/Library/Application Support/Cursor/User/globalStorage/state.vscdb` on macOS; `%APPDATA%\Cursor\User\globalStorage\state.vscdb` on Windows; `$XDG_CONFIG_HOME/Cursor/User/globalStorage/state.vscdb` on Linux). It's opened **read-only** and is safe to read alongside a running Cursor — Cursor doesn't lock it. The widget runs `SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken' LIMIT 1;`.
   - That access token is a JWT. The widget decodes its `sub` claim (no signature verification — it's not being trusted as a security boundary, just reused as Cursor's own already-logged-in session) to get a user ID, then derives Cursor's **first-party web session cookie**: `Cookie: WorkosCursorSessionToken={userID}::{accessToken}`. That cookie — not the raw token — is what's sent to `GET https://cursor.com/api/usage-summary`, an undocumented, unversioned endpoint.

**2. `cursor-agent` CLI** (`agent login` / `CURSOR_API_KEY`), which keeps a completely separate, opaque `crsr_...` API key — not a JWT — at one of `~/.config/cursor/auth.json`, `~/.config/cursor/credentials.json`, or `~/.cursor/credentials.json` depending on version/platform. **None of this is documented by Cursor** — the paths and field names come from community reverse-engineering (an strace against the actual binary), not an official source, so treat this path as more experimental than the IDE one. The widget tries this key as a bearer token (`Authorization: Bearer crsr_...`) against the same `usage-summary` endpoint; Cursor doesn't document a usage API for personal API keys, so this may simply not be authorized on any given account — if so, it fails silently and falls back to whatever the IDE source produces (or to the auth-expired state if neither works).

Any of these paths, key names, or endpoints could change without notice on a Cursor update, silently breaking this integration. If nothing is found, a token fails to decode, or every API call fails, the widget shows an explicit **"Cursor auth expired — reopen Cursor to refresh"** state rather than a stale or fabricated number — there's no refresh-token flow available to drive this programmatically, so reopening Cursor (or re-running `agent login`) is the actual fix.

Cursor has no short rolling window like Claude/Codex — just a monthly reset, shown as a single bar. If your plan has an on-demand/overage pool and you've spent from it, that shows as a small secondary line under the bar rather than a second bar, to keep the glance-read simple.

## Storage & privacy

- This app never writes any credential or token to disk itself. Each provider re-reads its source of truth (keychain / credential file / Cursor's own local database) on every refresh instead of caching a token locally.
- The only thing persisted to disk is the last-known-good *usage snapshot* (percentages and reset labels, no tokens) at `<app data dir>/usage-cache.json`, so the popover has something to show instantly on launch before the first live fetch completes.
- Polling runs every 5 minutes — this is a glance tool, not a real-time dashboard.

## Development

```bash
npm install
npm run tauri dev
```

Requires the Rust toolchain (`rustup`) in addition to Node.

## Out of scope for v1

- Burn-rate/pace indicators — plain % + reset date only.
- Custom OAuth app registrations with Anthropic/OpenAI/Cursor (none currently offer one for third parties).
- Windows/Linux tray polish beyond what's implemented — flagged for follow-up as encountered, not pre-solved.
