---
id: sample-project-workflow
title: 試験台 CLI でセッション作成・引き継ぎ・再開の一連の流れが通る
priority: P1
status: pending
last_checked_at: null
last_checked_by: null
notified_pass_at: null
tags:
  - testbed
  - auto-only
---

## 概要
ハーネスの見張り役・開発ループ・通知を安全に検証するための最小試験台シナリオ。
3 つのステップすべてをローカルコマンドで自動検証（`auto`）できるようにし、
途中で機能を欠いた状態からの停止検知と、補完後の全ステップ通過を確かめる。

## ステップ

### Step 1: セッションを作成して一覧に表示できる
- status: pending

- [ ] [auto] s1-c1: `sample-cli init demo` が終了コード 0 でセッションファイルを作成する
- [ ] [auto] s1-c2: `sample-cli list` の出力に `demo` が含まれる

### Step 2: 別ホスト想定のコンテキストへタスクを引き継げる
- status: pending

- [ ] [auto] s2-c1: `sample-cli handoff demo --target ubuntu` が終了コード 0 で引き継ぎ記録を保存する
- [ ] [auto] s2-c2: `sample-cli status demo` の出力に `target: ubuntu` が表示される

### Step 3: セッションを再開して完了状態にできる
- status: pending

- [ ] [auto] s3-c1: `sample-cli resume demo` が直前の引き継ぎ先と状態を復元して終了コード 0 で返る
