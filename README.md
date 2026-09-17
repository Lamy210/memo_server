# memo_server

Rust/Actix Web + SvelteKit で構成したメモアプリケーションです。現在の復旧フェーズでは、メモの作成・一覧・編集・検索・削除を安定して使えることを優先しています。

## 現在の構成

- Frontend: SvelteKit 2 / Svelte 5 / TypeScript / Tailwind CSS
- Backend: Rust / Actix Web
- Primary store: ScyllaDB
- Cache: Redis
- Search index: Elasticsearch
- Local orchestration: Docker Compose

ScyllaDB をメモ本体の永続化先とし、Redis はキャッシュ、Elasticsearch は検索用インデックスとして利用します。

> [!WARNING]
> 現在の `DEVELOPMENT_USER_ID` はローカル開発用の固定ユーザーです。認証・認可の代替ではありません。外部公開する前に実際の認証基盤へ置き換えてください。

## 起動

Docker と Docker Compose v2 が利用できる環境で、リポジトリルートから実行します。

```bash
docker compose up --build
```

起動後:

- Frontend: http://localhost:3001
- Backend health: http://localhost:8083/api/v1/health
- Elasticsearch: http://localhost:9200
- ScyllaDB: localhost:9042
- Redis: localhost:6379

Kibana も必要な場合は `observability` profile を有効にします。

```bash
docker compose --profile observability up --build
```

Kibana は http://localhost:5601 です。

停止:

```bash
docker compose down
```

永続ボリュームも破棄して完全に初期化する場合のみ、次を使います。

```bash
docker compose down -v
```

## 主な画面

- `/memos` — メモ一覧
- `/memos/new` — 新規作成
- `/memos/:id/edit` — 編集、Markdownプレビュー、削除
- `/memos/search` — 全文・タグ検索

編集画面では optimistic concurrency 用の `version` を利用し、古いバージョンからの更新は `409 Conflict` になります。Cmd/Ctrl+S と編集時の自動保存に対応しています。

## API

Base path は `/api/v1` です。

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/health` | Liveness |
| `GET` | `/memos` | 一覧 |
| `POST` | `/memos` | 作成 |
| `GET` | `/memos/{id}` | 取得 |
| `PATCH` | `/memos/{id}` | 更新 |
| `DELETE` | `/memos/{id}` | 削除 |
| `GET` | `/memos/search` | 検索 |

## ローカル品質チェック

Backend:

```bash
cd backend
cargo check
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Frontend:

```bash
cd frontend
corepack enable
corepack prepare pnpm@9.15.9 --activate
pnpm install --frozen-lockfile
pnpm check
pnpm lint
pnpm test:unit -- --run
pnpm build
```

Docker 定義:

```bash
docker compose config --quiet
docker compose build backend frontend
```

同じチェックは GitHub Actions でも実行されます。

## 設定

Backend が利用する主な環境変数:

| Variable | Development default |
| --- | --- |
| `SCYLLA_URI` | `127.0.0.1:9042` |
| `REDIS_URL` | `redis://127.0.0.1:6379` |
| `ELASTICSEARCH_URL` | `http://127.0.0.1:9200` |
| `PORT` | `8080` |
| `DEVELOPMENT_USER_ID` | `12345678-1234-1234-1234-123456789012` |

`DATABASE_URL` は既存環境との互換目的で Scylla の接続先としても読み取りますが、新規設定では `SCYLLA_URI` を使ってください。

Frontend の Vite 開発サーバーは `BACKEND_URL` を `/api` のproxy先として利用します。Compose では `http://backend:8080` が設定されます。

## スコープ

現在のMVPには認証/OIDC、添付ファイル、共有メモ、リアルタイム共同編集、WebRTC/CRDT、CQRS/Event Sourcing は含めていません。まず基本的なメモライフサイクルと開発・CI基盤を安定させ、その後に拡張します。

開発規約とPR運用は [CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。
