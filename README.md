# watchtower-rs

A low-memory, single-binary web page change detector written in Rust, with a
[changedetection.io](https://github.com/dgtlmoon/changedetection.io)-compatible REST API.

> Status: under development. Tested on Windows 11. The Docker image, a static Linux (musl)
> binary, aarch64 and a real TLS/STARTTLS mail server have **not** been verified yet.

Measured on a Windows release build against a simulated site: 1,000 idle watches use about
17 MB of RSS, and 5,000 watches checked every 300 s peak at about 24 MB. The executable is
about 28 MB. Your numbers will vary.

## Features

- **Compare what matters**: CSS selectors, XPath, JSONPath and jq to pick the target;
  regular expressions, ignored text and exclude selectors to remove noise; trigger text to
  notify only on a match.
- **JavaScript pages**: connect to a separately running Chrome (DevTools WebSocket), run
  click / type / wait steps, and pick elements on a screenshot with the visual selector
  (no JavaScript needed in the admin UI).
- **Price, stock and PDF**: reads structured data (JSON-LD, OpenGraph, microdata) and notifies
  at or below `price_below`; extracts text from PDFs.
- **Notifications**: Discord, Slack, Telegram, ntfy, Gotify, Mattermost, Pushover, email
  (SMTP), JSON webhooks, and an optional Apprise API for everything else. Titles and bodies
  are minijinja templates (per watch, tag or globally). Failed sends are retried.
- **Per-watch fetch options**: headers, method, body, proxy and interval, layered
  watch > tag > global.
- **Operations**: REST API with OpenAPI docs, `/metrics` (Prometheus), an RSS feed,
  backup / restore (CLI and UI), `/healthz`, import of changedetection.io's `url-watches.json`.
- **Security**: SSRF protection (connections to private/loopback/link-local addresses are
  refused unless allow-listed), argon2 password, CSRF tokens, a strict CSP, a Host header
  allow-list against DNS rebinding, and secrets kept out of logs and error messages.

## Build and run

Requires a recent stable Rust toolchain.

```sh
cargo build --release -p watchtower

watchtower set-password     # reads one line from standard input
watchtower api-key          # prints a new REST API key once
watchtower watch add https://example.com/ --interval 300
watchtower run              # scheduler + admin UI on 127.0.0.1:5000
```

Other commands: `watch list|remove`, `check`, `import`, `rss-token`, `backup`, `restore`,
`healthcheck`.

## Configuration

Settings come from `watchtower.toml` (or `--config`; see `watchtower.example.toml`) and `WATCHTOWER_*` environment variables,
which win. The most useful ones:

| Setting | Default | Meaning |
|---|---|---|
| `data_dir` | `data` | SQLite database and snapshots |
| `listen` | `127.0.0.1:5000` | Admin UI / API address |
| `base_url` | `http://localhost:5000` | Used in notification links and the RSS feed; `https://` turns on Secure cookies |
| `allowed_hosts` | `[]` | Extra Host names accepted by the UI and API (`localhost`, IP addresses and the `base_url` host are always accepted) |
| `allowed_ips` | `[]` | Private addresses that watches may fetch (SSRF exception) |
| `browser_ws_url` | none | Chrome DevTools WebSocket, needed for browser fetching and the visual selector |
| `notify_urls` | `[]` | Notification targets applied to every watch |
| `apprise_api_url` | none | Apprise API server for services not implemented natively |

List values are JSON in environment variables, e.g. `WATCHTOWER_ALLOWED_HOSTS='["watch.example.org"]'`.

## Docker

A `Dockerfile` (musl static binary on distroless) and `docker-compose.yml` are included. See the
comments in `docker-compose.yml` first: the admin UI is protected by a password only, so put it
behind HTTPS and do not expose the port directly.

## Development

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```

Tests that need a real Chrome are marked `#[ignore]`; set `WATCHTOWER_TEST_CHROME_WS` to the
browser's DevTools WebSocket URL and run them with `--ignored --test-threads=1` (see the header of
`crates/fetch/tests/cdp_real.rs`). The workspace is split into
`core`, `process`, `store`, `fetch`, `scheduler`, `notify`, `web`, `api` and `metrics`
crates plus the `bin/watchtower` binary.

## License

Apache License 2.0. See [LICENSE](LICENSE).

---

## 日本語

watchtower-rs は、Rust で書いた省メモリ・単一バイナリの Web ページ変更検知ツールです。
changedetection.io 互換の REST API を持ちます。開発中で、Windows 11 で動作を確かめています。
Docker イメージ、Linux の静的バイナリ（musl）、aarch64、TLS/STARTTLS の実メールサーバーは
未検証です。

メモリは、Windows のリリースビルドを模擬サイトに対して測ったところ、1,000 件の待機で約 17 MB、
5,000 件を 300 秒間隔で監視したときのピークが約 24 MB でした（環境によって変わります）。

使い方は上の「Build and run」と「Configuration」を参照してください。ライセンスは Apache-2.0 です。
