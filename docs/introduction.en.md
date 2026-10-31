# watchtower-rs

**Quietly watch pages for changes.**

watchtower-rs checks web pages on a schedule and notifies you of what changed.
It has a REST API compatible with [changedetection.io](https://github.com/dgtlmoon/changedetection.io) and runs as a single executable written in Rust.
Low memory use was the first design goal.

## Who it is for

- You do not want to miss an update to a price, a stock status, an announcement, terms or a public document.
- You want to watch many pages on a small server or an old PC.
- You want a lighter implementation behind an existing integration that uses the changedetection.io API.

## Memory use

Measured on a Windows release build against a simulated site of our own. Your numbers will vary.

| Situation | Memory (RSS) |
|---|---|
| 1,000 watches registered and idle | about 17 MB |
| Peak with 5,000 watches checked every 300 s | about 24 MB |

The executable is about 28 MB.

## Features

- **Compare what matters**: pick the target with CSS selectors, XPath, JSONPath or jq. Remove noise with regular expressions and ignored text, and notify only when a trigger text matches.
- **JavaScript pages**: connect to a separately running Chrome (DevTools WebSocket) and fetch the rendered page. Register click, type and wait steps, or pick an element on a screenshot with the visual selector.
- **Price, stock and PDF**: read prices from structured data on product pages (JSON-LD, OpenGraph, microdata) and notify at or below a threshold. Compare the text of PDFs too.
- **Many notification targets**: Discord, Slack, Telegram, ntfy, Gotify, Mattermost, Pushover, email (SMTP) and JSON webhooks. Anything else is forwarded to an Apprise API. Titles and bodies are templates, and failed sends are retried.
- **Per-watch fetch options**: set headers, method, body, proxy and interval per watch, per tag or globally. The closest setting wins.
- **Dashboard**: see the number of watches, the failing ones, the notification queue and the latest changes on one page.
- **One-command start**: `watchtower ui` starts the server and opens the admin page in your browser.
- **Easy to operate**: OpenAPI docs, Prometheus `/metrics`, an RSS feed, backup and restore, and `/healthz`. You can import changedetection.io's `url-watches.json`. Docker files are included.

## Security

- SSRF protection that refuses connections to internal addresses (private, loopback, link-local) by default
- Passwords stored with argon2, CSRF protection and a strict CSP
- A Host header allow-list against DNS rebinding
- Header values and notification secrets are kept out of logs and error messages

The admin UI is protected by a password only. Put it behind an HTTPS reverse proxy before exposing it.

## Quick start

```sh
watchtower set-password     # admin password (one line from standard input)
watchtower api-key          # issue a REST API key
watchtower watch add https://example.com/ --interval 300
watchtower ui               # start and open the admin page in a browser
```

Use `watchtower run` to keep it running without opening a page. The admin UI listens on `127.0.0.1:5000` by default.
Configure it with `watchtower.toml` (see `watchtower.example.toml`) or `WATCHTOWER_*` environment variables.

## Get it

- **Windows 11 (64-bit)**: extract the zip and run `watchtower.exe`. There is no installer.
- **Linux (x86_64 and aarch64)**: statically linked binaries are on the GitHub releases page.
- **Source code**: https://github.com/taotaro1104-commits/watchtower-rs
- **License**: Apache-2.0

## Status

In development (v0.1.0, pre-release). On Windows 11 it has been run as a real binary and driven through the browser.
These have not been verified yet:

- The Docker image (it builds in CI, but has not been run)
- Static Linux (musl) and aarch64 binaries (they build, but have not been run)
- Sending through a real mail server with TLS or STARTTLS
