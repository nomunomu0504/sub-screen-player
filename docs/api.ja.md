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
- `content` は `nothing`・`image`・`animation`・`clock`・`dashboard`・`stream` のいずれかです。ディスプレイが抜かれている間も保持されます
  （そのとき `connected: false`、`stats: null`）。
- `stats`: `dropped` は送る前に新しいフレームに置き換えられた数、`duplicates` は変化がなかったため送らなかった数です。

### `POST /displays/{id}/image`

ボディは PNG・JPEG・GIF・WebP の画像ファイルです（64 MiB まで）。アニメーション GIF・APNG・WebP は、コマごとの間隔で
繰り返し再生します（このときディスプレイの `content` は `animation` になります）。クエリパラメータ:

| パラメータ | 値 | 既定値 |
|---|---|---|
| `fit` | `contain`（全体を収めて余白は黒）、`cover`（はみ出しを切り取り）、`stretch`（引き伸ばし） | `contain` |
| `persist` | `true` でデバイスにも保存（電源を切っても残る。フラッシュに書き込みます）。アニメーションは最初のコマ | `false` |

```sh
curl --data-binary @photo.png "http://127.0.0.1:7920/api/v1/displays/default/image?fit=cover"
```

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

`widgets` は左から並べるパネルです（`clock`・`cpu`・`memory`・`network`・`disk` から1〜6個）。`accent` はグラフの色です。

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
