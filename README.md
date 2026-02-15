# saelora

## Run

```sh
cargo run
```

If your shell can't find `cargo`, use:

```sh
./scripts/run
```

Or fix your shell PATH for Rustup:

```sh
source "$HOME/.cargo/env"
```

Flags:

```sh
cargo run -- --addr 127.0.0.1:8080 --data-dir ./data
cargo run -- --headless
```

## Web UI

The website (`/`) and minimal chat app (`/app/`) are served by the Rust binary and embedded at build time.

Source files live in:
- `web/site/`
- `web/app/`

## TUI (admin)

Launch without flags to open the TUI:

- **OpenRouter config**: API key, model, base URL, headers.
- **System Prompt**: edit the server-side prompt (full-width textarea).
- **Mailjet**: API key/secret, from email/name, optional base URL for the Mailjet send endpoint.
- **Invites (pending/whitelist)**: approve/remove requests; updates live without restart.
- **Users**: list and change status.

Ctrl+C quits; Esc backs out of a screen; Ctrl+S saves in config screens.

## Auth flow (API)

- `POST /invite` — add email to waitlist.
- `POST /v1/auth/register` — create account if the email is whitelisted (email+password).
- `POST /v1/auth/login` — password login.
- `POST /v1/auth/request-password-link` — emails a setup/reset link if the email is eligible (response always returns `{ok:true}` to avoid leaking eligibility).
- `POST /v1/auth/setup` — consumes setup token, sets password, returns session token (new accounts require whitelist).
- `GET /v1/auth/me`, `POST /v1/auth/logout`.

## Agents & models

- Configure multiple agents (OpenRouter or Ollama) in the TUI “Agents” screen.
- Assign different agents per task (chat, summary). Chat API uses the configured chat agent.

## Chat API

- `POST /v1/chat/completions` (OpenAI-compatible; supports SSE streaming)

## Health

- `GET /healthz`
