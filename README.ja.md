<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/daihon-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="assets/daihon-logo-light.svg">
    <img alt="Daihon" src="assets/daihon-logo-light.svg" width="320">
  </picture>
</p>

<p align="center">
  <a href="README.md">English</a> | <strong>日本語</strong>
</p>

# Daihon（シナリオ駆動の開発ハーネス）

コーディングエージェントに開発を任せた際に「一番試したい使い方（シナリオ）」が後回しになったりズレたりする問題を防ぐための、**自然言語登録・別エージェント見張り・最小 TUI・ドリフト検知・開発ループ・スマホ通知（ntfy）・コマンド連携**ハーネスです。

> **シナリオの YAML / Markdown は手で書く必要はありません。**  
> `daihon register "〜したい"` と自然言語で話すだけで、保存形式（Markdown + YAML front matter）へ自動変換され、既存シナリオとの重複チェックと優先度案が提示されます。

## ドキュメント
- **新規・既存プロジェクトへの組み込み手順と日常運用**: [docs/getting-started.md](docs/getting-started.md)
- **シナリオ保存形式の仕様**: [docs/scenario-format.md](docs/scenario-format.md)

## インストール
```bash
git clone git@github.com:maromocooo/Daihon.git
cd Daihon
cargo install --path .
```

## 最小の導入手順（新規・既存プロジェクト共通）
```bash
cd /path/to/your-project
daihon init                         # .daihon.toml と scenarios/ を作成
echo '.daihon/' >> .gitignore
daihon init-integration agy         # agy / codex / claude から選択して導入

# 1. 大きな本筋シナリオ（台本）を自然言語で登録
daihon register "MacBook Air でアプリを開いて Ubuntu 端末の Blender を開き編集作業が行える"

# 1b. 本筋にぶら下げる品質条件・派生機能を「ト書き（Togaki）」として追加（親は壊れません）
daihon togaki <parent-id> "遅延が 50ms 以内に収まること"
daihon togaki <parent-id> "Air のストレージにある素材を開いている Blender に引き渡せること"

# 2. 開発役とは別のエージェントで裏検証
daihon watch

# 3. 最小 TUI で一覧・優先度・停止位置を確認
daihon tui

# 4. 止まったステップから「足りない機能を作る」タスク起票・修正ループ
daihon fix --plan-only              # タスク起票のみ
daihon fix                          # 承認を挟んで開発役 → 見張り役で再検証
```

## 主なコマンド一覧

| コマンド | 役割 |
| :--- | :--- |
| `daihon init` | `.daihon.toml` と `scenarios/` を作成 |
| `daihon validate [path]` | 保存形式と状態整合性をチェック（段階 1） |
| `daihon tui [--dir ...]` | 最小 TUI で一覧・優先度・ステップ状況・`[DRIFT]` 警告を表示（段階 2） |
| `daihon status [--json]` | 非対話で一覧・停止箇所・ドリフト状態を表示（段階 2） |
| `daihon context` | 開発役エージェントへ渡す現在地・ドリフト要約を出力 |
| `daihon register "<自然言語>"` | 自然言語から大きな E2E シナリオを生成・重複検出・優先度案提示（段階 3） |
| `daihon togaki <親ID> "<自然言語>"` | 既存シナリオに枝葉の品質条件・派生機能をト書き（`Togaki`）として紐づけ追加 |
| `daihon watch [--if-changed \| --background \| --interval 15m \| --stop \| --daemon-status]` | 開発役とは別エージェントで見張り実行・裏デーモン管理（段階 4） |
| `daihon check <id> <check-id> --pass` | `[manual]` 項目などの手動チェック更新（段階 4） |
| `daihon hook install` | git `post-commit` にバックグラウンド見張りを設置（段階 4） |
| `daihon fix [--plan-only \| --auto-approve]` | 停止ステップからタスク起票→開発役→再見張り（段階 5） |
| `daihon notify test [--dry-run]` | `ntfy` スマホ通知の疎通テスト（段階 6） |
| `daihon init-integration <agy\|codex\|claude> [--no-rules \| --no-hooks]` | Skill・Always-On ルール・ライフサイクルフックを安全にマージ導入（段階 7） |

## 開発・テスト
```bash
cargo test && cargo clippy --all-targets -- -D warnings
```
