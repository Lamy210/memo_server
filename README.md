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

> [!NOTE]
> メモAPIは認証必須です。Docker Compose は `AUTH_MODE=development` を明示し、Frontend がリクエストごとに `X-Development-User-Id` を付与します。本番では `AUTH_MODE=oidc` を使用してください。

## 起動

Docker と Docker Compose v2 が利用できる環境で、リポジトリルートから実行します。

```bash
docker compose up --build
```

起動後:

- Frontend: http://localhost:3001
- Backend liveness: http://localhost:8083/api/v1/health/live
- Backend readiness: http://localhost:8083/api/v1/health/ready
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
| `GET` | `/health` | Legacy liveness alias |
| `GET` | `/health/live` | Process liveness |
| `GET` | `/health/ready` | Dependency-aware readiness |
| `GET` | `/memos` | 一覧 |
| `POST` | `/memos` | 作成 |
| `GET` | `/memos/{id}` | 取得 |
| `PATCH` | `/memos/{id}` | 更新 |
| `DELETE` | `/memos/{id}` | 削除 |
| `GET` | `/memos/search` | 検索 |

### Authentication

Health endpoint 以外の memo API は認証が必要です。

ローカル開発では `AUTH_MODE=development` を明示し、各リクエストに `X-Development-User-Id: <UUID>` を付与します。固定ユーザーをBackendへ暗黙注入する方式は使用しません。Compose のFrontendは `VITE_DEVELOPMENT_USER_ID` からこのheaderを付与します。

本番では `AUTH_MODE=oidc` を使用します。BackendはOAuth2 Bearer access tokenをRS256で検証し、設定したissuer・audience・expiry・subjectを検証します。署名鍵は `AUTH_JWKS_URI` のJWKSから取得し、key rotation時はJWKSを再取得します。認証基盤の契約に合わせ、JWT `sub` をcanonical platform user UUIDとしてmemo ownershipに利用します。

```bash
curl -H 'Authorization: Bearer <access-token>' \
  http://localhost:8083/api/v1/memos
```

### Health / readiness

`/health/live` はプロセスがHTTPリクエストを処理できることだけを確認します。Docker Composeのbackend healthcheckもこのendpointを利用するため、任意のsecondary store障害だけではbackendコンテナをunhealthyにしません。

`/health/ready` は依存サービスを最大2秒で並行probeし、次の状態を返します。

| State | HTTP | ScyllaDB | Redis / Elasticsearch | Meaning |
| --- | ---: | --- | --- | --- |
| `ready` | 200 | healthy | healthy | 全機能を利用可能 |
| `degraded` | 200 | healthy | 1つ以上down | CRUDは利用可能。cache/search projectionは縮退 |
| `unavailable` | 503 | down | any | authoritative storeへ安全にアクセスできないためreadyではない |

ScyllaDBがauthoritative storeです。Redisはcache、Elasticsearchは再構築可能なsearch projectionとして扱うため、secondary store障害だけではcore CRUDのreadinessを落としません。

### Projection reconciliation

メモの作成・更新・削除では、Redis/Elasticsearchへ反映するためのdurable projection intentをScyllaDBへprimary mutationより先に一意eventとして保存します。保存時は対象のmemo version、削除時はdelete targetを持ちます。primary mutation自体が失敗した場合、そのmutation専用eventだけをcleanupするため、並行mutationのintentを上書きしません。

通常はprimary mutation直後に同期を試みます。RedisまたはElasticsearchが利用できない場合でもprimary CRUDは成功し、intentはScyllaDBに残ります。background reconcilerが約2秒間隔で再試行し、backend再起動後も未処理intentを再開します。

reconcilerは現在のScyllaDB状態をsource of truthとして同期します。保存intentはScyllaDBがtarget version以上へ到達するまで、削除intentは行が消えるまでackしません。各intentは一意eventなのでworker同士が別mutationのintentを削除しません。secondaryへ書いた直後にScyllaDBを再確認し、同期中にsource stateが変わっていればcorrective intentを先に追加してから古いeventをackするため、stale workerによる書き戻しも最終的に再収束します。

この仕組みにより、secondary store停止中のcreate/update/deleteは、secondary store復帰後にcache/search projectionへ収束します。

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
| `AUTH_MODE` | 必須。Composeでは `development` |
| `AUTH_ISSUER` | `AUTH_MODE=oidc` のとき必須 |
| `AUTH_AUDIENCE` | `AUTH_MODE=oidc` のとき必須。consuming serviceのOAuth2 client ID |
| `AUTH_JWKS_URI` | `AUTH_MODE=oidc` のとき必須 |

`DATABASE_URL` は既存環境との互換目的で Scylla の接続先としても読み取りますが、新規設定では `SCYLLA_URI` を使ってください。

Frontend の Vite 開発サーバーは `BACKEND_URL` を `/api` のproxy先として利用します。Compose では `http://backend:8080` が設定されます。ローカル開発用の `VITE_DEVELOPMENT_USER_ID` はFrontendから `X-Development-User-Id` として送信されます。本番buildでは設定しないでください。

## スコープ

現在のMVPはOIDC resource-server認証までを対象にします。添付ファイル、共有メモ、リアルタイム共同編集、WebRTC/CRDT、CQRS/Event Sourcing は含めていません。まず基本的なメモライフサイクル、認証境界、開発・CI基盤を安定させ、その後に拡張します。

開発規約とPR運用は [CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。
