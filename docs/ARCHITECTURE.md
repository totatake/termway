# termway システムアーキテクチャ詳細設計書

本書は、`termway` の内部構造、モジュール設計、プロトコル仕様、およびマルチバックエンド機構について解説する技術設計ドキュメントです。

---

## 1. 全体構造とモジュール構成

`termway` は以下の疎結合な 5 つのコアレイヤーによって構成されています。

```
src/
├── main.rs                   # CLIパーサー、起動パラメータ初期化
├── engine.rs                 # メインイベントループ、フレーム同期制御
├── terminal/                 # 端末制御・保護層
│   ├── winsize.rs            # TIOCGWINSZ によるピクセル/セル解像度取得
│   └── raw.rs                # termios Rawモード化 & RAII Drop ガード
├── protocol/                 # エスケープシーケンス解析・生成層
│   ├── kitty.rs              # Kitty Graphics Protocol エンコーダ
│   ├── sgr_mouse.rs          # xterm SGR 1006 マウスプロトコルパーサー
│   └── keyboard.rs           # ANSIキー入力・特殊キーパーサー
├── display/                  # 仮想ディスプレイ管理・差分検出層
│   ├── xvfb.rs               # Xvfb ライフサイクル管理
│   ├── framebuffer.rs        # RGBA8888 フレームバッファ & XImage 変換
│   └── damage.rs             # タイルベース差分トラッカー
├── input/                    # 入力注入層
│   └── xtest.rs              # libXtst によるマウス/キー仮想イベント注入
└── backend/                  # マルチバックエンド抽象化層
    ├── mod.rs                # 共通 Backend トレイト & 環境自動判定
    ├── terminal.rs           # TerminalBackend (Kitty + SGR)
```

---

## 2. プロトコル仕様と変換アルゴリズム

### 2.1 Kitty Graphics Protocol エンコード (`src/protocol/kitty.rs`)
Kitty Graphics Protocol は、エスケープシーケンス `\x1b_G<control-data>;<payload>\x1b\\` を用いて、テキスト端末内に直接ピクセル画像を描画します。

#### コントロールキー定義
- `a=T`: 即時転送および表示 (Transmit and display)
- `f=32`: 32-bit RGBA フォーマット
- `s={width}, v={height}`: 送信画像矩形のピクセルサイズ
- `X={col}, Y={row}`: ターミナルウィンドウ内でのピクセルオフセット配置
- `m=1`: チャンク継続フラグ（次のエスケープシーケンスに後続データがあることを示す）
- `m=0`: チャンク終了フラグ（ペイロードの最終ブロック）

