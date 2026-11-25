---
name: daihon
description: >-
  シナリオ駆動の開発ハーネス（scenario CLI）を呼び出して、シナリオ登録・見張り実行・状況表示・
  足りない機能の実装ループを行うための Codex 連携スキル。
---

# Daihon Workflow (`codex`)

`codex` セッション内からシナリオ駆動の開発ハーネス（`daihon` CLI）を呼び出す手順です。
`.agents/skills/daihon/SKILL.md` は `agy` と共通配置でき、Codex からも自動で検出されます。

## コマンド一覧

- **状況の確認**: `daihon status`（または `daihon status --json`）
- **シナリオ登録**: `daihon register "<自然言語のシナリオ>"`（既存シナリオとの重複・優先度案を確認後に保存）
- **見張り実行**: `daihon watch`（開発役とは別エージェントで自動検証し、結果を `scenarios/*.md` へ書き戻す）
- **足りない機能の起票・修正**: `daihon fix --plan-only` / `daihon fix`
- **形式検証**: `daihon validate`
- **TUI 表示（ユーザー操作用）**: `daihon tui`

## 注意
- 開発作業を `codex` で行っている場合、見張り役には別エージェント（例: `--watcher agy` または `.daihon.toml` の `agents.watcher = "agy"`）を指定してください。
