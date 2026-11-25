# 試験台プロジェクト（`examples/sample-project`）

ハーネスの **段階 4（見張り）** と **段階 5（足りない機能を作るループ）** を安全に検証するための最小 CLI プロジェクトです。
終盤の既存 OSS には触れず、この試験台で動作を確かめます。

- 対応シナリオ: [02-sample-project-workflow.md](../scenarios/02-sample-project-workflow.md)
- 初期状態の実装状況:
  - `init` / `list`（Step 1）: 実装済み
  - `handoff`（Step 2）: **意図的に未実装**（見張り役が Step 2 で停止する確認用）
  - `status` / `resume`（Step 2〜3）: 実装済み
