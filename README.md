# memo_server

Rust/Actix Web + SvelteKit で構成したメモアプリケーションです。現在の復旧フェーズでは、メモの作成・一覧・編集・検索・削除を安定して使えることを優先しています。

## 現在の構成

- Frontend: SvelteKit 2 / Svelte 5 / TypeScript / Tailwind CSS
- Backend: Rust / Actix Web
- Primary store: ScyllaDB
- Cache: Valkey 9.1
- Search index: Elasticsearch
- Local orchestration: Docker Compose

ScyllaDB をメモ本体の永続化先とし、Valkey はキャッシュ、Elasticsearch は検索用インデックスとして利用します。Rust側はRESP互換のため既存の `redis` crateをクライアント実装として継続利用します。

> [!NOTE]
> メモAPIは認証必須です。Docker Compose は `AUTH_MODE=development` を明示し、SvelteKit server proxy が private env `DEVELOPMENT_USER_ID` から `X-Development-User-Id` を付与します。ブラウザ側JSは認証headerを生成しません。本番では独立した認証サービスを運用し、`AUTH_MODE=jwt` でそのサービスが発行するaccess tokenを検証します。

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
- Valkey: localhost:6379

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

Memo write API applies explicit input boundaries: JSON request bodies are limited to 512 KiB, titles to 160 Unicode characters, tags to at most 10 entries, and each tag to 64 Unicode characters. Search queries are limited to 512 Unicode characters and search tags to 64. Requests outside these field limits return `422 Unprocessable Entity`; an oversized JSON body returns `413 Payload Too Large`.

### Authentication

Health endpoint 以外の memo API は認証が必要です。

ローカル開発では `AUTH_MODE=development` を明示し、SvelteKit server proxy が private env `DEVELOPMENT_USER_ID` から各Backendリクエストへ `X-Development-User-Id: <UUID>` を付与します。ブラウザから送られた `Authorization` / `X-Development-User-Id` / Cookie はそのままBackendへ転送せず、server-sideの認証コンテキストだけを利用します。

本番では `AUTH_MODE=jwt` を使用します。Backendは専用の認証サービスが発行したBearer access tokenをRS256で検証し、設定したissuer・audience・expiry・issued-at・subjectを検証します。署名鍵は `AUTH_JWKS_URI` のJWKSから取得し、key rotation時はJWKSを再取得します。JWT `sub` はmemo_server内のuser UUIDとして扱います。

memo_serverは認証サービスと独立して運用します。Oryや共通認証基盤との連携は前提にせず、認証サービス側がユーザー登録・ログイン・セッション/refresh token・パスワード/MFA等を担当し、memo_serverはaccess tokenの検証とuser境界の適用だけを担当します。

```bash
curl -H 'Authorization: Bearer <access-token>' \
  http://localhost:8083/api/v1/memos
```

### Health / readiness

`/health/live` はプロセスがHTTPリクエストを処理できることだけを確認します。Docker Composeのbackend healthcheckもこのendpointを利用するため、任意のsecondary store障害だけではbackendコンテナをunhealthyにしません。

`/health/ready` は依存サービスを最大2秒で並行probeし、次の状態を返します。

| State | HTTP | ScyllaDB | Valkey / Elasticsearch | Meaning |
| --- | ---: | --- | --- | --- |
| `ready` | 200 | healthy | healthy | 全機能を利用可能 |
| `degraded` | 200 | healthy | 1つ以上down | CRUDは利用可能。cache/search projectionは縮退 |
| `unavailable` | 503 | down | any | authoritative storeへ安全にアクセスできないためreadyではない |

ScyllaDBがauthoritative storeです。Valkeyはcache、Elasticsearchは再構築可能なsearch projectionとして扱うため、secondary store障害だけではcore CRUDのreadinessを落としません。Valkeyはdisposable cacheとしてRDB/AOFを無効化しています。readiness JSONの `.checks.redis` は後方互換のため現時点では名称を維持しています。

### Projection reconciliation

メモの作成・更新・削除では、Valkey/Elasticsearchへ反映するためのdurable projection intentをScyllaDBへprimary mutationより先に一意eventとして保存します。保存時は対象のmemo version、削除時はdelete targetを持ちます。primary mutationでクライアント側にエラーが返ってもserver-side commit済みの可能性があるためintentは即時削除せず保持し、reconcilerがauthoritative stateを確認します。targetへ到達しないintentはgrace period後にstaleとして破棄します。

通常はprimary mutation直後に同期を試みます。ValkeyまたはElasticsearchが利用できない場合でもprimary CRUDは成功し、intentはScyllaDBに残ります。background reconcilerが約2秒間隔で再試行し、backend再起動後も未処理intentを再開します。

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
| `REDIS_URL` | `redis://127.0.0.1:6379`（Valkey接続先。互換env名を維持） |
| `ELASTICSEARCH_URL` | `http://127.0.0.1:9200` |
| `PORT` | `8080` |
| `AUTH_MODE` | 必須。Composeでは `development` |
| `AUTH_ISSUER` | `AUTH_MODE=jwt` のとき必須 |
| `AUTH_AUDIENCE` | `AUTH_MODE=jwt` のとき必須。memo API向けのaudience値 |
| `AUTH_JWKS_URI` | `AUTH_MODE=jwt` のとき必須 |

`DATABASE_URL` は既存環境との互換目的で Scylla の接続先としても読み取りますが、新規設定では `SCYLLA_URI` を使ってください。

Frontend は SvelteKit server route `/api/v1/...` をBackendへの同一origin proxyとして利用します。`BACKEND_URL` はserver-sideのみで参照され、Composeでは `http://backend:8080` が設定されます。ローカル開発では private env `DEVELOPMENT_USER_ID` を設定し、development buildのserver proxyだけが `X-Development-User-Id` を注入します。`VITE_*` へ認証情報を置かないでください。

## スコープ

現在のMVPは専用Authサービスが発行するJWTのresource-server検証までを対象にします。添付ファイル、共有メモ、リアルタイム共同編集、WebRTC/CRDT、CQRS/Event Sourcing は含めていません。まず基本的なメモライフサイクル、認証境界、開発・CI基盤を安定させ、その後に拡張します。

認証サービス境界の詳細は [docs/authentication.md](docs/authentication.md) を参照してください。開発規約とPR運用は [CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。
