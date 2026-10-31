# watchtower-rs

**ページの変化を、静かに見張る。**

watchtower-rs は、Web ページの変化を定期的に確かめ、差分を通知する監視ツールです。
[changedetection.io](https://github.com/dgtlmoon/changedetection.io) と互換の REST API を持ち、Rust で書いた 1 つの実行ファイルで動きます。
メモリの少なさを一番に考えて作っています。

## こんな方に

- 商品の価格や在庫、お知らせ、規約、公開資料の更新を見逃したくない
- 小さなサーバーや古い PC でも、たくさんのページを見張りたい
- changedetection.io の API を使った既存の連携を、軽い実装に置き換えたい

## メモリの使用量

Windows のリリースビルドを、自作の模擬サイトに対して測った値です。環境によって変わります。

| 状況 | メモリ（RSS） |
|---|---|
| 1,000 件を登録して待機しているとき | 約 17 MB |
| 5,000 件を 300 秒間隔で監視したときのピーク | 約 24 MB |

実行ファイルの大きさは約 28 MB です。

## 特長

- **必要な所だけ比べる**: CSS セレクタ、XPath、JSONPath、jq で対象を絞ります。正規表現や無視する文字列で揺れを除き、トリガーテキストが一致したときだけ通知することもできます。
- **JavaScript のページも**: 別に動かした Chrome（DevTools の WebSocket）に接続して、描画後のページを取得します。クリック・入力・待機の操作を登録でき、画面から要素を選ぶビジュアルセレクタも使えます。
- **価格・在庫・PDF**: 商品ページの構造化データ（JSON-LD、OpenGraph、microdata）から価格を読み、しきい値以下で通知します。PDF の本文も比べられます。
- **多くの通知先**: Discord、Slack、Telegram、ntfy、Gotify、Mattermost、Pushover、メール（SMTP）、JSON の Webhook に対応します。そのほかは Apprise API へ転送します。件名と本文はテンプレートで変えられ、送れなかった通知は再送します。
- **取得の細かな指定**: ヘッダ・メソッド・ボディ・プロキシ・間隔を、ウォッチごと、タグごと、全体で決められます。近い設定が優先されます。
- **ダッシュボード**: ウォッチの数、失敗中のもの、通知キューの滞り、直近の変化を 1 画面で見られます。
- **ワンコマンドで起動**: `watchtower ui` で、サーバーを起動してブラウザで管理画面を開きます。
- **運用しやすい**: OpenAPI の文書、Prometheus 形式の `/metrics`、RSS フィード、バックアップと復元、`/healthz`。changedetection.io の `url-watches.json` も取り込めます。Docker 用のファイルも付いています。

## 安全への配慮

- 内部のアドレス（プライベート・ループバック・リンクローカル）への接続を既定で断る SSRF 対策
- argon2 で保存するパスワード、CSRF 対策、厳しい CSP
- DNS リバインディングを防ぐ Host 名の許可リスト
- ヘッダの値や通知先の秘密を、ログやエラーに出さない

管理画面はパスワードだけで守られます。外へ公開するときは、必ず HTTPS のリバースプロキシの背後に置いてください。

## 使い方

```sh
watchtower set-password     # 管理画面のパスワード（標準入力から 1 行）
watchtower api-key          # REST API のキーを発行
watchtower watch add https://example.com/ --interval 300
watchtower ui               # 起動して、ブラウザで管理画面を開く
```

画面を開かずに常駐させるときは `watchtower run` を使います。管理画面は既定で `127.0.0.1:5000` に開きます。
設定は `watchtower.toml`（例は `watchtower.example.toml`）か、`WATCHTOWER_*` の環境変数で変えます。

## 入手方法

- **Windows 11（64 ビット）**: zip を展開して `watchtower.exe` を実行します。インストールは要りません。
- **Linux（x86_64・aarch64）**: GitHub のリリースに、静的リンクのバイナリがあります。
- **ソースコード**: https://github.com/taotaro1104-commits/watchtower-rs
- **ライセンス**: Apache-2.0

## 開発の状況

開発中です（v0.1.0・プレリリース）。Windows 11 では、実際のバイナリを起動してブラウザから操作できることを確かめています。
次の項目は、まだ確かめていません。

- Docker イメージ（CI でビルドは通っていますが、起動はまだ確かめていません）
- Linux 用の静的バイナリ（musl）と aarch64（ビルドは通っていますが、実行はまだ確かめていません）
- TLS・STARTTLS を使う実際のメールサーバーへの送信
