# コマンドラインガイド

[English](cli.md)

操作はすべて `ssp` という1つのコマンドで行います。`ssp serve` がディスプレイを管理するデーモンを動かし、
それ以外のコマンドはすべて、そのデーモンに [HTTP API](api.ja.md) で処理を依頼します。

- [ユースケース](#ユースケース): よくある使い方のレシピ
- [コマンドリファレンス](#コマンドリファレンス): すべてのコマンドとオプション
- [トラブルシューティング](#トラブルシューティング)

## ユースケース

### ログインしている間ずっと時計を表示する

```sh
ssp service install
```

これでログイン時にデーモンが起動し、見つけたすべてのディスプレイに時計を表示します。時計の見た目をずっと変えておきたい
場合は、設定ファイルを作って `[clock]` セクションを編集します。

```sh
ssp config init        # コメント付きの設定ファイルを作成し、そのパスを表示
```

```toml
[clock]
seconds = false
date_format = "%a %-d %b"     # 例: "Wed 7 Oct"。"" にすると日付を表示しません
color = "#FFD080"
```

編集したら、デーモンが設定を読み直すように[再起動](#自動起動)してください。
なお、内蔵フォントは欧文の文字しか持っていないため、日本語などは表示できません。

その場だけ変えたい場合は、オプションで指定します。

```sh
ssp clock --no-seconds --date-format ""
```

### PC の電源を切っても画像を表示しておく

```sh
ssp show wallpaper.png --fit cover --persist
```

`--persist` を付けると画像がディスプレイ側にも保存され、PC をシャットダウンしたりディスプレイを挿し直したりしても
表示されます。付けない場合、画像が表示されるのはデーモンが動いている間だけです。

保存はディスプレイのフラッシュメモリへの書き込みで、少し時間がかかります（D92 では約 1.5 秒）。しばらく表示しておく
画像に使い、アニメーションや毎分変わるようなものには使わないでください。

デーモンの終了時に、最後に表示していた絵を自動で保存させたい場合は、設定ファイルに次のように書きます。

```toml
[display]
on_exit = "save-last"
```

### 表示を止める・消す・画面を消灯する

| コマンド | 画面の絵 | 画面 | 時計・画像・ストリーム |
|---|---|---|---|
| `ssp stop` | そのまま残る | 点灯 | 停止 |
| `ssp clear` | 黒になる | 点灯 | 停止 |
| `ssp off` | 保持される | 消灯（バックライト off） | 動き続ける |
| `ssp on` | | 再び点灯 | |

再び何かを表示するには `ssp clock` や `ssp show ...` を実行します。

### 夜は画面を暗くする

`ssp brightness` を定期実行します。cron の場合（macOS・Linux、`crontab -e`）:

```text
0 22 * * * /usr/local/bin/ssp brightness 20
0 7  * * * /usr/local/bin/ssp brightness 100
```

Windows の場合:

```bat
schtasks /Create /SC DAILY /ST 22:00 /TN "ssp dim" /TR "C:\Tools\ssp.exe brightness 20"
schtasks /Create /SC DAILY /ST 07:00 /TN "ssp bright" /TR "C:\Tools\ssp.exe brightness 100"
```

定期実行のジョブは `PATH` が最小限なので、`ssp` はフルパスで書いてください。ディスプレイが接続されるたびに明るさを
設定したい場合は、設定ファイルの `[display]` セクションに `brightness = 80` のように書きます。

### 複数のディスプレイを使う

```sh
ssp devices
```

```text
ID                MODEL       SIZE      CONTENT  STATE
d92-470B03781D1F  upHere D92  1920x462  clock    connected
d92-5C2A11FE0A42  upHere D92  1920x462  clock    connected
```

`-d` / `--display` でディスプレイを選びます。

```sh
ssp -d d92-470B03781D1F clock
ssp -d d92-5C2A11FE0A42 show photo.jpg
```

`--display` を省略すると `default`、つまり接続中のディスプレイのうち ID 順で最初のものが対象になります。
各ディスプレイは表示内容を覚えていて、抜いて挿し直しても同じ表示を続けます。

### 別の PC から操作する

既定では、デーモンは同じ PC からの接続しか受け付けません。ほかの PC からも操作するには、ディスプレイをつないだ PC の
設定ファイルで、待ち受けアドレスとトークンを設定します。

```toml
listen = "0.0.0.0:7920"
token = "a-long-random-string-of-at-least-16-characters"   # 16 文字以上の長いランダムな英数字
```

操作する側の PC では次のようにします。

```sh
export SSP_URL=http://192.168.1.10:7920
export SSP_TOKEN=a-long-random-string-of-at-least-16-characters
ssp status
ssp show photo.jpg
```

通信は暗号化されていない HTTP なので、トークンや画像は暗号化されません。信頼できるネットワークでだけ使ってください。

### 自作のコンテンツを表示する（ダッシュボード、ゲームなど）

どんなプログラムからでも、WebSocket でデーモンにフレームを送れます（最大 60fps）。[README の例](../README.ja.md#自作プログラムから描画する)
と [API リファレンス](api.ja.md#websocket-ストリーム)を参照してください。ストリーム中は `ssp devices` の CONTENT が
`stream` になります。`ssp clock` や `ssp show` を実行すると、ディスプレイの制御を取り戻せます。

### 1台を使いながら、別のディスプレイへの対応を開発する

たとえば D92 で時計を表示したまま、別のディスプレイ用のドライバ `dxxxx` を書いているとします。デーモンを使うドライバだけで
起動すれば、横で新しいディスプレイを検査できます。

```sh
ssp serve --driver d92            # デーモンはほかのディスプレイに触らない
ssp selftest --driver dxxxx       # 別のターミナルで、何度でも
ssp devices --driver dxxxx        # 新しいディスプレイが認識されているか
```

開発中のドライバには「試験中（experimental）」の印を付けられます（[機種の追加方法](adding-a-device.ja.md)を参照）。
`--driver` や `[drivers] enable` で名前を指定したときだけ使われるので、うっかりディスプレイを取ってしまうことはありません。

### デーモンを手動で動かす

```sh
ssp serve                                # ログをターミナルに出力。Ctrl-C で終了
SSP_LOG=debug ssp serve                  # より詳しいログ
ssp serve --listen 127.0.0.1:8000        # 別のポート（ほかのコマンドでは --url を指定）
ssp serve --log-file ~/ssp.log           # ログをファイルに出力
```

## コマンドリファレンス

### 共通オプション

すべてのコマンドで使えます。

| オプション | 環境変数 | 既定値 | 意味 |
|---|---|---|---|
| `--config <FILE>` | `SSP_CONFIG` | [後述](#設定ファイルの場所) | 使用する設定ファイル |
| `--url <URL>` | `SSP_URL` | 設定ファイルの `listen` から（`http://127.0.0.1:7920`） | デーモンの場所 |
| `--token <TOKEN>` | `SSP_TOKEN` | 設定ファイルの `token` | デーモンが要求する場合の API トークン |
| `-d`, `--display <ID>` | | `default` | 操作するディスプレイ（`ssp devices` で確認） |
| `-h`, `--help` | | | コマンドのヘルプ。例: `ssp show --help` |
| `-V`, `--version` | | | バージョンを表示 |

`SSP_LOG` は `ssp serve` のログレベルを設定します（既定は `info`。例: `debug`、`ssp_server=debug`）。

### デーモン

| コマンド | 説明 |
|---|---|
| `ssp serve` | デーモンをフォアグラウンドで起動します。Ctrl-C（または SIGTERM）で止まり、そのとき設定の `on_exit` を実行します。 |
| `  --listen <ADDR>` | 待ち受けアドレス（例: `127.0.0.1:8000`）。設定ファイルの `listen` より優先されます。 |
| `  --log-file <FILE>` | ログを表示する代わりにファイルへ追記します。 |
| `  --driver <ID>` | このドライバだけを使います（複数指定可）。例: `--driver d92`。設定ファイルの `[drivers]` の `enable` より優先されます。ほかのドライバのディスプレイには触りません。 |

### ディスプレイ

| コマンド | 説明 |
|---|---|
| `ssp devices` | ディスプレイの一覧（ID、機種、サイズ、表示内容、接続状態）。デーモンが動いていない場合は、代わりにこの PC に挿さっている対応ディスプレイを表示します。 |
| `  --json` | 詳細を JSON で出力します（`GET /api/v1/displays` と同じ内容）。 |
| `  --driver <ID>` | このドライバのディスプレイだけを表示します（複数指定可）。 |
| `ssp status` | デーモンのバージョンと、ディスプレイごとのファームウェアとフレームの統計: 表示した数、間引いた数（新しいフレームに置き換えられたもの）、変化がなく省略した数、受け取った数、直近のエンコード時間・送信時間・サイズ。 |
| `ssp selftest` | デーモンを使わずに、接続中のディスプレイを直接検査します。1台あたり約 25 秒かかり、項目ごとに PASS / WARN / FAIL を表示します: 接続（とファームウェア）、コマンド（点灯、明るさ 100%）、静止画、連続送信（fps）、キープアライブ（しばらく何もしなくても接続が保たれるか）、電源（消灯と点灯）。複数台あるときは、全台を先に開いてから1台ずつ検査します。デーモンが動いている場合、デーモンが使うドライバのディスプレイは飛ばします（デーモンがいつ取りにくるか分からないため）。検査するには、デーモンを止めるか、ほかのドライバだけで起動し直してください（`--driver`）。失敗した項目がある、または1台も検査できなかった場合は終了コード 1 で終わります。`--display` で1台だけ検査できます。 |
| `  --driver <ID>` | このドライバのディスプレイだけを検査します（複数指定可）。試験中のドライバも使えるようになります。 |
| `  --frames <N>` | 連続送信の検査で送るフレーム数（既定 180）。 |
| `  --hold <SECONDS>` | キープアライブの検査で何もしない時間（秒、既定 15）。 |
| `  --json` | 結果を JSON で出力します。 |

### 表示する内容

| コマンド | 説明 |
|---|---|
| `ssp show <FILE>` | PNG・JPEG・GIF・WebP の画像を表示します。 |
| `  --fit contain` | （既定）画像全体を収め、余白は黒にします。 |
| `  --fit cover` | 画面全体を埋め、はみ出した部分は切り取ります。 |
| `  --fit stretch` | 画面全体を埋めます。必要なら画像を引き伸ばします。 |
| `  --persist` | ディスプレイにも画像を保存し、電源を切っても残るようにします（フラッシュに書き込みます）。 |
| `ssp clock` | 組み込みの時計を表示します。指定しなかった項目は設定ファイルの `[clock]` に従います。 |
| `  --no-seconds` | 秒を表示しません。 |
| `  --format <FMT>` | 時刻の書式。例: `"%H:%M"`、`"%I:%M %p"`（[strftime 形式](https://docs.rs/jiff/latest/jiff/fmt/strtime/)）。 |
| `  --date-format <FMT>` | 日付行の書式。例: `"%Y-%m-%d %a"`。`""` で非表示。 |
| `ssp stop` | 時計・画像・ストリームを止めます。画面には最後の絵が残ります。 |
| `ssp clear` | 時計・画像・ストリームを止め、画面を消去します。 |

### 画面

| コマンド | 説明 |
|---|---|
| `ssp brightness <0-100>` | バックライトの明るさをパーセントで設定します。 |
| `ssp on` | 画面を点灯します。 |
| `ssp off` | 画面を消灯します。表示中の内容は裏で動き続けます。 |

### 自動起動

| コマンド | macOS | Linux | Windows |
|---|---|---|---|
| `ssp service install` | LaunchAgent `~/Library/LaunchAgents/dev.sub-screen-player.ssp.plist` | systemd ユーザーユニット `~/.config/systemd/user/sub-screen-player.service` | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` の値 `sub-screen-player` |
| `ssp service uninstall` | デーモンを止めて LaunchAgent を削除 | デーモンを止めてユニットを削除 | 登録を削除（動いているデーモンはサインアウトまで動き続けます） |
| `ssp service status` | 登録の有無と動作状態 | 登録の有無と動作状態 | 登録の有無 |
| 再起動（設定を変えたときなど） | もう一度 `ssp service install` | `systemctl --user restart sub-screen-player` | タスクマネージャーで `ssp.exe` を終了してから `ssp service install` |
| ログ | `~/Library/Logs/sub-screen-player.log` | `journalctl --user -u sub-screen-player` | `%LOCALAPPDATA%\sub-screen-player\ssp.log` |

`install` はすぐにデーモンを起動します。登録には、インストールした時点の `ssp` 実行ファイルと設定ファイルのパスが
記録されるので、どちらかを移動したら `install` をやり直してください。

### 設定ファイル

| コマンド | 説明 |
|---|---|
| `ssp config path` | 設定ファイルのパスを表示します。 |
| `ssp config init` | 既定値を書いたコメント付きの設定ファイルを作成します。 |
| `  --force` | 既存のファイルを上書きします。 |
| `ssp config show` | 実際に使われる設定（既定値＋ファイルの内容）を表示します。 |

#### 設定ファイルの場所

| OS | パス |
|---|---|
| macOS | `~/Library/Application Support/sub-screen-player/config.toml` |
| Linux | `~/.config/sub-screen-player/config.toml`（`$XDG_CONFIG_HOME` が設定されていればそれに従います） |
| Windows | `%APPDATA%\sub-screen-player\config\config.toml` |

設定ファイルはなくても構いません。その場合は既定値で動きます。`ssp config init` で作ったファイルには、すべての設定項目が
コメント付きで書かれています。デーモンは起動時に設定ファイルを読み込みます。

### 終了コード

| コード | 意味 |
|---|---|
| `0` | 成功 |
| `1` | 失敗（デーモンが動いていない、リクエストを拒否されたなど）。理由は `error:` に続けて表示されます。 |
| `2` | コマンドライン引数の誤り |

## トラブルシューティング

1. **`ssp selftest` を実行する**: デーモンを使わずに、USB から画面表示までをひととおり検査します。デーモンを止めてから実行し、
   [問題を報告する](../CONTRIBUTING.ja.md)ときは出力を添えてください。
2. **`ssp devices` を実行する**
   - 「The daemon is not running」と表示され、ディスプレイが「Plugged in」と出ている: `ssp serve` か
     `ssp service install` でデーモンを起動してください。
   - ディスプレイがまったく表示されない: ケーブルを確認してください。Linux では udev ルールを入れて
     （[README](../README.ja.md#インストール) を参照）、ディスプレイを挿し直してください。
3. **デーモンのログに「cannot open ... display」と出る**: メーカー製アプリや2つ目の `ssp serve` など、ほかのプログラムが
   ディスプレイを使っています。それを終了してください。デーモンは 10 秒ごとに再試行します。
4. **「cannot listen on 127.0.0.1:7920 (is the daemon already running?)」と出る**: すでにデーモンが動いている
   （`ssp service status` で確認できます）か、ほかのプログラムがそのポートを使っています。`--listen` で別のポートを指定し、
   ほかのコマンドでは `--url` を指定してください。
5. **`ssp status`** で、フレームがディスプレイに届いているかを確認できます。`shown` が増えていれば届いています。
   プログラムがディスプレイの処理能力より速くフレームを送っている場合、`dropped` が多くなるのは正常です。
6. **詳しく調べたい**: `SSP_LOG=debug ssp serve` で起動してログを確認してください。
7. **デーモンを止めると D92 に古い絵が表示される**: D92 は、デーモンからの通信が途絶えて約 8 秒後に再起動し、最後に
   *保存された*画像を表示します。表示しておきたい画像を `ssp show --persist` で保存するか、`on_exit = "save-last"` を設定してください。
8. **ディスプレイが何にも反応しない**: いったん抜いて挿し直してください。デーモンが自動的に認識し直します。
