# HTTP / WebSocket API

[English](api.md)

`ssp serve` がこの API を提供します。`ssp` CLI もこの API を使っているので、CLI でできることはすべて自作の
プログラムからもできます。

- ベース URL: `http://127.0.0.1:7920/api/v1`（設定の `listen` で変更できます）
- 特に記載がなければ、ボディは JSON です。成功した操作は `204 No Content` を返します。
- エラー時は `{"error": "..."}` という JSON を返します。

## 認証

| 設定 | 必要なもの |
|---|---|
| `token` なし（既定。ループバックのみ） | 認証情報は不要です。`Host` または `Origin` ヘッダーがループバックでないリクエストには `403` を返すため、Web ページがブラウザ経由で API を使うことはできません。 |
| `token = "..."` | `Authorization: Bearer <token>` を送るか、ヘッダーを付けられない場合（ブラウザの WebSocket など）は `?token=<token>` を付けます。なければ `401` です。 |

## ディスプレイ ID

ID は `d92-470B03781D1F`（ドライバ ID ＋ USB シリアル番号）のような形で、`GET /displays` で一覧できます。
`default` は、接続中のディスプレイのうち ID 順で最初のものを指します。

## エンドポイント

| メソッド | パス | ボディ | 動作 |
|---|---|---|---|
| `GET` | `/health` | | デーモンの状態とバージョン |
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
| `POST` | `/displays/{id}/web` | `{"url": "...", "reload": 600}` | Web ページを表示（後述） |
| `GET` | `/web/chrome` | | ヘッドレス Chrome の準備ができているか |
| `POST` | `/web/chrome` | | ヘッドレス Chrome をダウンロード |
| `PUT` | `/metrics/{id}` | JSON | ダッシュボードの `metric:<id>` パネルに出す値を設定（後述） |
| `GET` | `/metrics` | | すべてのメトリクス |
| `GET` | `/metrics/{id}` | | 1つのメトリクス |
| `DELETE` | `/metrics/{id}` | | メトリクスを削除 |

### `GET /health`

```json
{"status": "ok", "version": "0.1.1", "drivers": ["d92"]}
```

`drivers` は、デーモンが使うドライバの ID の一覧です。デーモンはこれらのドライバのディスプレイにしか触りません
（[コマンドラインガイド](cli.ja.md)の `--driver` を参照）。0.1.1 より前のデーモンは返しません。

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
      "submitted": 925, "shown": 908, "dropped": 17, "duplicates": 0,
      "last_encode_ms": 5.39, "last_send_ms": 21.85, "last_bytes": 50940
    }
  }
]
```

- `width` x `height` が描画すべきサイズです（横長）。ほかのサイズはパネルに合わせて調整されます。
- `content` は `nothing`・`image`・`animation`・`video`・`web`・`clock`・`dashboard`・`stream` のいずれかです。ディスプレイが抜かれている間も保持されます
  （そのとき `connected: false`、`stats: null`）。
- `stats`: `dropped` は送る前に新しいフレームに置き換えられた数、`duplicates` は変化がなかったため送らなかった数です。

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
`[video] ffmpeg`、`PATH`、`/opt/homebrew/bin`・`/usr/local/bin`・`/usr/bin`・`/snap/bin` の順に探します。

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
再起動すると消えるので、もう一度送ってください（送る側はたいてい定期実行なので、そのまま戻ります）。まだ値が届いていない
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

ダッシュボードに `claude-code` パネルを表示している間は、デーモン自身がメトリクス `claude-code` を更新し続けます（今の5時間
ブロックのトークン数、`detail` にブロックの終わる時刻と今日の合計、`history` に直近1時間の1分ごとのトークン数）。

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
