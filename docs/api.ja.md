# HTTP / WebSocket API

[English](api.md)

`ssp serve` がこの API を提供します。`ssp` CLI もこの API を使っているので、CLI でできることはすべて自作の
プログラムからもできます。

- ベース URL: `http://127.0.0.1:7920/api/v1`（設定の `listen` で変更できます）
- 特に記載がなければ、ボディは JSON です。成功した操作は `204 No Content` を返します。
- エラー時は `{"error": "..."}` という JSON を返します（形式の誤ったリクエストや、存在しないパスも同じです）。
  JSON のボディで項目名を書き間違えると、無視されずにエラーになります。

## 認証

| 設定 | 必要なもの |
|---|---|
| `token` なし（既定。ループバックのみ） | 認証情報は不要です。`Host` または `Origin` ヘッダーがループバックでないリクエストには `403` を返すため、Web ページがブラウザ経由で API を使うことはできません。 |
| `token = "..."` | `Authorization: Bearer <token>` を送るか、ヘッダーを付けられない場合（ブラウザの WebSocket など）は `?token=<token>` を付けます。なければ `401` です。 |

### `ssp web` で表示するページ

`ssp web`（または `POST /displays/{id}/web`）で表示するページは、デーモンの数値を読んで、自分のダッシュボードを描けます。
デーモンは、ページのスクリプトが動く前に、次の値をページに渡します。

```js
window.ssp = { api: "http://127.0.0.1:7920/api/v1", token: "..." }
```

ページは `Authorization: Bearer <ssp.token>` を付けて、`/health`・`/displays`・`/displays/{id}`・`/metrics`・
`/metrics/{id}`・`/system` を `GET` できます。ページのオリジンは問いません（ファイルなら `null`）。これらの応答には
`Access-Control-Allow-Origin` が付き、これらのパスへのプリフライトリクエストにも応答します。このトークンでそれ以外を
リクエストすると `403` です。ページから、表示する内容・明るさ・メトリクスを変えることはできません。

```js
const response = await fetch(`${ssp.api}/metrics/claude-code`, {
  headers: { Authorization: `Bearer ${ssp.token}` },
});
```

トークンはページごとに新しく作り、表示している間だけ有効です（`reload` で読み込み直しても同じトークンのままです）。
ディスクやログには書きません。ページが読み込む別のサイトにトークンを渡すと、そのサイトも表示中は同じ数値を読めるので、
信頼できるページを表示してください。[contrib/web/system.html](../contrib/web/system.html) が例です。

