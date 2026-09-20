# Contributing

## Development principles

このリポジトリでは、まず動作の正しさと変更理由の追跡可能性を優先します。大規模な将来構想を一度に実装せず、検証可能な単位で変更してください。

## Branch and pull request workflow

- `main` へ直接機能実装を行わず、目的が分かるブランチを作成します。
- 例: `fix/...`, `feat/...`, `refactor/...`, `docs/...`。
- PRには変更理由、スコープ、検証方法、既知の制約を記載します。
- CIが失敗しているPRはマージしません。
- 仕様変更と無関係な依存関係の一括更新は同じPRに混ぜません。
- 可能な限り squash merge を使用し、`main` の履歴を変更単位で保ちます。

## Rust architecture

Backend の依存方向を次のように保ちます。

1. `domain`
   - Entity と repository trait など、業務ルールと境界を所有します。
   - Actix Web、ScyllaDB、Redis、Elasticsearch など具体的なI/O実装へ依存させません。
2. `application`
   - Domain を組み合わせてユースケースを実装します。
   - HTTP固有の型やDBドライバを持ち込みません。
3. `infrastructure`
   - Domain が定義した境界を ScyllaDB / Redis / Elasticsearch などで実装します。
4. `interfaces`
   - HTTP request/response、routing、handlerを担当し、Application を呼び出します。

新しい抽象化や空のレイヤーファイルは、具体的な利用箇所ができてから追加してください。

### Rust style

変更前後で次を通します。

```bash
cd backend
cargo check
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

- request path と通常の起動処理では `unwrap()` / `expect()` を避け、意味のあるerrorへ変換します。
- 永続化クエリはScyllaDBのpartition key / clustering keyに沿ったアクセスパターンにします。
- ユーザースコープのデータでは `user_id` をrepository境界から落とさないでください。
- キャッシュキーにもユーザー境界を含めます。
- Domain entityへDBドライバ固有のserialization traitを実装しないでください。
- エラーを握りつぶさず、利用者へ公開する内容と内部ログに残す内容を区別します。

## Frontend architecture

- TypeScriptを使用し、`any`でAPI契約を回避しないでください。
- HTTPアクセスは `src/lib/api` に集約します。route/componentから同じAPIを独自に再実装しないでください。
- Backend DTO とFrontend typeのfield名・必須性を合わせます。特に `version` は更新競合制御に必要です。
- 1つのSvelteコンポーネント内でlegacy syntaxとrunes syntaxを混在させないでください。
- 新規コードはSvelte 5で明示的かつ一貫した書き方を優先します。
- loading / empty / error / conflict の状態をUI上で区別してください。
- destructive actionには確認操作を設けます。
- server side renderingが必要になるまで、現在のMVPはブラウザから同一origin `/api` を利用します。

Frontend の品質チェック:

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

`pnpm check` がSvelte/TypeScriptの主要な静的検証です。現在のESLintはJavaScript設定ファイルを対象とし、Svelte/TypeScriptの構文検証は `svelte-check` が担当します。

### UI visual diff

PRでは `UI Diff / Visual diff` workflow がbase revisionとPR revisionを同じChromium環境で起動し、`/memos`、検索、作成、編集画面を固定fixtureで撮影します。UI差分はreview用のinformational signalであり、pixel差分そのものではPRをfailさせません。

workflow artifact `ui-diff-pr-<number>` には各画面の以下を含めます。

- `baseline/`: PR baseのスクリーンショット
- `candidate/`: PR headのスクリーンショット
- `diff/`: pixel diff、before/after横並び画像、changed-pixel summary

UIを変更するPRでは、通常のFrontend CIに加えてこのartifactを確認してください。撮影や比較処理自体が壊れた場合はworkflowをfailさせます。

同じcaptureでは `@axe-core/playwright` でWCAG 2.x系ルールを検査し、baseと候補画面のserious/critical違反を比較します。既存違反はbaselineとして許容しますが、新しいルールが出た場合、または同じルールで影響ノード数が増えた場合はUI Diffをfailさせます。違反が減る変更はそのまま通過します。

Svelte componentは `@testing-library/svelte` + Vitest + jsdomで、DOM実装詳細ではなくrole/text/linkなど利用者から観測できる振る舞いを優先して検証します。

### Lighthouse baseline

PRでは `Lighthouse / Lighthouse baseline` workflow がproduction build/previewを固定fixture backendへ接続し、代表画面（一覧・新規・編集）を各3回計測します。median runの Performance / Accessibility / Best Practices / SEO scoreをjob summaryへ出し、HTML/JSON reportとmanifestを `lighthouse-pr-<number>` artifactへ14日間保存します。

現在の安定baseline（代表3画面で各category 100）を基準に、Accessibilityは100、Best PracticesとSEOは95未満をblocking failureにします。PerformanceはCI runnerの実行ノイズを考慮し、90未満をwarningとして可視化します。依存解決・build・preview・Lighthouse collection・report生成が壊れた場合もworkflowをfailさせます。

## Tests

- bugfixでは、可能な限り不具合を再現するテストを先に追加します。
- domain/applicationのロジックは外部サービスなしで検証できる形を優先します。
- ScyllaDBなど実サービスが必要なintegration testは、通常のunit testと区別してください。
- UI変更では、少なくとも `pnpm check` と `pnpm build` を通し、PRの `UI Diff` artifactも確認してください。

## Docker Compose

通常の開発環境は次で起動できる状態を維持します。

```bash
docker compose up --build
```

変更時は最低限、次を確認します。

```bash
docker compose config --quiet
docker compose build backend frontend
```

通常利用に不要な補助サービスはCompose profileへ分離してください。

## Security

- `AUTH_MODE=development` と `X-Development-User-Id` はローカル開発専用です。本番では使用せず、独立した認証サービスが発行するBearer JWTを検証してください。Oryや共通認証基盤への依存をmemo_serverへ持ち込まないでください。
- credential、token、API keyをrepositoryへcommitしないでください。
- ユーザー入力をHTMLへ描画する場合はsanitizeを維持してください。
- tenant/user境界を外す変更は、明示的な認可設計なしに行わないでください。

## Review checklist

PRをReadyにする前に以下を確認します。

- 変更がPRの目的に限定されている
- API契約のFrontend/Backend不整合がない
- error/loading/empty/conflict stateが破綻していない
- user/tenant境界が維持されている
- `cargo fmt`, `clippy`, testsが通る
- `pnpm check`, lint, tests, buildが通る
- UI変更を含む場合、`UI Diff` artifactのbefore/after/diffを確認した
- Docker定義がvalidでapplication imageをbuildできる
- READMEまたは設計資料に影響する変更が反映されている
