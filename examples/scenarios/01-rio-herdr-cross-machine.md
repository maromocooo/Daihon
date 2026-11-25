---
id: rio-herdr-cross-machine
title: Rio から herdr を使い、Mac Studio と Ubuntu をまたいで開発を続ける
priority: P0
status: failed
last_checked_at: 2026-11-25T09:30:00+09:00
last_checked_by: codex
notified_pass_at: null
tags:
  - dogfooding
  - multi-machine
  - core-workflow
---

## 概要
MacBook Air 上のターミナル Rio から herdr（複数のコーディングエージェントを並べて動かす作業環境）を起動し、
Mac Studio と Ubuntu（RTX 4090）をまたいで日常の開発作業を途切れずに続けられること。
テキストエディタやブラウザといった小さな部品ではなく、プロダクト本来の目的である
「3 台を 1 つの環境のように使って日常の開発を続ける」中核ワークフローを最優先で確かめる。

## ステップ

### Step 1: Rio から herdr セッションを起動できる
- status: ready_for_manual
- summary: CLI・PTY 経由の自動検証は通過。Rio 実機での描画確認のみ未実施。

- [x] [auto] s1-c1: `herdr --version` が終了コード 0 で返る
- [x] [auto] s1-c2: PTY 経由で herdr セッションを新規作成し、セッション一覧に表示される
- [ ] [manual] s1-c3: MacBook Air の Rio ウィンドウ上で複数ペインの表示崩れなく操作できる

### Step 2: Mac Studio と Ubuntu のセッションへ透過的に接続できる
- status: failed
- failed_check: s2-c2

- [x] [auto] s2-c1: Mac Studio へ SSH 経由で接続し、対象ワークスペースのセッション状態を取得できる
- [!] [auto] s2-c2: Ubuntu (RTX 4090) 側へ作業コンテキストを引き継ぎ、ビルドコマンドを実行できる
  - reason: Ubuntu 側へセッションコンテキストを引き継ぐサブコマンドが未実装のため終了コード 2 で失敗

### Step 3: マシンをまたいで作業を中断・再開できる
- status: pending

- [ ] [auto] s3-c1: 接続断のあとに再接続しても、herdr のセッション一覧と直前の作業ディレクトリが復元される
- [ ] [manual] s3-c2: MacBook Air 側から途中経過を確認し、違和感なく開発作業を再開できる