インターネット上のページからこの PC のデーモンへのリクエストは、Chrome ではローカルネットワークへのアクセスとして扱われ、
許可がないとできません。デーモンは表示するページのオリジンにだけこの許可を与えるので、Web サイトのページ
（[画面ギャラリー](https://subscreen.dev/ja/screens/)など）も数値を読めます。0.5.0 までは許可を与えておらず、
こうしたページには応答が届きませんでした。

## ディスプレイ ID

ID は `d92-470B03781D1F`（ドライバ ID ＋ USB シリアル番号）のような形で、`GET /displays` で一覧できます。
`default` は、接続中のディスプレイのうち ID 順で最初のものを指します。

## エンドポイント

| メソッド | パス | ボディ | 動作 |
|---|---|---|---|
| `GET` | `/health` | | デーモンの状態とバージョン |
| `POST` | `/reload` | | 設定ファイルを読み直して反映します（後述） |
| `GET` | `/displays` | | デーモン起動後に見つかったすべてのディスプレイ |
| `GET` | `/displays/{id}` | | 1つのディスプレイ |
| `POST` | `/displays/{id}/image` | 画像ファイル | 画像を表示（後述） |
| `POST` | `/displays/{id}/clock` | 任意の JSON | 組み込みの時計を表示 |
| `POST` | `/displays/{id}/dashboard` | 任意の JSON | 組み込みのダッシュボードを表示 |
| `POST` | `/displays/{id}/brightness` | `{"percent": 0-100}` | バックライトの明るさを設定 |
| `POST` | `/displays/{id}/power` | `{"on": true \| false}` | 画面をオン／オフ |
| `POST` | `/displays/{id}/clear` | | 表示中のコンテンツを止めて、画面を消去 |
| `POST` | `/displays/{id}/stop` | | 表示中のコンテンツを止める（画面は最後の絵のまま） |
| `GET` | `/displays/{id}/stream` | | ライブフレーム用の WebSocket（後述） |
| `POST` | `/displays/{id}/notify` | JSON | 画面の上にメッセージをしばらく重ねて出す（後述） |
| `DELETE` | `/displays/{id}/notify` | | 通知を消す |
| `POST` | `/displays/{id}/web` | `{"url": "...", "reload": 600}` | Web ページを表示（後述） |
| `POST` | `/displays/{id}/layout` | JSON | 複数の内容を並べて表示（後述） |
| `GET` | `/web/chrome` | | ヘッドレス Chrome の準備ができているか |
| `POST` | `/web/chrome` | | ヘッドレス Chrome をダウンロード |
| `PUT` | `/metrics/{id}` | JSON | ダッシュボードの `metric:<id>` パネルに出す値を設定（後述） |
| `GET` | `/metrics` | | すべてのメトリクス |
| `GET` | `/metrics/{id}` | | 1つのメトリクス |
| `DELETE` | `/metrics/{id}` | | メトリクスを削除 |
| `GET` | `/system` | | CPU・メモリ・通信・ディスクの数値（後述） |
| `GET` | `/schedule` | | スケジュールの直前と次のエントリー（後述） |
| `POST` | `/schedule/pause` | | 時刻による切り替えを止める |
| `POST` | `/schedule/resume` | | 今の状態を当てはめて切り替えを再開する |

### `GET /health`

```json
{"status": "ok", "version": "0.6.0", "drivers": ["d92"], "started": "2026-10-09T06:30:00Z"}
```

`drivers` は、デーモンが使うドライバの ID の一覧です。デーモンはこれらのドライバのディスプレイにしか触りません
（[コマンドラインガイド](cli.ja.md)の `--driver` を参照）。0.1.1 より前のデーモンは返しません。`started` は、
デーモンが起動した時刻、または最後に設定を反映し直した時刻です（0.6.0 より前のデーモンは返しません）。
`reload_error` は、設定の読み直しが反映されなかったときだけ返り、新しい設定で起動できずに元の設定に戻った理由を示します。

### `POST /reload`

デーモンが起動時に読んだ設定ファイルを（同じコマンドラインオプションで）読み直し、その設定で起動できるなら、同じ
プロセスのまま新しい設定で動き直します。誤りのあるファイルには、誤りを添えて `400` を返し、何も変えません。
そうでなければ、先に次の応答を返します。

```json
{"listen": "127.0.0.1:7920"}
```

そのあとデーモンは画面と API をいったん止め、新しい設定で起動し直して `listen` で待ち受けます。`GET /health` の
`started` が新しくなれば完了です。ディスプレイは消灯せず、新しい設定で起動時に表示するものを表示します。API から
表示した内容は引き継がれず、メトリクスは引き継がれます。`ssp config reload` は、これらを行って完了を待ちます。

### `GET /displays`

```json
[
  {
    "id": "d92-470B03781D1F",
    "driver": "d92",
    "model": "upHere D92",
    "serial": "470B03781D1F",
    "firmware": "V25.upHere_gamingD92.02.014",
    "connected": true,
    "width": 1920,
    "height": 462,
    "content": "clock",
    "capabilities": {
      "live_frames": true, "saved_frames": true, "brightness": true,
      "power": true, "clear": true, "max_fps": 60
    },
    "stats": {
      "submitted": 925, "shown": 908, "partial": 850, "dropped": 17, "duplicates": 0,
      "last_encode_ms": 5.39, "last_send_ms": 21.85, "last_bytes": 50940
    }
  }
]
```

- `width` x `height` が描画すべきサイズです（横長）。ほかのサイズはパネルに合わせて調整されます。
- `content` は `nothing`・`image`・`animation`・`video`・`web`・`clock`・`dashboard`・`rotation`・`layout`・`stream` のいずれかです。ディスプレイが抜かれている間も保持されます
  （そのとき `connected: false`、`stats: null`）。
- `stats`: `dropped` は送る前に新しいフレームに置き換えられた数、`duplicates` は変化がなかったため送らなかった数、`partial` は変わった部分だけを送った数です（`[display] partial_updates`）。
- `notification` は通知を出している間だけ入ります（後述）:
  `{"text": "CI が失敗しました", "style": "banner", "color": "#dc2626", "seconds_left": 8, "sticky": false}`。
  2行目があれば `detail` が入り、`sticky` のときは `seconds_left` がありません。

### `POST /displays/{id}/image`

ボディは PNG・JPEG・GIF・WebP の画像ファイルです（64 MiB まで）。アニメーション GIF・APNG・WebP は、コマごとの間隔で
繰り返し再生します（このときディスプレイの `content` は `animation` になります）。10,000 コマまで対応します。
展開すると 256 MiB を超えるアニメーション（画面全体の 60fps で約1秒を超えるもの）は、メモリに置かず、再生しながら
展開し直します。クエリパラメータ:

| パラメータ | 値 | 既定値 |
|---|---|---|
| `fit` | `contain`（全体を収めて余白は黒）、`cover`（はみ出しを切り取り）、`stretch`（引き伸ばし） | `contain` |
| `persist` | `true` でデバイスにも保存（電源を切っても残る。フラッシュに書き込みます）。アニメーションは最初のコマ | `false` |

```sh
curl --data-binary @photo.png "http://127.0.0.1:7920/api/v1/displays/default/image?fit=cover"
```

ボディには動画（MP4・QuickTime・WebM・Matroska・AVI・MPEG-TS、4 GiB まで）も送れます。PC に入っている
[ffmpeg](https://ffmpeg.org/) で繰り返し再生します（このとき `content` は `video`）。再生中、デーモンはファイルを一時フォルダに
置き、応答する前に ffmpeg で読めるかを確かめます。再生できないファイルには ffmpeg のメッセージ付きで `400` を、
ffmpeg が見つからないときはインストール方法付きで `501` を返します。動画に `persist` は使えません。ffmpeg は、設定ファイルの
`[video] ffmpeg`、`PATH`、よく使われるインストール先（`/opt/homebrew/bin`・`/usr/local/bin`・`/usr/bin`・`/snap/bin`、
Windows では WinGet・Scoop・Chocolatey のフォルダ）の順に探します。

### `POST /displays/{id}/clock`

すべての項目は省略可能で、省略した項目は設定ファイルの `[clock]` の値になります。

```json
{
  "seconds": false,
  "time_format": "%H:%M",
  "date_format": "%A, %B %-d",
  "color": "#FFD080",
  "background": "#101018"
}
```

書式は strftime 形式です（[リファレンス](https://docs.rs/jiff/latest/jiff/fmt/strtime/)）。
`date_format` を空にすると日付を表示しません。`weekdays`（日曜日から順に7つの名前）を指定すると、`%a` と `%A` が
その名前になります（例: `["日", "月", "火", "水", "木", "金", "土"]`）。

### `POST /displays/{id}/dashboard`

すべての項目は省略可能で、省略した項目は設定ファイルの `[dashboard]` の値になります。時計のパネルは `[clock]` の書式を
使います。

```json
{
  "widgets": ["clock", "cpu", "memory", "network", "disk"],
  "color": "#F0F2F8",
  "accent": "#6EE7B7",
  "background": "#000000"
}
```

`widgets` は左から並べるパネルです（`clock`・`cpu`・`memory`・`network`・`disk`・`claude-code`・`metric:<id>` から1〜6個。
`claude-code` は [CLI ガイド](cli.ja.md#claude-code-の使用量を表示する)を参照）。`accent` はグラフの色です。

### `GET /schedule`・`POST /schedule/pause`・`POST /schedule/resume`

設定ファイルのスケジュール（`[[schedule]]`。[コマンドラインガイド](cli.ja.md)を参照）です。

```json
{
  "entries": 4,
  "paused": false,
  "last": { "at": "2026-10-09T19:00:00+09:00", "entries": [2], "does": ["show clock, brightness 40"] },
  "next": { "at": "2026-10-10T01:00:00+09:00", "entries": [3], "does": ["power off"] }
}
```

`last` と `next` の `entries` は設定ファイルでの順番（1 から）、`at` はローカル時刻です。スケジュールがなければ `entries`
は 0 で、`last` と `next` はありません。`pause` で時刻による切り替えを止め、`resume` で今の時点の状態をすべての
ディスプレイに当てはめて再開します。スケジュールがないときは、どちらも `409` を返します。

### `POST /displays/{id}/notify`・`DELETE /displays/{id}/notify`

表示中の内容の上にメッセージを重ねて出し、そのあと元の表示に戻します。下の内容は動き続けます。新しい通知は今の通知を置き換えます。

```json
{ "text": "CI が失敗しました", "detail": "main · 41 件中 2 件のジョブ", "seconds": 30, "color": "red" }
```

| 項目 | 意味 |
|---|---|
| `text` | メッセージ（1〜80 文字）。必須です。 |
| `detail` | その下の小さな行（120 文字まで）。 |
| `seconds` | 出しておく秒数。1〜86400（既定は 10）。 |
| `sticky` | `true` なら、`seconds` の代わりに、消すか置き換えるまで出しておきます。 |
| `style` | `banner`（下3分の1、既定）か `full`（パネル全体）。 |
| `color` | 背景色。`red`・`orange`・`yellow`・`green`・`blue`（既定）・`gray`、または `#rrggbb`。 |
| `wake` | `true` なら、消灯中の画面を通知の間だけ点灯し、終わったらまた消灯します。 |

通知は表示内容ではなくディスプレイに属します。表示内容を切り替えても、ディスプレイを抜いている間も残ります（戻ったときに、
まだ終わっていなければ再び出します）。何も表示していないとき（`content` が `nothing`、または `stop` のあと）は、最後の画面の
上に描きます（なければ黒の上）。`POST /displays/{id}/clear` で表示内容とともに消え、`DELETE /displays/{id}/notify` ですぐに
消えます（通知がなくても `204`）。ページ用トークンでは、`GET /displays` で読めますが、通知は出せません。

### メトリクス

メトリクスは、自分のスクリプトから送る値です。ダッシュボードの `metric:<id>` パネルに表示されます。CI の状態、キューの長さ、
価格、室温など、何でも構いません。値が変わるたびに送れば、1秒以内にパネルが描き直されます。

```sh
curl -X PUT http://127.0.0.1:7920/api/v1/metrics/ci \
  -H 'Content-Type: application/json' \
  -d '{"label": "CI", "value": 2, "unit": "failed", "detail": "main · 39 of 41 jobs passed"}'
```

`PUT /metrics/{id}` の項目はすべて省略可能で、省略した項目は前の値のままです。新しく作るときは `value`・`text`・`series`
のどれかが必要です。

| 項目 | 型 | 意味 |
|---|---|---|
| `value` | 数値 | 表示する値。送るたびにパネルのグラフにも追加されます（直近60個を保持）。 |
| `text` | 文字列（40文字まで） | 数値の代わりに表示する短い文字列（例: `"passing"`）。グラフは消えます。 |
| `series` | 数値の配列（1000個まで） | グラフの値を古い順に置き換えます。`value` を同時に送らなければ、最後の値が `value` になります。 |
| `label` | 文字列（40文字まで） | 値の上に出す名前。省略時は id。 |
| `unit` | 文字列（16文字まで） | 値の後ろに小さく出す単位（例: `"%"`）。 |
| `detail` | 文字列（120文字まで） | 値の下の行。 |
| `max` | 数値 | グラフの上端。省略時はグラフ中の最大値。 |
| `ttl` | 秒 | 値を最新とみなす時間（既定 300、最大1週間）。過ぎるとパネルが暗くなり、何分前の値かを表示します。 |

100,000 以上の数値は、パネルでは短く表示します（`123k`・`4.56M`・`7.8B`）。id は `a-z`・`0-9`・`-` の1〜32文字です。デーモンが持てるメトリクスは64個までで、メモリ上にだけ保持します。デーモンを
再起動すると消えるので（`POST /reload` では消えません）、もう一度送ってください（送る側はたいてい定期実行なので、そのまま戻ります）。まだ値が届いていない
`metric:<id>` パネルには「waiting for data」と表示されます。

`GET /metrics` はすべてのメトリクスを id 順で、`GET /metrics/{id}` は1つを返します。

```json
{
  "id": "ci",
  "label": "CI",
  "value": 2.0,
  "unit": "failed",
  "detail": "main · 39 of 41 jobs passed",
  "ttl": 300,
  "updated": "2026-10-08T10:24:49.809733Z",
  "age": 12,
  "stale": false,
  "history": [0.0, 3.0, 2.0]
}
```

`text` を設定したときは `value` の代わりに `text` が入り、`max` は設定したときだけ入ります。`age` の単位は秒です。
`DELETE /metrics/{id}` は、存在しない id に `404` を返します。

ダッシュボードに `claude-code` パネルを表示するか、`GET /metrics/claude-code` でメトリクスを読むと、それ以降はデーモン自身がメトリクス `claude-code` を更新し続けます（今の5時間
ブロックのトークン数、`detail` にブロックの終わる時刻と今日の合計、`history` に直近1時間の1分ごとのトークン数）。

### `GET /system`

ダッシュボードが描く数値です。

```json
{
  "cpu_percent": 12.5,
  "cpu_count": 10,
  "load": 2.31,
  "memory_used": 25769803776,
  "memory_total": 68719476736,
  "rx_per_sec": 1250000.0,
  "tx_per_sec": 84000.0,
  "disk_used": 512000000000,
  "disk_total": 994662584320
}
```

CPU 使用率（0〜100、全コア）と通信速度（ループバックを除く全インターフェースの毎秒のバイト数）は、前回のリクエストからの
平均です。1秒に1回くらい読んでください。メモリとディスクの単位はバイトで、ディスクはシステムのあるもの（`/`、Windows では
`C:\`）です。`load`（1分間のロードアベレージ）は Windows では `null`、ディスクが見つからないときは `disk_used` と
`disk_total` が `null` です。

### `POST /displays/{id}/layout`

設定ファイルの `[layout]` と同じ形で、領域を並べて表示します（このとき `content` は `layout`）。

```json
{
  "zones": [
    { "show": "clock", "width": 0.34 },
    { "show": "image", "image": "/home/me/loop.mp4", "fit": "cover" },
    { "show": "metric:ci", "width": 480 }
  ],
  "gap": 16,
  "background": "#000000"
}
```

`show` は、ダッシュボードのパネル（`clock`・`cpu`・`memory`・`network`・`disk`・`claude-code`・`metric:<id>`）、
`dashboard`、`image`（`image` に、デーモンのある PC 上の画像・アニメーション・動画のファイル）、`web`（`url` と `reload`）、
`nothing` のいずれかです。`width` はパネルに対する割合（1 まで）かピクセルで、指定のない領域は残りを分け合います。領域の
端は 16 ピクセル単位にそろえます。領域は 8 つまで、動画と Web ページはそれぞれ1つまでで、超えたときやファイルを読めない
ときは `400` を返します。

### `POST /displays/{id}/web`

Web ページを、ヘッドレス Chrome でパネルの大きさに描いて表示します（このとき `content` は `web`）。

```json
{ "url": "https://example.com/status", "reload": 600 }
```

`url` は `http`・`https`・`file` の URL です。`reload`（省略可）を指定すると、その秒数ごとにページを読み込み直します。
ブラウザは応答の後に起動します。読み込めないページは、ディスプレイとデーモンのログに表示されます。ブラウザがない
ときは `409` を返します（`[web] auto_download = true` なら、先にダウンロードします）。

### `GET /web/chrome`・`POST /web/chrome`

`GET` はブラウザの準備ができているかを返します。`POST` は Chrome for Testing から最新のヘッドレス Chrome を
ダウンロードします（約 100 MB。終わるまで応答を待ち、失敗したら `502`）。どちらも次の形で返します。

```json
{
  "installed": true,
  "path": "/Users/me/Library/Application Support/sub-screen-player/chrome/155.0.8059.39/chrome-headless-shell-mac-arm64/chrome-headless-shell",
  "version": "155.0.8059.39",
  "configured": false,
  "dir": "/Users/me/Library/Application Support/sub-screen-player/chrome"
}
```

ブラウザが `[web] chrome` の指定によるものなら `configured` は `true` で、`version` はありません。

## WebSocket ストリーム

`GET /displays/{id}/stream` は WebSocket に切り替わります。**バイナリ**メッセージ1つが1フレームです。

| `format` クエリパラメータ | メッセージの中身 |
|---|---|
| `image`（既定） | 任意サイズの画像ファイル（PNG・JPEG・GIF・WebP）。`fit` でパネルに合わせます |
| `rgb` | 8 ビット RGB の生データ。ちょうど `width * height * 3` バイト |
| `rgba` | 8 ビット RGBA の生データ。ちょうど `width * height * 4` バイト。アルファは黒の上に合成します |

- 送る頻度は自由です。ディスプレイは表示できる範囲で最新のフレームを出し（最大 `max_fps`）、古いものは捨てます。
  デーモンにとって最も処理が軽いのは、パネルとほぼ同じサイズの JPEG です。
- ストリームが接続されている間、ディスプレイの `content` は `stream` になります。ほかのコンテンツ（画像、時計、ダッシュボード、`stop`）を
  設定するとストリームは終了し、次に送られたフレームにはクローズコード `4000` で応答します。
- 不正なフレームがあってもストリームは終わりません。デーモンはテキストメッセージ `{"error": "..."}` を返して続行します。
- ディスプレイを挿し直してもストリームは続きます。抜かれている間に送られたフレームにはエラーを返します。
- デーモンが終了するとストリームも閉じられます。

ブラウザでの例（`localhost` から配信されるページ）:

```js
const ws = new WebSocket("ws://127.0.0.1:7920/api/v1/displays/default/stream");
const canvas = Object.assign(document.createElement("canvas"), { width: 1920, height: 462 });
const ctx = canvas.getContext("2d");

ws.onopen = () => setInterval(() => {
  ctx.fillStyle = "#000";
  ctx.fillRect(0, 0, 1920, 462);
  ctx.fillStyle = "#fff";
  ctx.font = "200px sans-serif";
  ctx.fillText(new Date().toLocaleTimeString(), 80, 300);
  canvas.toBlob((blob) => ws.send(blob), "image/jpeg", 0.85);
}, 1000 / 30);
ws.onmessage = (event) => console.warn("ssp:", event.data);
```

`localhost` のページからは WebSocket を使えます。HTTP のエンドポイントはまだ CORS ヘッダーを返さないため、
Web ページからではなく、スクリプトや CLI から呼び出してください。
