# 導入・運用ガイド（新規プロジェクト／既存プロジェクト）

## よくある疑問：YAML や Markdown は手で書く必要がありますか？

**いいえ、手で書く必要はありません。**

普段は `daihon register "〜したい"` と自然言語で話しかけるだけで、登録エージェントが以下を自動生成します。

1. 人間が読める自然言語の概要（`## 概要`）
2. 検証可能なステップと OK 条件チェック項目（`auto` / `manual` 分類つき）
3. 既存シナリオとの重なり箇所と、優先度案（`P0`〜`P3`）

保存形式が素の Markdown + YAML front matter なのは、**人が読んだり Git で差分レビューしたり微調整したりしやすくするため**です。最初から手で書く必要はなく、エージェントが作ったものを確認・承認して足していけます。

---

## 初回セットアップ（マシン全体で 1 回だけ）

### 1. `daihon` コマンドをインストール
このリポジトリのルートで実行すると、`~/.cargo/bin/daihon` が入ります。

```bash
cargo install --path .
daihon --version
```

### 2. スマホ通知（ntfy）の設定（任意）
1. iOS / Android に [ntfy](https://ntfy.sh/) アプリを入れ、自分だけのトピック名（推測されにくい名前を推奨）を購読する。
2. `~/.zshrc` に環境変数を追加しておく：
   ```bash
   export DAIHON_NTFY_TOPIC="your-private-topic"
   ```
3. 疎通テスト：
   ```bash
   daihon notify test
   ```
   ※ 本番の運用中は**あるシナリオが全ステップ通過した瞬間（または `manual` 項目を含むシナリオで全 `auto` 項目が通った瞬間）だけ**通知が届きます。途中のステップや失敗時は通知されません。

---

## ケース A：新規プロジェクトに最初から組み込む

次に作るプロダクトで最初からシナリオ駆動で進めたいときの手順です。

### 1. プロジェクトを初期化してハーネスを入れる
```bash
cd /path/to/your-new-project
git init
daihon init
```
これで以下が作成されます：
- `.daihon.toml`（使うエージェントや通知の設定）
- `scenarios/`（シナリオ Markdown の保存先）

`.gitignore` にハーネスの実行ログ類を追加します（`scenarios/*.md` は Git 管理します）：
```bash
echo '.daihon/' >> .gitignore
```

### 2. 普段使うエージェントの連携（Skill・Always-On ルール・フック）を入れる
使いたいツールを選んで実行します（既存の `AGENTS.md` / `CLAUDE.md` / `hooks.json` / `settings.json` は壊さず安全にマージします）：
```bash
daihon init-integration agy     # SKILL.md + AGENTS.md ルール + .agents/hooks.json (SessionStart/Stop)
daihon init-integration codex   # SKILL.md + AGENTS.md ルール
daihon init-integration claude  # SKILL.md + /daihon-* + CLAUDE.md ルール + .claude/settings.json
```
※ ルールやフックを個別に省きたい場合は `--no-rules` / `--no-hooks` を指定できます。
これにより、**開発セッション開始時に P0 シナリオの現在地とドリフト警告が自動注入**され、**ターン終了時（Stop フック）にコード変更があれば裏で見張り役が自動起動**します。

### 3. 一番最初にドッグフーディングしたい大きな本筋（台本）を登録し、後からト書き（`Togaki`）を生やす
```bash
# まず大きな E2E マイルストーン（本筋）を自然言語で登録
daihon register "MacBook Air でアプリを開いて Ubuntu 端末の Blender を開き編集作業が行える"

# 本筋にぶら下げる品質条件や派生機能を「ト書き（Togaki）」として追加（親ファイルは壊れません）
daihon togaki <parent-id> "遅延が 50ms 以内に収まること"
daihon togaki <parent-id> "Air のストレージにある素材を開いている Blender に引き渡せること"
```
- 親シナリオが未通過の間、子ト書きは **`[親待]`** として自動スキップされ、開発役・見張り役ともに本筋の開通に集中します。
- 親シナリオが通過した直後から、ぶら下がっているト書きが自動的に見張り・開発ループの対象になり、ト書きが全通したタイミングでも `ntfy` 通知が届きます。
- 登録に別のエージェントを使いたい場合は `--agent codex` / `--agent claude` / `--agent agy` でも切り替えられます。

### 4. （任意）裏の定期見張りデーモン／git フックを有効にする
```bash
# git コミット時に裏で見張り実行
daihon hook install

# コミットしない長時間作業中も裏で定期チェック（コード変化がないときは自動スキップ）
daihon watch --interval 15m --background
daihon watch --daemon-status   # 稼働確認
daihon watch --stop            # 停止
```

### 5. 日常の開発サイクル（ドリフト防止ループ）
1. 開発役エージェント（`agy` / `claude` / `codex`）を起動すると、Always-On ルールと `SessionStart` フックが P0 シナリオの現在地と `.daihon/NEXT_TASK.md` を自動で共有する。
2. 開発役が実装を進めると、ターン終了フック・コミットフック・定期デーモンのいずれかが裏で別エージェント（既定: `codex`）の見張り役を走らせる。
3. 同じステップで 3 回連続停止（またはコード変更があるのに 2 回連続前進なし）すると、TUI・`daihon status`・`.daihon/NEXT_TASK.md` に **`[DRIFT]` 警告** が自動出力され、開発役へ軌道修正を促す。
4. 止まったステップから不足機能を作らせたいときは `daihon fix`（タスク起票 → 承認 → 開発役 → 見張り役で再検証）。
5. 全ステップ通った瞬間だけスマホへ `ntfy` 通知が届くので、そこで初めてドッグフーディングする！

---

## ケース B：既存プロジェクトに途中から組み込む

すでに進んでいるリポジトリへ安全に追加する手順です。既存ソースコードには一切手を入れません。

```bash
cd /path/to/existing-project
daihon init
echo '.daihon/' >> .gitignore
daihon init-integration agy   # 必要に応じて codex / claude も
```

1. **まず 1 本だけ登録する**: 「今一番壊れてほしくない／優先したい使い方」を自然言語で `daihon register "..."` に話す。
2. **現在地を測る**: `daihon watch --dry-run-notify` を 1 回流し、`daihon tui` で「どのステップのどの項目で止まっているか」を可視化する。
3. **足りない機能を埋める**: `daihon fix --plan-only` で起票されたタスクファイルを確認し、納得できたら `daihon fix` で開発役へ渡す。
4. **シナリオを足し続ける**: 新しい使い方が思いつくたびに `daihon register "..."` を呼ぶだけ。既存シナリオファイルは一切書き換わらず、重複箇所と優先度案が自動で提示されます。

---

## `.daihon.toml` のよく使うカスタマイズ

### エージェントの役割分担を変える
```toml
[agents]
register = "codex"   # 登録に使うエージェント（agy | codex | claude）
developer = "claude" # 開発役
watcher = "codex"    # 見張り役（既定では developer と別エージェントを強制）
```

### 複数マシン（Mac Studio・Ubuntu 等）へ SSH して自動検証する
`codex` の既定サンドボックス（`workspace-write`）はネットワーク通信を遮断するため、SSH を含む `[auto]` チェックを検証するプロジェクトでは以下を設定します。`[environment]` の内容は見張り役のプロンプトと環境変数へ自動で渡されます。

```toml
[watcher]
timeout_secs = 300
sandbox = "danger-full-access"

[environment]
hosts = ["mac-studio", "ubuntu-4090"]
setup_commands = ["herdr --version"]
notes = """
- MacBook Air がローカル環境
- Mac Studio へは `ssh mac-studio`、Ubuntu (RTX 4090) へは `ssh ubuntu-4090` で接続可能
"""

[environment.vars]
# HERDR_PROFILE = "dogfood"
```

GUI の描画や操作感など自動化しづらい項目は `[manual]` として登録され、見張り役はスキップします。実機で確認できたら以下でチェックを付けられます：
```bash
daihon check <scenario-id> <check-id> --pass
```

### 承認を挟まず開発ループを自動で回す
```toml
[loop_policy]
auto_start_dev = true
max_retries = 2
```