#### チャンク分割転送
ターミナルエミュレータのバッファオーバーフローを防止するため、エンコードされた Base64 ペイロードは 4096 文字ごとに分割して送信します：
1. **先頭チャンク**: `\x1b_Ga=T,f=32,s=100,v=100,X=10,Y=20,m=1;<Base64 chunk 1>\x1b\`
2. **中間チャンク**: `\x1b_Gm=1;<Base64 chunk 2>\x1b\`
3. **最終チャンク**: `\x1b_Gm=0;<Base64 chunk N>\x1b\`

---

### 2.2 xterm SGR 1006 マウスプロトコル解析 (`src/protocol/sgr_mouse.rs`)
ターミナルに `\x1b[?1000h\x1b[?1002h\x1b[?1006h` を送信することで、SGR 1006 拡張マウスモードを有効化します。

#### シーケンスフォーマット
- ボタン押下 / ドラッグ / 移動: `\x1b[<{btn};{col};{row}M`
- ボタン解放: `\x1b[<{btn};{col};{row}m`

#### ボタンビットフィールド解析
- 下位 2 ビット:
  - `0`: 左クリック
  - `1`: 中クリック
  - `2`: 右クリック
- ビット 5 (32): マウス移動フラグ (Drag または Hover)
- ビット 6 (64): ホイールイベント (64: WheelUp, 65: WheelDown)
- 修飾キー: Shift (+4), Meta (+8), Ctrl (+16)

#### セル座標から仮想ディスプレイへのマッピング式 (`src/terminal/winsize.rs`)
端末全体のピクセル解像度を $W_{term}, H_{term}$、文字セル数を $C, R$、仮想ディスプレイ解像度を $W_{x11}, H_{x11}$ とすると、受信したセル座標 $(col, row)$ は次のようにピクセルへ変換されます：

$$cw = \lfloor W_{term} / C \rfloor, \quad ch = \lfloor H_{term} / R \rfloor$$
$$px = (col - 1) \times cw + \lfloor cw / 2 \rfloor$$
$$py = (row - 1) \times ch + \lfloor ch / 2 \rfloor$$
$$x_{x11} = \mathrm{clamp}\left(\lfloor \frac{px \times W_{x11}}{W_{term}} \rfloor, 0, W_{x11} - 1\right)$$
$$y_{x11} = \mathrm{clamp}\left(\lfloor \frac{py \times H_{x11}}{H_{term}} \rfloor, 0, H_{x11} - 1\right)$$

---

## 3. 差分レンダリング (Tile Damage Tracking)

フルHD画面（1920x1080）を毎フレーム全体転送すると約 8MB/frame の Base64 テキストが必要となり、端末が描画飽和を起こします。そのため、`termway` はタイルベースの差分トラッカー（`src/display/damage.rs`）を内蔵しています。

1. **画面のグリッド分割**: 画面を 32x32 ピクセル（または 64x64）のタイルに分割。
2. **高速ハッシュ比較**: 各タイルの RGBA バイト列に対して FNV-1a 64-bit ハッシュを計算し、前フレームのハッシュ配列と比較。
3. **汚損領域の抽出**: ハッシュ値が異なるタイルのみを Dirty 矩形として抽出。
4. **Coalescing (矩形統合)**: 汚損タイルが多数（画面の50%以上）に及ぶ場合は、エスケープシーケンスのヘッダオーバーヘッドを避けるため、外接バウンディングボックス 1 枚の更新へ自動結合。

---

## 4. マルチバックエンド（二刀流）アーキテクチャ


```rust
pub trait Backend {
    fn name(&self) -> &'static str;
    fn render_damage(&mut self, fb: &Framebuffer, damage: &[DamageRect]) -> io::Result<()>;
    fn poll_input(&mut self, injector: &mut XTestInjector, target_w: u32, target_h: u32) -> io::Result<bool>;
    fn on_resize(&mut self, new_size: &TerminalSize);
}
```

### 4.1 起動環境の自動判定ロジック
1. **SSH環境**: `$SSH_CONNECTION`, `$SSH_CLIENT`, `$SSH_TTY` のいずれかが存在する場合 → **`TerminalBackend`**
2. **PTY環境**: 標準入力の tty 名が `/dev/pts/*` の場合（ローカル端末や tmux セッション） → **`TerminalBackend`**

- Linux カーネルの `/dev/dri/card0` を直接オープン。
- 物理コンソール上の直接描画により、X11/Wayland サーバーが一切常駐していないミニマル環境でも GUI を即座に表示可能。

---

## 5. 端末保護とライフサイクル安全設計

ターミナルを Raw モードおよびマウストラッキングモードに設定した状態でプログラムが不正終了（Panic, SIGINT, Segmentation Fault 等）すると、元のシェルがマウスクリック文字で埋め尽くされ操作不能になります。

これを防止するため、`termway` は **RAII (Resource Acquisition Is Initialization)** パターンを採用しています：

```rust
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        self.restore();
    }
}
```
- `RawModeGuard` がスコープを抜ける際（通常終了、return、`?` によるエラー伝播、パニック時のスタック巻き戻しのいずれでも）、必ず以下の端末復元シーケンスが標準出力へフラッシュされ、`tcsetattr` で元の termios が再設定されます：
  - `\x1b[?1006l\x1b[?1002l\x1b[?1000l`: マウストラッキング無効化
  - `\x1b[?25h`: カーソル再表示
  - `\x1b[?1049l`: Alternate Screen の終了（元のシェル画面に戻る）
