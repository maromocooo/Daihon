---
name: daihon
description: >-
  シナリオ駆動の開発ハーネス（scenario CLI）を使って、自然言語からのシナリオ登録、
  別エージェントによる裏検証（見張り）、進捗状況の確認、足りない機能のタスク起票と修正ループを実行する。
---

# Daihon Workflow (`agy`)

対象プロジェクトでユーザーが本当に試したい使い方（シナリオ）が後回しにならないよう、`daihon` CLI を通じて登録・見張り・確認を行います。

## 利用するコマンド

1. **現在の状況を見る（非対話）**
   ```bash
   daihon status
   daihon status --json
   ```
   TUI を人が開くときは `daihon tui` を案内します。

2. **自然言語から新しいシナリオ（台本）／ト書き（Togaki）を登録する**
   大きな E2E 体験（台本）は `register`、親シナリオにぶら下げる枝葉の条件・非機能要件・派生機能は `togaki` を使います。
   ```bash
   daihon register "<ユーザーの自然な説明>"
   daihon togaki <parent-id> "<枝葉の条件や派生ユースケース>"
   ```
   ※ 自動確定（`--yes`）はユーザーが明示したときのみ使い、通常は重複・優先度案をユーザーに見せて確認します。

3. **見張り役（別エージェント）で自動検証する**
   ```bash
   daihon watch
   daihon watch --scenario <scenario-id>
   ```
   見張り役は `.daihon.toml` で開発役と別のエージェント（例: `codex`）が指定されています。

4. **止まったステップから足りない機能を作るループを回す**
   ```bash
   daihon fix --plan-only       # タスク起票のみ
   daihon fix                   # 承認を挟んで開発役→再見張り
   ```

5. **形式チェック**
   ```bash
   daihon validate
   ```
