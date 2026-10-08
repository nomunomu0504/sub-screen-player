# 機種の追加方法

[English](adding-a-device.md)

ドライバは、sub-screen-player に1つの機種ファミリーの扱い方を教えるものです。それ以外（デバイスの検出、
フレームの送信ペース、エンコード、キープアライブ、API、CLI）はすべて共通なので、ドライバは多くの場合
数百行で済みます。お手本は `crates/drivers/d92` です。

## 1. デバイスの通信方法を調べる

次のことを調べ、`docs/devices/<model>.md`（と日本語版の `docs/devices/<model>.ja.md`）に書き残してください。

- **USB ID**（ベンダー:プロダクト）とインターフェースの種類。macOS は `ioreg -p IOUSB -l`、Linux は `lsusb -v`、
  Windows はデバイスマネージャー →「詳細」→「ハードウェア ID」で確認できます。
- **パネルのサイズと向き**: 呼び出し側が描くサイズ（横長）と、送る前に画像を回転する必要があるか。
- **画像形式**: JPEG、生の RGB565 など。サイズの上限も。
- **コマンド**: フレームを表示する方法。あれば明るさ、電源、消去、キープアライブも。
- **タイミング**: どこまで速くフレームを送れるか、キープアライブが必要か。

ドキュメントがない場合は、メーカー製アプリの通信をキャプチャします。x86 の Windows なら Wireshark と USBPcap が
使えます。それができない環境（Apple Silicon など）では、VMware の仮想マシンでメーカー製アプリを動かし、
`usb.analyzer.enable = "TRUE"` を設定すると、USB の通信が `vmware.log` に記録されます。
[通信を調べるときの作法](../CONTRIBUTING.ja.md#通信を調べるときの作法)も読んでおいてください。

現在 `ssp-core` が対応しているのは **HID** デバイスです。USB バルク転送やシリアルポートを使うディスプレイの場合は、
先に Issue を立ててください。`crates/core` に新しい `Transport` が必要になり、それはほかのドライバとも共有されます。

## 2. クレートを作る

```text
crates/drivers/<model>/
├── Cargo.toml
└── src/
    ├── lib.rs        Driver と Display の実装
    └── protocol.rs   レポートを組み立てる純粋関数
```

`Cargo.toml`:

```toml
[package]
name = "ssp-driver-<model>"
description = "sub-screen-player driver for the <Vendor Model>"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true

[dependencies]
ssp-core.workspace = true
tracing.workspace = true

[lints]
workspace = true
```

## 3. プロトコルを書く

`protocol.rs` には定数（USB ID、パネルサイズ、レポート長）と、コマンドや画像をバイト列に変換する関数を置きます。
I/O を含めないことで、網羅的にテストできるようにします。

テストは正しいと分かっているバイト列と照合します。たとえば D92 ドライバは、14547 バイトの JPEG に付くヘッダーが
1バイトも違わないことを確認しています。

```rust
#[test]
fn live_frame_header_is_exact() {
    let out = live_frame(&vec![0xAB; 14547]);
    assert_eq!(&out[..13], &[0x43, 0x52, 0x54, 0, 0, 0x44, 0x52, 0x41, 0, 0, 0x38, 0xf3, 0xb1]);
}
```

デバイスを壊れた状態にするコマンドが分かっている場合は、ドライバがそれを決して送らないことを確かめるテストも書いてください。

## 4. `Display` を実装する

```rust
pub struct MyDisplay {
    transport: Box<dyn Transport>,
    info: DisplayInfo,
}

impl MyDisplay {
    /// HID のハンドルではなく `Transport` を受け取るので、テストでは `RecordingTransport` を渡せます。
    pub fn open(transport: Box<dyn Transport>, serial: &str) -> Result<Self> { ... }
}

impl Display for MyDisplay {
    fn info(&self) -> &DisplayInfo { &self.info }
    fn show(&mut self, image: &EncodedImage) -> Result<()> { ... }
    // デバイスが対応しているものだけを実装し、残りは既定の実装のままにします。
}
```

`DisplayInfo` は実態どおりに埋めてください。

- `driver`: 短く、変わらない ID（`"d92"` など）。ディスプレイ ID やログに使われます。
- `panel`: 横長のサイズ、送信時に必要な `Rotation`、`ImageFormat`。
- `capabilities`: `max_fps` は実測した値に、キープアライブが必要なら `keep_alive_interval` を、
  `max_image_bytes` はデバイスが受け付けるサイズにします。

デバイスがなくなったときは `Error::Transport` か `Error::Disconnected` を返してください。デーモンが自動的に開き直します。
ドライバの中でリトライしたり、ループでスリープしたりしないでください。

## 5. `Driver` を実装する

```rust
pub struct MyDriver;

impl Driver for MyDriver {
    fn id(&self) -> &'static str { "mymodel" }
    fn name(&self) -> &'static str { "Vendor Model" }
    fn usb_matches(&self) -> &'static [UsbMatch] {
        &[UsbMatch { vendor_id: 0x1234, product_id: 0x5678, usage_page: Some(0xFF00) }]
    }
    fn open(&self, candidate: &Candidate) -> Result<Box<dyn Display>> {
        let transport = ssp_core::hid::open(candidate)?;
        Ok(Box::new(MyDisplay::open(Box::new(transport), &candidate.serial)?))
    }
}
```

マッチ条件はできるだけ狭くしてください（デバイスに HID インターフェースが複数あるなら usage page も指定します）。
そうすれば、ドライバが誰かのキーボードを掴んでしまうことはありません。

開発中は、ドライバに「試験中（experimental）」の印を付けてください。名前を指定したとき（`--driver mymodel`、
または設定ファイルの `[drivers]` の `enable`）だけ使われるようになるので、作りかけのドライバがリリースに入ってしまっても、
勝手にディスプレイを取ることはありません。

```rust
    fn experimental(&self) -> bool {
        true // 実機で確認できたら外す
    }
```

## 6. 登録する

1. ルートの `Cargo.toml`: `members` と `[workspace.dependencies]` にクレートを追加します。
2. `crates/server/Cargo.toml`: `ssp-driver-<model>.workspace = true` を追加します。
3. `crates/server/src/drivers.rs`: `registry.register(ssp_driver_<model>::MyDriver);` を追加します。
4. `contrib/linux/70-sub-screen-player.rules`: ベンダー ID とプロダクト ID の行を追加します。
5. `README.md` と `README.ja.md`: 「対応ディスプレイ」の表に追加します。
6. `docs/devices/<model>.md` と `docs/devices/<model>.ja.md`: 手順 1 で調べたプロトコルのメモを置きます。

## 7. 実機で試す

`mise run ci` が通ることが前提です。そのうえで実機で自動検査を実行し、出力をプルリクエストに貼ってください。

```sh
ssp selftest --driver mymodel  # すべて PASS になるはず
```

デーモンが動いていても、検査の妨げになるのはデーモンがそのドライバを使っている場合だけです。ほかのディスプレイを
表示したまま検査したいときは、そのディスプレイのドライバだけでデーモンを起動してください（例: `ssp serve --driver d92`）。

続けて手でも試し、画面を確認してください。

```sh
mise run serve                 # 時計が表示されるはず
ssp devices                    # ディスプレイが一覧に出る
ssp brightness 30 && ssp brightness 100
ssp off && ssp on
ssp show some-photo.jpg --fit cover   # 向きを確認
ssp status                     # 表示数・間引き数と送信時間を確認
```

フレームレートを測るには、WebSocket API（[api.ja.md](api.ja.md)）でフレームを流しながら `ssp status` を見てください。
時計の表示中にディスプレイを抜き差しして、自動で復帰することも確認してください。
