---
name: discord-notify
description: Discord の webhook（環境変数 DISCORD_WEBHOOK_URL）へ通知を送る。本文を 2000 文字の上限内に割って送り、拒否されたら失敗にする。PR 作成後・ルーティンの終了時など Discord へ通知するときに使う。
---

# discord-notify — 通知を上限内に割って送る

Discord のメッセージは 1 通 2000 文字まで。**上限を超えた本文は http 400 で落ちるか、
文字数を確かめずに送ると末尾が切れて届く**（pr-207 は 400、pr-443 は 2089 文字を 204 のまま送った）。
どちらも `curl` を直に叩いたことが原因なので、**送るときは `curl` を直に叩かずこのスクリプトを通す**。

```bash
printf '%s' "$CONTENT" | bash .claude/skills/discord-notify/send.sh
```

- 行の境界で 1900 文字以下に割り、複数通になるときは頭に `(i/n)` を付ける
- 1 通でも 2xx 以外が返れば、その番号を出して失敗する（`curl` が 204 を返したかの確認も不要）
- `DISCORD_DRY_RUN=1` を付けると送らず、割った 1 通ごとの文字数だけ出す
- 検査: `bash .claude/skills/discord-notify/send-test.sh`
