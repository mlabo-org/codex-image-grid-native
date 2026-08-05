# Codex Image Grid

A native macOS image-generation workspace for Codex, built with Rust and
SwiftUI. It provides Prompt Batch generation, reference-image analysis,
Japanese/English UI, light/dark themes, run history, and artifact handoff
through the `codex_image_grid/generate_image_grid` MCP tool.

Codex向けのmacOSネイティブ画像生成ワークスペースです。RustとSwiftUIで
実装され、Prompt Batch、参照画像解析、日英UI、ライト/ダークテーマ、
生成履歴、成果物の受け渡しに対応しています。

[English](#english) · [日本語](#日本語)

## UI preview / UIプレビュー

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="./docs/assets/codex-image-grid-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="./docs/assets/codex-image-grid-light.png">
  <img alt="Codex Image Grid native macOS UI" src="./docs/assets/codex-image-grid-light.png" width="100%">
</picture>

<details>
<summary>Show both themes / 両テーマを表示</summary>

| Light / ライト | Dark / ダーク |
| --- | --- |
| ![Light theme](./docs/assets/codex-image-grid-light.png) | ![Dark theme](./docs/assets/codex-image-grid-dark.png) |

</details>

## English

### Requirements

- macOS 14 or later
- Apple Swift 6.3 toolchain
- Rust 1.95 (selected automatically by `rust-toolchain.toml`)
- Codex CLI installed and signed in
- `jq`, `rg`, `curl`, Xcode command-line tools, and internet access for the
  first dependency build

The generated-image route uses `codex app-server`, so the active Codex account
must have access to image generation.

### Automatic setup when opened in Codex

Clone this repository and open the clone as a Codex workspace. On the first
setup, build, test, run, or source task, the repository-scoped `AGENTS.md`
instructs Codex to run:

```bash
scripts/bootstrap-codex.sh
```

The idempotent bootstrap:

1. checks the macOS toolchain;
2. builds the locked Rust workspace and release SwiftUI app;
3. installs the signed app at
   `~/Applications/Codex Image Grid Native.app`;
4. registers this clone as the `codex-image-grid-native` local marketplace;
5. installs the `codex-image-grid` plugin; and
6. records a source fingerprint under the ignored `.run/` directory so an
   unchanged clone is not rebuilt.

Passive cloning alone does not execute repository code. Codex performs the
bootstrap when it starts an applicable task in the imported workspace. The
script stops without replacing anything if another source already owns the
same plugin or marketplace identity.

Preview the setup without changing the machine:

```bash
scripts/bootstrap-codex.sh --dry-run
```

Rebuild and refresh a plugin installed by this repository's marketplace after
an intentional source change:

```bash
scripts/bootstrap-codex.sh --force
```

Start a new Codex task after first-time plugin installation so the newly
installed MCP route is discovered.

### Quick Start and MCP usage

#### Generate your first image

After automatic setup, start a new Codex task and ask:

> Use `codex_image_grid/generate_image_grid` with `waitMs: 120000` to generate
> one 16:9 `clean-thumbnail` image: "A polished product hero for a native
> macOS image-generation app, graphite background, blue accent, no text."

Codex turns the natural-language request into an MCP call, so you do not need
to write JSON yourself. The native SwiftUI app opens or becomes active
automatically. When generation completes, Codex receives display-safe image
URLs, absolute output paths, Codex Markdown, and artifact handoff information.

To submit multiple prompts and image jobs together, ask for a Prompt Batch:

> Use `codex_image_grid/generate_image_grid` to create one Prompt Batch. Make
> one 16:9 image for each prompt: (1) a clean product hero for the app; (2) an
> editorial scene showing prompts becoming an image grid.

Multiple prompts are submitted in one `prompts` array. `count` sets the number
of variants for every prompt, so two prompts with two variants create four
image jobs in the same run.

#### Native UI or Codex/MCP?

| Route | Best for |
| --- | --- |
| Native SwiftUI app | Editing Prompt Batches interactively, choosing a reference image with the macOS file picker, and watching progress or run history. |
| Codex/MCP | Generating from a Codex task, passing project-specific prompts or local reference files, and handing absolute artifact paths to another workflow. |

Calling the MCP tool also opens the native app, so both routes use the same
local runtime and artifact history.

#### Representative MCP input and output

The following is an illustrative MCP argument object, not a shell command. It
sends two prompts with two variants each as one batch of four image jobs:

```json
{
  "prompts": [
    "A clean product hero for a native macOS image-generation workspace, graphite background, electric-blue accent, no text",
    "An editorial workflow scene showing prompt batches becoming a polished image grid, no text"
  ],
  "count": 2,
  "mood": "clean-thumbnail",
  "engine": "app-server-image",
  "aspectRatio": "16:9",
  "referencePremise": "Preserve the mascot's round glasses and blue scarf.",
  "referenceImagePath": "/Users/you/Pictures/mascot.png",
  "waitMs": 120000
}
```

Only `prompts` is required. A batch may contain up to 12 prompts and 6
variants per prompt, with at most 24 image jobs in total
(`prompts.length * count`). `referenceImagePath`, when supplied, must be an
absolute path to a local PNG, JPEG, or WebP file no larger than 100 MiB. Use
`engine: "codex-svg"` for SVG output. `waitMs` may be from 0 through 120,000.

A completed one-image call returns structured fields such as:

```json
{
  "runId": "<runId>",
  "status": "done",
  "completed": true,
  "outputPaths": [
    "/Users/you/Library/Application Support/codex-image-grid/generated/<runId>/variant-01.png"
  ],
  "imageUrls": [
    "http://127.0.0.1:4322/generated/<runId>/variant-01.png"
  ],
  "manifestPath": "/Users/you/Library/Application Support/codex-image-grid/generated/<runId>/manifest.json",
  "handoffPath": "/Users/you/Library/Application Support/codex-image-grid/generated/<runId>/handoff.md",
  "codexMarkdown": "![prompt 1/1 variant 1/1](http://127.0.0.1:4322/generated/<runId>/variant-01.png)"
}
```

Each run is saved under
`~/Library/Application Support/codex-image-grid/generated/<runId>/`, including
the generated files, `manifest.json`, `handoff.md`, and a staged reference
image when one was supplied. If generation is still running when `waitMs`
expires, use the returned `statusUrl` or `handoffPath` to follow the run.

### Build and install manually

Run the complete provider-free acceptance command:

```bash
scripts/check.sh
```

Build the components directly:

```bash
cargo build --workspace --locked
swift build --package-path macos
```

Inspect or execute the native app installer:

```bash
scripts/install-native-app.sh --dry-run
scripts/install-native-app.sh --execute
```

Register the plugin manually from the repository root:

```bash
codex plugin marketplace add .
codex plugin add codex-image-grid@codex-image-grid-native
```

### Repository layout

- `crates/image-grid-core/` — validation, job state, retry policy, and artifact
  contracts.
- `crates/image-grid-server/` — loopback-only HTTP/SSE runtime and Codex App
  Server bridge.
- `crates/image-grid-mcp/` — stdio MCP server for
  `generate_image_grid`.
- `macos/` — native SwiftUI application.
- `plugin/codex-image-grid/` — installable Codex plugin package.
- `.agents/plugins/marketplace.json` — local marketplace metadata for Codex.

Runtime files and generated images are stored outside the repository at
`~/Library/Application Support/codex-image-grid`. The local HTTP runtime binds
only to loopback and rejects non-loopback browser origins.

### License

MIT. See [LICENSE](LICENSE).

## 日本語

### 必要環境

- macOS 14以降
- Apple Swift 6.3ツールチェーン
- Rust 1.95（`rust-toolchain.toml`で自動選択）
- インストール・サインイン済みのCodex CLI
- `jq`、`rg`、`curl`、Xcode Command Line Tools
- 初回の依存関係取得に使うインターネット接続

画像生成は`codex app-server`を使用します。利用中のCodexアカウントで画像生成が
使える必要があります。

### Codexに取り込んだときの自動セットアップ

このリポジトリをcloneし、そのcloneをCodexのワークスペースとして開いてください。
初回のセットアップ・ビルド・テスト・実行・ソース変更タスクで、リポジトリ内の
`AGENTS.md`に従い、Codexが次を実行します。

```bash
scripts/bootstrap-codex.sh
```

このスクリプトは一度の実行で次を行います。

1. macOSのビルド環境を確認
2. lock済みRustワークスペースとSwiftUIリリースアプリをビルド
3. `~/Applications/Codex Image Grid Native.app`へ署名・配置
4. このcloneを`codex-image-grid-native`ローカルマーケットプレイスとして登録
5. `codex-image-grid`プラグインをインストール
6. 無視対象の`.run/`へソース指紋を保存し、変更がなければ次回ビルドを省略

cloneしただけではリポジトリ内のコードは実行されません。Codexが取り込んだ
ワークスペースで対象タスクを開始した時点でセットアップされます。同名の
プラグインまたはマーケットプレイスを別ソースが所有している場合は、上書きせず
停止します。

マシンを変更せず内容だけ確認する場合:

```bash
scripts/bootstrap-codex.sh --dry-run
```

意図的なソース変更後、このリポジトリのマーケットプレイスから入れたプラグインを
再ビルド・再反映する場合:

```bash
scripts/bootstrap-codex.sh --force
```

初回プラグインインストール後は、新しいCodexタスクを開始すると追加されたMCPが
検出されます。

### Quick StartとMCP利用

#### 最初の1枚を生成

自動セットアップ完了後、新しいCodexタスクで次のように依頼します。

> `codex_image_grid/generate_image_grid`を使い、「macOSネイティブ画像生成アプリを
> 紹介する、グラファイト背景と青いアクセントの洗練された製品ビジュアル、
> 文字なし」を、`clean-thumbnail`、16:9で1枚生成してください。完了を最大120秒
> 待ってください。

Codexが自然言語の依頼からMCPを呼び出すため、JSONを手書きする必要はありません。
ネイティブSwiftUIアプリが自動的に開くか、前面へ移動します。生成完了後は、
表示用画像URL、絶対パス、Codex Markdown、成果物の受け渡し情報がCodexへ返ります。

複数のプロンプトや画像ジョブをまとめて依頼する場合は、Prompt Batchとして依頼します。

> `codex_image_grid/generate_image_grid`を使い、1つのPrompt Batchを作成してください。
> 次の各プロンプトから16:9画像を1枚ずつ生成してください。(1) アプリのクリーンな
> 製品ビジュアル、(2) プロンプトが画像グリッドになる編集風の作業シーン。

複数のプロンプトは1つの`prompts`配列で送信されます。`count`は各プロンプトの
バリエーション数を指定するため、2プロンプトで各2枚を指定すると、同じ実行内で
4つの画像ジョブが作成されます。

#### ネイティブUIとCodex/MCPの使い分け

| 利用方法 | 適している場面 |
| --- | --- |
| ネイティブSwiftUIアプリ | Prompt Batchを画面で調整する、macOSのファイル選択から参照画像を指定する、進行状況や生成履歴を確認する場合 |
| Codex/MCP | Codexタスクの文脈から生成する、プロジェクト固有のプロンプトやローカル参照ファイルを渡す、絶対パス付きの成果物を別工程へ引き渡す場合 |

MCPから呼び出した場合もネイティブアプリが開き、両方の経路が同じローカルランタイムと
成果物履歴を使用します。

#### MCPの代表的な入力と出力

次はMCPへ渡される引数の例であり、シェルコマンドではありません。2つのプロンプトと
各2枚のバリエーションを1つのバッチとして送信し、4つの画像ジョブを作成します。

```json
{
  "prompts": [
    "A clean product hero for a native macOS image-generation workspace, graphite background, electric-blue accent, no text",
    "An editorial workflow scene showing prompt batches becoming a polished image grid, no text"
  ],
  "count": 2,
  "mood": "clean-thumbnail",
  "engine": "app-server-image",
  "aspectRatio": "16:9",
  "referencePremise": "丸い眼鏡と青いスカーフというマスコットの特徴を維持する。",
  "referenceImagePath": "/Users/you/Pictures/mascot.png",
  "waitMs": 120000
}
```

必須項目は`prompts`だけです。1つのバッチには最大12プロンプト、各プロンプトには最大
6バリエーションを指定でき、合計24画像ジョブまでです
(`prompts.length * count`)。`referenceImagePath`には、100 MiB以下のローカルPNG・JPEG・
WebPファイルの絶対パスを指定します。SVGが必要な場合は`engine: "codex-svg"`を使用します。
`waitMs`には0から120,000までを指定できます。

1枚の生成が完了したときは、次のような構造化フィールドが返ります。

```json
{
  "runId": "<runId>",
  "status": "done",
  "completed": true,
  "outputPaths": [
    "/Users/you/Library/Application Support/codex-image-grid/generated/<runId>/variant-01.png"
  ],
  "imageUrls": [
    "http://127.0.0.1:4322/generated/<runId>/variant-01.png"
  ],
  "manifestPath": "/Users/you/Library/Application Support/codex-image-grid/generated/<runId>/manifest.json",
  "handoffPath": "/Users/you/Library/Application Support/codex-image-grid/generated/<runId>/handoff.md",
  "codexMarkdown": "![prompt 1/1 variant 1/1](http://127.0.0.1:4322/generated/<runId>/variant-01.png)"
}
```

各実行の成果物は
`~/Library/Application Support/codex-image-grid/generated/<runId>/`へ保存されます。
生成ファイル、`manifest.json`、`handoff.md`に加え、指定時は参照画像のコピーも含まれます。
`waitMs`内に完了しなかった場合は、返された`statusUrl`または`handoffPath`から進行状況を
確認できます。

### 手動ビルド・インストール

プロバイダー不要の一括検証:

```bash
scripts/check.sh
```

各コンポーネントを直接ビルド:

```bash
cargo build --workspace --locked
swift build --package-path macos
```

ネイティブアプリのインストール内容確認・実行:

```bash
scripts/install-native-app.sh --dry-run
scripts/install-native-app.sh --execute
```

リポジトリルートからプラグインを手動登録:

```bash
codex plugin marketplace add .
codex plugin add codex-image-grid@codex-image-grid-native
```

### リポジトリ構成

- `crates/image-grid-core/` — 入力検証、ジョブ状態、再試行、成果物契約
- `crates/image-grid-server/` — loopback限定HTTP/SSEとCodex App Server接続
- `crates/image-grid-mcp/` — `generate_image_grid`用stdio MCPサーバー
- `macos/` — SwiftUIネイティブアプリ
- `plugin/codex-image-grid/` — Codexへインストールするプラグイン一式
- `.agents/plugins/marketplace.json` — Codex用ローカルマーケットプレイス定義

実行データと生成画像はリポジトリ外の
`~/Library/Application Support/codex-image-grid`へ保存されます。ローカルHTTP
サーバーはloopbackだけにbindし、外部オリジンからのブラウザ要求を拒否します。

### ライセンス

MITです。[LICENSE](LICENSE)を参照してください。
