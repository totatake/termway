# 利用ガイド (`termway`)

`termway` の対応端末、基本的な使い方、CLI オプション・`TERMWAY_*` 環境変数リファレンス、ショートカット、Perfetto トレース手順、およびトラブルシューティングをまとめます。

## 1. 対応端末とプロトコル自動判定

- **Kitty Graphics Protocol 対応端末**: Kitty, Ghostty, WezTerm, Noctty など（`--kitty` で強制可）
- **Sixel 対応端末**: Windows Terminal, iTerm2, Foot, mlterm, RLogin, VS Code 統合ターミナルなど（`--sixel` で強制可）
- 端末の診断レポートを確認する場合:
  ```bash
  termway --query
  ```

---

## 2. 基本操作と代表例

```bash
# 内蔵 GTK3 デモアプリ起動
termway --demo

# 任意 GUI アプリを起動 (Wayland / X11 / Snap 対応)
termway -- firefox
termway -- gvim
termway -- gparted

# Sixel 指定や HiDPI スケール・セルサイズ指定
termway --sixel --scale 1.5 -- firefox
termway --cell-size 10x22 -- gvim
```

### ショートカットキー・連携機能

| 操作 | 機能 | 説明 |
| :--- | :--- | :--- |
| `Ctrl-Q` | 安全終了 | アプリケーションを終了し、端末 Raw モードと画面状態を復元 |
| `F12` | スクリーンショット | 現在の GUI フレームを PNG 保存（`TERMWAY_SCREENSHOT_DIR` で保存先変更可） |
| `Ctrl-C` / 選択 | クリップボード送信 | OSC 52 経由でホスト OS クリップボードへコピー（`--no-osc52` で無効化可） |
| `Ctrl-V` / 貼付 | クリップボード貼付 | Bracketed Paste 経由でマルチバイト文字を含めて入力 |
| マウス中クリック | Primary Selection | Wayland Primary Selection のテキストを即時貼り付け |

---

## 3. CLI オプション・環境変数リファレンス

| オプション | 主な対応環境変数 | 説明 | デフォルト |
| :--- | :--- | :--- | :--- |
| `--kitty` / `--sixel` | `TERMWAY_FORCE_PROTOCOL` | 描画プロトコルを明示指定 | 自動検出 |
| `--jpeg` / `--no-jpeg` | `TERMWAY_KITTY_JPEG` | Kitty `f=101` JPEG 拡張の有効/無効 | 自動プローブ |
| `--jpeg-quality <1-100>` | `TERMWAY_JPEG_QUALITY` | JPEG 圧縮品質 | `75` |
| `--pixel-mouse` / `--no-pixel-mouse` | — | SGR-Pixels (`1016h`) 1px マウス追従制御 | 自動検出 |
| `--no-host-keyring` | `TERMWAY_NO_HOST_KEYRING=1` | ホスト Secret Service キーリング中継を無効化 | 有効 |
| `--no-osc52` | `TERMWAY_NO_OSC52=1` | OSC 52 クリップボード同期を無効化 | 有効 |
| `--no-polkit` | `TERMWAY_NO_POLKIT=1` | Polkit 認証エージェント自動起動を無効化 | 有効 |
| `--cell-size <WxH>` | `TERMWAY_CELL_SIZE` | セルピクセル寸法（例: `10x22`） | SGR 14t 自動測定 |
| `--width <PX>` / `--height <PX>` | — | 仮想ディスプレイ解像度（最大 `7680x4320`） | 端末から算出 |
| `--fps <FPS>` | `TERMWAY_FPS` | フレームレート上限 | `30` |
| `--scale <FACTOR>` | — | Wayland 出力スケール（`0.25`〜`4.0`） | `1.0` |
| `--hud` | — | FPS・エンコード・帯域・`Ovf` オーバーレイ表示 | 無効 |
| `--profile` | `TERMWAY_PROFILE=1`, `TERMWAY_PROFILE_FILE` | パイプライン統計サマリー / JSON 出力 | 無効 |
| `--trace` | `TERMWAY_TRACE=1` | Perfetto 互換タイムライントレース有効化 | 無効 |
| `--trace-file <PATH>` | `TERMWAY_TRACE_FILE` | 出力先（`.gz` 対応） | `/tmp/termway_trace.json` |
| `--trace-buffer <N>` | `TERMWAY_TRACE_BUFFER` | スレッド毎リング容量（例: `64k`） | `65536` |
| `--trace-flush-secs <N>` | `TERMWAY_TRACE_FLUSH_SECS` | 連番スナップショット出力間隔（秒） | `0` (無効) |
| `--trace-no-input` | `TERMWAY_TRACE_NO_INPUT=1` | 入力イベント記録を抑制 | 無効 |
| `--no-splash` | — | 起動スプラッシュ非表示 | 無効 |
| `—` | `TERMWAY_NO_CURSOR_SHAPE=1` | OSC 22 カーソル形状同期を無効化 | 有効 |
| `—` | `TERMWAY_JPEG_SIMD=0` | SIMD JPEG を無効化しスカラー実装へ切替 | SIMD 有効 |
| `—` | `TERMWAY_PIPELINE=serial` | Double-Buffering を無効化し直列実行へ切替 | 並列有効 |

---

## 4. プロファイル・Perfetto トレース手順

詳細な仕様は [../design/trace/03-cli-and-env.md](../design/trace/03-cli-and-env.md) も参照してください。

```bash
TERMWAY_PROFILE_FILE=/tmp/profile.json \
TERMWAY_TRACE_FILE=/tmp/termway_trace.json.gz \
  ./target/release/termway --profile --trace -- firefox
```

出力された `/tmp/termway_trace.json.gz` は [ui.perfetto.dev](https://ui.perfetto.dev) にドラッグ＆ドロップして `frame_id` フローや `TIOCOUTQ`、`win_{id}_state` を確認できます。実行中に `kill -USR1 $(pgrep termway)` で時点スナップショットも保存可能です。

---

## 5. トラブルシューティング (FAQ)

### WSL2 (Ubuntu) で `/tmp/.X11-unix` 書込エラーが出て X11 アプリが起動しない
WSLg が `/tmp/.X11-unix` を読み取り専用（`ro`）でマウントしている場合、以下のコマンドで読み書き可能（`rw`）に再マウントしてください：

```bash
sudo mount -o remount,rw /tmp/.X11-unix
```

WSL2 起動時に自動適用するには `/etc/wsl.conf` に `[boot] command = "mount -o remount,rw /tmp/.X11-unix"` を追加します。
