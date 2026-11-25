---
name: daihon
description: >-
  シナリオ駆動の開発ハーネス（scenario CLI）を Claude Code から呼び出し、
  シナリオの登録・見張り実行・状況確認・足りない機能のタスク起票を行う。
---

# Daihon Workflow (`claude`)

Claude Code 内から `daihon` CLI を呼び出して、シナリオ駆動で開発を進めます。

## 主な操作
- 状況確認: `daihon status`（詳細確認はユーザーへ `daihon tui` を案内）
- 形式チェック: `daihon validate`
- 自然言語でシナリオ登録: `daihon register "<自然言語>"`（重複と優先度案を必ずユーザーに共有）
- 親シナリオへ枝葉（ト書き）を追加: `daihon togaki <parent-id> "<追加条件・派生機能>"`
- 別エージェントによる見張り実行: `daihon watch`（既定では `codex` など Claude 以外のエージェントが検証）
- 止まったステップから足りない機能のタスク起票・修正: `daihon fix --plan-only` または `daihon fix`
- コミット時の自動見張りフック設置: `daihon hook install`
