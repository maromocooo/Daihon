---
id: rio-herdr-reconnect-latency
title: 再接続とホスト切替の遅延が 500ms 以内に収まる
kind: togaki
parent: rio-herdr-cross-machine
attach_step: 3
priority: P1
status: pending
last_checked_at: null
last_checked_by: null
notified_pass_at: null
tags:
  - performance
  - togaki
---

## 概要
親シナリオ（Rio から herdr を使い Mac Studio と Ubuntu をまたいで開発を続ける）が開通した前提のうえで、
ネットワーク瞬断からの再接続やホスト間のコンテキスト切替が体感ストレスのない 500ms 以内に収まることを検証するト書き。

## ステップ

### Step 1: ホスト切替・再接続の応答遅延が目標値以内である
- status: pending

- [ ] [auto] s1-c1: ホスト切替コマンドを 5 回計測した p95 応答時間が 500ms 以内である
- [ ] [manual] s1-c2: MacBook Air の Rio 上でペイン切替時にもたつきを感じない
