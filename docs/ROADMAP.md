# Future Engineering & Architecture Roadmap

本ドキュメントは、`termway` の長期的な保守性・スケーラビリティ・堅牢性を向上させるためのアーキテクチャおよびエンジニアリング上の技術的課題（Technical Debt）と今後の改善ロードマップをまとめたものです。

---

## 1. `TermwayState` のドメイン分割（v0.6.0 実装済み）

### 背景と課題
`src/compositor/state.rs` 内の `TermwayState` 構造体は、Smithay コンポジタの要求によりイベントループの中心に配置されていましたが、初期実装では以下の多岐にわたる責務を 1 ファイル（約 1,000 行）で一括保持していました：
- **Wayland プロトコルコア**: CompositorState, XdgShellState, ShmState, XWaylandShellState
- **入力機器・Seat 管理**: SeatState, Seat, KeyboardHandle, PointerHandle
- **ウィンドウライフサイクル・管理**: Wayland サーフェス, X11 サーフェス, ManagedWindow, ポップアップ管理
- **インタラクション状態マシン**: ウィンドウの移動・リサイズ（`WindowDragState`）、最大化/最小化、タスクバー状態
- **描画・ループ制御**: `dirty` フラグ, `running` フラグ, 各種タイマー

### 実装された構造（v0.6.0）
Smithay の `delegate_*!` マクロおよび `KeyboardHandle::set_focus(&mut TermwayState)` の型制約と調和しつつ、以下のように責務を分割しました：
- **[`WindowManager`](../src/compositor/window_manager.rs) (`state.wm`)**:
  - ウィンドウ（Wayland / X11）の Z-order、アクティブ状態、ドラッグ・リサイズ状態（`WindowDragState`）、最小化/最大化、カスケード配置オフセットを Smithay の `DisplayHandle` に依存しない純粋ドメイン構造体として分離・単体テスト化。
- **プロトコルハンドラのドメイン別モジュール分割 ([`src/compositor/handlers/`](../src/compositor/handlers/mod.rs))**:
  - `handlers/compositor.rs`: `CompositorHandler`, `BufferHandler`, `ShmHandler`, `OutputHandler`
  - `handlers/xdg_shell.rs`: `XdgShellHandler`, `XdgDecorationHandler`
  - `handlers/seat.rs`: `SeatHandler`, `SelectionHandler`, `DataDeviceHandler`（OSC 52 クリップボード連携）

---

## 2. パフォーマンス最適化アーキテクチャ（v0.4.0〜v0.7.0 実装済み）

### 2.1 差分更新とダーティバンドキャッシュ（Damage Tracking & Dirty Band Caching）

#### 初期の課題
初期実装では、画面の一部（xeyes の瞳移動、エディタのカーソル点滅、時計の更新など）が変化しただけでも毎フレーム全画面（数MB）を再エンコードして送信しており、高解像度端末や SSH 接続で帯域逼迫と遅延を招いていました。

#### 実装された構造（v0.4.0〜v0.7.0）
- **32x32 タイル AVX2 差分検出 ([`DamageTracker`](../src/display/damage.rs))**: 画面を 32x32 タイルに分割し、AVX2 8 レーン並列 FNV-1a ハッシュ比較により 1080x720 画面を約 0.14〜0.20ms で走査し、無変化フレームの再エンコードを完全スキップ。
- **Sixel 6 行ダーティバンドキャッシュ ([`SixelStreamEncoder`](../src/protocol/sixel.rs))**: 6 ピクセル高の水平バンドごとに FNV-1a ハッシュをキャッシュし、変化したバンドのみを RGB332 減色・RLE 圧縮して再構築。カーソル移動や部分更新時の Sixel エンコード時間を **4.3ms → 0.34ms（92〜95% 削減）** に高速化。

### 2.2 非同期エンコードワーカーと統一 FD ポーリング（Async Pipeline & Unified Polling）

#### 初期の課題
Pixman レンダリングと画像エンコード、標準出力フラッシュがイベントループ内で同期実行されていたため、重い画面更新時に入力応答がブロックされるほか、アイドル時にも短いタイマーポーリングで CPU を消費していました。

#### 実装された構造（v0.4.0〜v0.7.0）
- **非同期ワーカースレッド分離 ([`TerminalBackend`](../src/backend/terminal.rs))**: エンコードと stdout フラッシュを専用スレッド（`termway-async-render`）へ分離。ワーカー処理中に新しいフレームが到着した場合は `pending_job` を最新フレームで上書き（Frame Dropping）し、PTY のバッファブロートを防止。
- **統一 FD ポーリング & フレームペーシング ([`TermwayCompositor`](../src/compositor/mod.rs))**: `stdin`・Wayland `ListeningSocket`・`display.backend().poll_fd()` を単一の `nix::poll` で一括監視し、アイドル時の CPU 使用率を 0% に削減。さらに `--fps`（デフォルト 30fps）に基づくフレームペーシングにより、連続コミット時の過剰な再描画を抑制。

### 2.3 ゼロアロケーション・バッファ再利用と SIMD ベクトル化

#### 初期の課題
毎フレーム数MBの `Vec<u8>` や `deflate` 状態が動的に確保・破棄され、アロケータのロック競合やキャッシュ汚染によるフレーム時間のジッターが発生していました。

#### 実装された構造（v0.5.0〜v0.7.0）
- **ゼロアロケーション・ストリーミング ([`KittyStreamEncoder`](../src/protocol/kitty.rs))**: `CompressorOxide` インスタンスの `reset()` 再利用と、Base64 エンコード結果の 4096B チャンク直接出力により、フレームごとの一時ヒープ確保を完全にゼロ化。また `TerminalBackend` の `spare_buffer` 往復により RGBA フレームバッファも完全再利用。
- **AVX2 SIMD ベクトル高速化**: `x86_64` 環境で AVX2 命令を実行時検出し、Sixel の RGB332 減色・`tzcnt` ビットマスク走査、Damage タイルハッシュ、BGRX→RGBA 変換をベクトル高速化。
- **Pixman レンダラ パス統合 ([`TermwayRenderer`](../src/compositor/renderer.rs))**: 可視ウィンドウとポップアップの逆順描画パスを整理・統合し、1080x720 フルフレーム合成を約 0.67ms に短縮。

---

## 3. レガシー X11 エンジンと Wayland コンポジタの一本化（v0.6.0 実装済み）

### 背景と課題
リポジトリ内には初期実装（v0.1.0）から以下の 2 つの独立した実行エンジンが並存していました：
1. **Smithay ベース Wayland コンポジタ** (`src/compositor/`): XWayland を内蔵し、Wayland および X11 アプリケーションを透過的に実行。
2. **レガシー X11 エンジン** (`src/engine.rs`): `Xvfb` + `openbox` + X11 SHM + `XTest` を使用して画面をキャプチャし描画する初期実装。

### 実装された構造（v0.6.0）
- `src/engine.rs`、`src/display/{xvfb,shm,cursor,clipboard}.rs`、`src/input/{mod,xtest}.rs`、および `--x11` / `--xvfb` / `--wm` オプションを廃止し、Smithay + XWayland パイプラインへ完全統合。
- `Cargo.toml` から C `libx11` / `libxtst` / `libxfixes` バインディング（`x11` クレート）への依存を削除し、純粋な Rust + Smithay 構成へ統一。

---

## 4. エラーハンドリング体系の型安全化（v0.6.0 実装済み）

### 背景と課題
パニック（`.unwrap()`）の排除は完了していましたが、エラー型の伝播において `Box<dyn std::error::Error>` や `Result<(), String>` などのアドホックな型が一部に残っていました。

### 実装された構造（v0.6.0）
- 外部依存を追加することなく、標準ライブラリの `std::error::Error` / `Display` / `From` を実装した統一エラー列挙型 [`TermwayError`](../src/error.rs)（`Io`, `NoFreeWaylandSocket`, `X11SocketDirNotWritable`, `XWayland`, `DbusSession`, `Renderer`, `ClientSpawn`, `ClientFailed`, `ConfigParse`）および `termway::Result<T>` を導入。
- コードベース内の `Box<dyn std::error::Error>` および `Result<_, String>` をすべて排除し、完全な型安全化を達成。

---

## 5. コンポジタオーケストレータのサブモジュール分割（v0.7.0 実装済み）

### 背景と課題
`src/compositor/mod.rs` に XDG 環境構築、X11 空きディスプレイ探索、テスト用シグナル処理、タスクバー生成・ヒットテスト、プロファイリング統計、スクリーンショット保存、およびメインイベントループが集約され、1,400 行を超えて肥大化していました。

### 実装された構造（v0.7.0）
単一責任の原則（SRP）に基づき、以下の機能別サブモジュールへ分割しました：
- **[`src/compositor/env.rs`](../src/compositor/env.rs)**: `ensure_xdg_runtime_dir`, `free_x11_display_candidates`, `find_xwayland_binary`, `setup_test_signals`, `log_line`
- **[`src/compositor/taskbar.rs`](../src/compositor/taskbar.rs)**: `Taskbar`, `TaskbarChip`（下部タスクバー文字列構築とクリック座標判定の純粋モジュール化）
- **[`src/compositor/profile.rs`](../src/compositor/profile.rs)**: `CompositorMetrics`（`--profile` / `TERMWAY_PROFILE=1` 時の各ステージ avg/max/p95 レイテンシ集計と帯域レポート出力）
- **[`src/compositor/screenshot.rs`](../src/compositor/screenshot.rs)**: `save_screenshot`（`F12` キーおよび `SIGUSR1` による PNG キャプチャ保存）
- **[`src/compositor/window_manager.rs`](../src/compositor/window_manager.rs)**: `resize_windows`（端末リサイズ時の最大化・浮動ウィンドウ再配置ロジックの移管）

---

## 6. aarch64 ビルド & ARM NEON SIMD 対応（v0.8.0 実装済み）

### 背景と課題
ARM64 Linux（AWS Graviton、Raspberry Pi 4/5、Apple Silicon 上の Linux VM など）での利用ニーズが高まる中、これまでは x86_64 向けバイナリおよび AVX2 最適化のみが提供されており、AArch64 環境向けの公式ビルド成果物やハードウェアベクトルアクセラレーションが未対応でした。

### 実装された構造（v0.8.0）
- **ARM NEON SIMD ベクトル最適化 (`std::arch::aarch64`)**:
  - **タイル差分ハッシュ計算 (`src/display/damage.rs`)**: `uint64x2_t` 64-bit レーン並列積和演算による NEON タイルハッシュ関数 `hash_tile_neon` を実装。
  - **ピクセル色空間変換 (`src/display/framebuffer.rs`)**: 4 要素一括ロード/ストア命令 (`vld4q_u8` & `vst4q_u8`) を用いて、BGRX から RGBA へのチャンネル並べ替えと Alpha=255 付与を CPU レジスタ上で 16 ピクセル単位で瞬時に完結させる `copy_bgrx_to_rgba_neon` を実装。
  - **Sixel RGB332 減色 & 6 行バンド走査 (`src/protocol/sixel.rs`)**: `quantize_rgb332_neon`（16 ピクセル一括減色）および `encode_band_color_neon`（`vceqq_u8` + 6 行重み加算による Sixel 6-bit ASCII ベクトル直接生成）を実装。
- **Polkit Multiarch パス対応 (`src/session/mod.rs`)**:
  - Debian/Ubuntu aarch64 の multiarch 配置パス（`/usr/lib/aarch64-linux-gnu/polkit-mate/...` 等）を `KNOWN_POLKIT_AGENTS` に追加。
- **Ubuntu クロスビルド環境と CI/Release マトリクス (`.github/workflows/`, `scripts/setup_cross_aarch64.sh`)**:
  - Ubuntu 22.04 LTS ホスト上で multiarch (`arm64`) と `gcc-aarch64-linux-gnu` を設定する自動化スクリプトを提供。
  - GitHub Actions CI において QEMU user emulation による aarch64 単体テスト自動検証を追加。
  - GitHub Release において `x86_64-unknown-linux-gnu` と `aarch64-unknown-linux-gnu` のバイナリおよびチェックサムを横並びで自動生成・配布するマトリクスワークフローを構築。

---

## 7. Kitty 部分矩形プレースメント転送（v0.9.0 実装済み）

### 背景と課題
高解像度端末（1080p や 4K）において、テキストエディタのカーソル点滅や 1 文字入力といった画面ごく一部の微小更新時にも全画面のフルフレーム zlib 圧縮および Base64 転送を行っていたため、高 FPS 設定時の CPU 圧縮負荷や PTY 転送帯域が課題となっていました。

### 実装された構造（v0.9.0）
- **部分矩形プレースメント転送 (`encode_frame_into`, `encode_subimage_into`)**:
  - `DamageTracker` の汚損領域バウンディングボックスを文字セル境界へ外接スナップし、画面全体の 75% 以下の変化時は累積バウンディングボックス（$B_{\text{cand}} = B_{\text{prev}} \cup D_{\text{step}}$）として部分サブイメージ（`i=2, p=1`）を 1 パス RGB565 量子化＋zlib 圧縮して `\x1b[{row};{col}H` へダイレクト配置転送。
  - カーソル点滅や文字入力時の圧縮負荷と転送サイズを最大 99.5% 削減（1080p で 900KB → 4.2KB）。
  - サブ画像が過去に覆っていた領域を決して露出させない累積不変条件により、移動オブジェクト（`xeyes`、カーソル、ウィンドウドラッグ）の残像（Ghosting）を完全根絶。
  - スパース膨張検知（カーソル等の大移動）による適応的リベースと 1×1 透明ダミーピクセルによる非破壊中和を採用し、定点動画再生時は不要な全画面フォールバックを排除して無期限・高速な部分ストリーミングを継続。

---

## 8. Wayland Fractional Scale / HiDPI 制御（v0.9.0 実装済み）

### 背景と課題
4K ディスプレイや高 DPI（Retina）ターミナル環境において、等倍（1.0x）表示では GUI アプリケーションのボタン・アイコン・フォントが極小化し視認性が著しく低下していました。また、Wayland 規格における従来の整数倍スケーリング（`wl_output.scale`）のみでは、1.25x や 1.5x といった現代の一般的な HiDPI スケールファクターに正確に追従できず、ボケや座標ズレの原因となっていました。

### 実装された構造（v0.9.0）
- **Wayland Fractional Scale & Viewporter プロトコル統合**:
  - `wp_fractional_scale_v1`（`FractionalScaleManagerState` / `FractionalScaleHandler`）を実装し、クライアントに正確な推奨小数スケール値（120分の1単位）を即時通知。
  - `wp_viewporter`（`ViewporterState`）を統合し、高解像度バッファをサーフェスの論理座標サイズへアトミックにマッピング。
  - サーフェス生成・コミット時に `output.enter(surface)` と `set_preferred_scale(scale)` を行い、Wayland クライアントがネイティブな高精細描画を行えるよう制御。
- **論理座標系と物理描画座標系の分離**:
  - ウィンドウマネージャ（`WindowManager`）、ウィンドウジオメトリ、ドラッグ/リサイズ境界、最大化計算、ポインタ入力座標（SGR マウス変換）をすべて一貫して **論理座標系（Logical Coordinates）** で管理。
  - ターミナルへ出力する Pixman レンダラ（`TermwayRenderer`）および Sixel/Kitty 出力バッファは **物理ピクセル解像度（Physical Coordinates）** で保持し、`render_elements_from_surface_tree` および `draw_render_elements` に `scale` を渡してピクセルパーフェクトに合成。
  - Openbox/Fluxbox 風 SSD 装飾（タイトルバー、枠線、ボタン、アイコン、フォント位置）もスケール係数に追従して自動拡大描画。
- **CLI オプション `--scale <FACTOR>` の導入**:
  - `--scale 1.25`、`--scale 1.5`、`--scale 2.0` など（有効範囲 0.25〜4.0）を柔軟に指定可能（デフォルト: `1.0`）。

---

## 9. 端末ネイティブ体験・相互運用性の向上（v0.10.0 実装済み）

### 背景と課題
ターミナル上で GUI アプリケーションを操作する際、マウスカーソルが矢印のまま変化せずウィンドウ枠線やドラッグ可能箇所の判別が付きにくかった点、Escape キーや複雑な修飾キーの組み合わせがターミナルによって単一コードに潰れてしまう問題、および Linux デスクトップの標準的な習慣である「選択テキストの中クリック貼り付け（Primary Selection）」や「現在アクティブなアプリ名が端末タブに反映される機能」の欠如が、デスクトップネイティブ環境とのギャップとなっていました。

### 実装された構造（v0.10.0）
- **OSC 22 マウスカーソル形状同期 (`\x1b]22;<name>\x07`)**:
  - ウィンドウ境界やリサイズハンドル（`nwse-resize`）、タイトルバー（`grab`）、ボタン（`pointer`）、テキスト入力（`text`）など、ポインタ直下の GUI 要素を追跡し、ホストターミナルのマウスカーソル形状を動的に同期。
- **Kitty Keyboard Protocol プログレッシブ拡張 (`\x1b[>1u` / `\x1b[<u`)**:
  - 端末起動時に CSI u 拡張を有効化。修飾キー（Shift, Ctrl, Alt, Super）の正確な区別や、単体 Escape キーとシーケンスの遅延なき判定を実現。
- **Wayland Primary Selection & 中クリックペースト (`zwp_primary_selection_v1`)**:
  - `zwp_primary_selection_device_manager_v1` および `gtk_primary_selection` を実装し、マウスでテキストを選択するだけで Primary Selection に格納。マウス中クリックによる即時貼り付けに対応。
- **OSC 2 端末タブタイトル動的同期 (`\x1b]2;termway: <Title>\x07`)**:
  - 現在アクティブな GUI ウィンドウのタイトル（タイトルバー文字列）を検知し、ホスト端末のウィンドウ/タブタイトルにリアルタイム反映。終了時には起動前の端末タイトルをクリーンに復元。

---

## 10. SIMD 最適化 zlib エンジン (`zlib-ng`) の統合（v0.10.0 実装済み）

### 背景と課題
Kitty グラフィックスプロトコルの RFC 1950 zlib 圧縮ストリーミング（`o=z`）において、純粋 Rust 実装である `miniz_oxide` はポータブルで安全である一方、フルフレーム（1080p 等の数MB）や高エントロピーなグラデーション/動画コンテンツの圧縮においてスカラー処理のハッシュ探索ボトルネックが存在し、CPU 時間の多くを消費していました。

### 実装された構造（v0.10.0）
- **`flate2` + `zlib-rs`（`zlib-ng` Rust ポート）による SIMD 高速化**:
  - `zlib-ng` プロジェクト公式による純粋 Rust ポート `zlib-rs`（AVX-512 / AVX2 / SSE4.2 / ARM NEON 実行時自動検出対応）を `flate2` バックエンドとして統合。
  - C ツールチェーン（cmake 等）のビルド依存を持たず、クロスコンパイル（aarch64）および純粋な環境でもゼロ設定でビルド可能。
- **Cargo Feature によるデフォルト有効化 (`zlib-ng`) と Level 2 動的ハフマン最適化**:
  - `flate2`/`zlib-rs` を Level 2（動的ハフマン木＋AVX-512/AVX2/NEON ベクトル探索）で構成し、デフォルト有効化（`default = ["zlib-ng"]`）。
  - 一般的なデスクトップ GUI 画面において `miniz_oxide` 比で約 10% 小さい転送サイズを達成し、Noctty (Ghostty) の Wuffs zlib デコーダを含む全端末エミュレータで安定動作を両立。
  - `--no-default-features` を指定した場合は、最小依存の純粋 `miniz_oxide` 実装へ自動フォールバック。
- **実測パフォーマンス検証**:
  - 1080p フルフレーム高エントロピー画像（8.3MB）の圧縮において、スループット **376.2 MB/s → 516.8 MB/s（+37.4% 高速化）**、レイテンシ **22.05ms → 16.05ms（-27.2% 短縮）** を達成。
  - 通常デスクトップ GUI（800x600）において転送サイズ **18.2KB → 16.1KB（-11.5% 帯域削減）**、1080p デスクトップ GUI において **62.7KB → 57.8KB（-7.7% 帯域削減）** を達成。

---

## 11. 16-bit RGB565 色量子化 SIMD エンジンと端末安定性保護（v0.11.0 実装済み）

### 背景と課題
動画再生や高頻度描画時、端末側でのテクスチャ大量確保や一括削除エスケープシーケンス（`\x1b_Ga=d,d=a\x1b\`）によるマルチスレッド競合クラッシュ（Use-After-Free）が発生していました。また、差分オーバーレイ（`i=2`）を移動させると背後の古いベースキャンバス（`i=1`）が露出し、`xeyes` の瞳が二重に表示されるなどのゴースト現象が確認されました。

### 実装された構造（v0.11.0）
- **16-bit RGB565 色量子化 SIMD 前処理 (`quantize_rgb565`)**:
  - 4バイト RGBA32 のアライメントおよび不透明アルファ（`0xFF`）を保持したまま、色チャンネル下位ビット（R: 5bit, G: 6bit, B: 5bit）をゼロマスク。
  - AVX2（`_mm256_and_si256`）および ARM NEON（`vandq_u32`）により 1 フレーム 0.08ms で前処理し、zlib-ng 可逆圧縮効率を劇的に向上（転送サイズ 55〜60% 削減）。
- **固定画像 ID 方式（ADR-0002）と端末クラッシュ防止**:
  - `IMAGE_ID_CANVAS = 1` によるインプレース上書きを採用し、一括削除 `a=d,d=a` を廃止。Noctty / Ghostty 系ターミナルのクラッシュを根絶。
- **動的描画のゴースト根絶（全画面標準更新 & `TERMWAY_KITTY_PARTIAL` オプトイン）**:
  - ダーティ領域移動時のゴーストを排除するため、デフォルトで全画面キャンバスの直接更新を採用（16-bit 量子化により xeyes 等は 5〜15KB で超高速転送）。
  - デバッグ・ベンチマーク用オプトイン環境変数 `TERMWAY_KITTY_PARTIAL=1` を提供。
- **実測パフォーマンス検証 (YouTube 720p 2分間連続再生)**:
  - クラッシュ 0（完全安定）、描画フレームレート **3.9 FPS → 7.4 FPS（+102.8%）**、フレームドロップ率 **50.3% → 10.6%（-76.1% 減少）**、PTY 書込遅延 **137.5ms → 66.2ms（-51.8% 半減）** を実証。

---

## 12. Kitty Graphics Protocol `f=101` (JPEG) 拡張と 30fps パイプライン最適化（v0.12.0 / v0.13.0 実装済み）

### 背景と課題
Kitty グラフィックスプロトコルの標準仕様（RFC 1950 zlib 圧縮 `f=32,o=z`）はロスレス圧縮として優れた画質を提供する一方、720p/1080p 動画再生や写真・3D グラフィックなど高エントロピー画像において圧縮率の頭打ちと CPU 圧縮負荷（DEFLATE ハフマン探索）が課題となっていました。
また、公式の Noctty や Ghostty を用いた高負荷な動画連続ストリーミングでは、端末側の GPU テクスチャ管理・描画競合に起因して端末自体がクラッシュ（SIGSEGV）する既知の課題がありました。
これに対し、Kitty Graphics Protocol 独自拡張フォーマット `f=101`（Baseline JPEG）および高負荷クラッシュ対策を実装した独自拡張版の Noctty（※本家 upstream には未マージの実験的フォーク）が開発されたため、termway 側でもこの超高圧縮・超低遅延フォーマットを最大限に活用しつつ、未対応の端末（公式 Noctty、Kitty 本家、Ghostty 本家、WezTerm 等）を破壊しない後方互換性と動的フォールバックが求められました。
さらに、動画再生時のフレームレートを 20〜30fps へ引き上げるため、Wayland コンポジタのポーリング遅延解消およびエンコード演算の最適化（除算完全排除、境界判定バイパス、転送サイズ削減）を実施しました。

### 実装された構造（v0.12.0 / v0.13.0）
- **ゼロアロケーション Baseline JPEG エンコーダ (`src/protocol/jpeg.rs`)**:
  - 外部 C ライブラリや動的ヒープ確保に依存しない、純粋 Rust によるフルスクラッチの Baseline JPEG（SOF0、YCbCr 4:2:0 サブサンプリング）エンコーダを実装。
  - ITU-T T.81 / JPEG Annex K 標準量子化テーブルおよびハフマン符号化テーブルを採用。
  - Loeffler / LLM (Ligtenberg-Loeffler-Moschytz) 高速整数 1D/2D FDCT アルゴリズム（固定小数点スケーリング）による高速 DCT 変換。
  - 事前計算した固定小数点逆数乗算（`u64` 乗算＋33-bit シフト）により、量子化時のハードウェア整数除算（`idiv`）を完全排除。
  - フレームバッファ境界内の大半のブロックに対する境界クランプをバイパスする高速パスを実装。
  - `0xFF 0x00` バイトスタッフィングおよびストリーミング Base64 チャンク出力に対応。
- **コンポジタ・イベントループの 30fps 解禁と遅延ゼロ化 (`src/compositor/mod.rs`)**:
  - 描画完了時の `send_frames_surface_tree()` 直後に即座に `self.display.flush_clients()` を呼び出し、Wayland クライアントへの `wl_callback.done` 送信遅延（従来の 50ms `poll` タイムアウト待ち）を完全解消。
  - `poll_fds` に `event_loop` (calloop) を統合し、XWayland やシグナルイベントも遅延なく即時起床。
- **Kitty プロトコル連携と透過ダミー保護 (`src/protocol/kitty.rs`)**:
  - `KittyStreamEncoder` に `use_jpeg` モードおよび品質パラメータ（`jpeg_quality: 1..=100`、デフォルト 75）を統合。
  - 全画面ストリーミングおよび部分更新矩形（`i=2, p=1`）双方で `f=101` チャンクを自動送出（JPEG 圧縮済みのため `o=z` は省略）。
  - JPEG はアルファチャンネルを持たないため、部分更新中和用の 1x1 透過ダミーピクセル（`clear_placements`）は安全に `f=32` を維持（Noctty / Kitty 双方で `f=101` 画像を `f=32` ダミーで正常に上書き可能）。
- **動的機能検知バリア & フォールバック (`src/terminal/query.rs`, `src/backend/terminal.rs`)**:
  - 端末起動時にクエリシーケンス `\x1b_Gi=32,s=1,v=1,a=q,t=d,f=101;<1x1_JPEG>\x1b\` と Primary Device Attributes (DA) バリア `\x1b[c` を連続送出。
  - 端末からの応答を受信し、`\x1b_Gi=32;OK\x1b\` を検知した場合は `f=101` JPEG モードを有効化。未対応端末やエラー応答時はタイムアウトを待つことなく安全に `f=32,o=z`（RGB565 + zlib）へ自動フォールバック。
  - 端末名（HUD やログ）に `[Kitty-JPEG]` / `[Kitty]` を区別表示。
- **環境変数および CLI フラグによる制御**:
  - `TERMWAY_KITTY_JPEG`: `1` / `true`（強制有効）、`0` / `false`（強制無効）、`auto` / 未設定（自動プローブ検知）。
  - `TERMWAY_JPEG_QUALITY`: `1..=100`（デフォルト: `75`）。
  - CLI オプション: `--jpeg`、`--no-jpeg`、`--jpeg-quality <1-100>`。
- **実測パフォーマンス検証 (YouTube 連続再生プロファイル)**:
  - 描画フレームレート: **8.70 fps → 19.75 fps (+127% / 2.27倍向上、約 20fps 達成)**
  - アイドル待機スキップ (`clean_skips`): **912 回 (49%) → 176 回 (8%) に激減**
  - 平均 PTY 書込時間 (`avg_write_ms`): **13.68ms → 10.74ms (-21.5%)**
  - 1 フレーム平均データサイズ: **67.6 KB → 55.0 KB (-18.6% 削減)**
  - 転送スループット: **574.8 KB/s → 1,061.6 KB/s (1.06 MB/s)**

---

## 13. 完了済みマイルストーン一覧

| マイルストーン | 項目 | 優先度 | 達成効果 | ステータス |
| :--- | :--- | :---: | :--- | :--- |
| **フェーズ 1**（パフォーマンス基礎刷新） | メモリアロケーション排除（バッファ再利用）＆ 差分更新（Damage Tracking） | 最優先 | CPU負荷大幅削減、リモート/SSH時の転送量 80〜95% 削減 | **実装済み (v0.4.0)** |
| **フェーズ 2**（パイプライン非同期化） | レンダリング・エンコードのワーカースレッド分離 ＆ 最新フレーム間引き | 最優先 | 高負荷・高解像度時の入力レイテンシ削減、超低遅延応答 | **実装済み (v0.4.0)** |
| **追加最適化**（プロトコル最適化） | Kitty RFC 1950 zlib圧縮 (`o=z`) ＆ 同期更新モード (`\x1b[?2026h`) | 最優先 | Kitty転送量98%削減、チラつき・テアリングの完全解消 | **実装済み (v0.4.0)** |
| **フェーズ 3**（エンコーダ最適化） | Sixel / Kitty エンコーダ最適化（水平有界探索・ブランチレス走査・ゼロアロケーション整形） | 高 | エンコード時間短縮、バッファブロート防止 | **実装済み (v0.4.0)** |
| **フェーズ 3.5**（SIMD高速化＆UX拡張） | AVX2 SIMD命令によるDamage/Sixel/XImage高速化、動的端末リサイズ (`SIGWINCH`)、リアルタイムHUD (`--hud`)、起動時スプラッシュ (`--no-splash`) | 高 | 差分走査 0.2ms / Sixel 3.7ms 達成、端末リサイズ追従とテレメトリ可視化 | **実装済み (v0.5.0)** |
| **フェーズ 4**（モジュール構造刷新） | `TermwayState` のドメイン分割 (`WindowManager`, `handlers/{compositor,xdg_shell,seat}`) | 中 | コード可読性・拡張性の向上、単体テスト容易性 | **実装済み (v0.6.0)** |
| **フェーズ 5**（エンジン統合） | レガシー Xvfb / X11 エンジンの廃止と Smithay Wayland + XWayland コンポジタへの完全一本化 | 中 | アーキテクチャの単純化、C Xlib 依存排除、保守コスト大幅削減 | **実装済み (v0.6.0)** |
| **フェーズ 6**（エラー型安全化） | 統一エラー列挙型 `TermwayError` (`src/error.rs`) の導入と `Box<dyn Error>` / `String` エラーの完全排除 | 中 | エラー伝播の型安全化、パターンマッチによるテスト容易性向上 | **実装済み (v0.6.0)** |
| **フェーズ 7**（性能プロファイリング＆ストリームエンコーダ最適化） | パイプライン段別ベンチマーク (`tests/perf_pipeline.rs`) とランタイムプロファイラ (`--profile`) の導入、Sixel 6px ダーティバンドキャッシュ (`SixelStreamEncoder`)、Kitty ゼロアロケーション zlib リセット & Base64 ストリーミング (`KittyStreamEncoder`) | 最優先 | Sixel カーソル・部分更新レイテンシ 92〜95% 削減 (4.3ms→0.34ms)、Kitty 動的ヒープ確保ゼロ化 | **実装済み (v0.7.0)** |
| **フェーズ 8**（統一ポーリング＆フレームペーシング＆モジュール分割） | `stdin` + `listener` + `display.backend().poll_fd()` の単一 `poll` 統合によるアイドル時 CPU 0% 化、`--fps` 目標フレームレート制御、Pixman レンダラ パス統合、`src/compositor/mod.rs` のサブモジュール分割 (`env.rs`, `taskbar.rs`, `profile.rs`, `screenshot.rs`, `window_manager.rs`) | 高 | アイドル時 CPU 負荷解消、高速コミット時の過度な再描画抑制、コンポジタ可読性・保守性向上 | **実装済み (v0.7.0)** |
| **フェーズ 9**（aarch64 ビルド & ARM NEON SIMD 対応） | ARM64 Linux 向け NEON ベクトル最適化（Damage / Sixel / BGRX 変換）、Ubuntu 22.04 LTS クロスビルド環境整備、CI QEMU テスト & Release マトリクス配布 | 高 | AWS Graviton / Apple Silicon / Raspberry Pi 等でのネイティブ高速動作、x86_64/aarch64 横並びリリース配布 | **実装済み (v0.8.0)** |
| **フェーズ 10**（Kitty 部分矩形プレースメント転送） | セルスナップ外接矩形判定による Kitty 部分プレースメント転送 (`encode_frame_into`) とアトミック定期リフレッシュ | 高 | 微小更新時の zlib 圧縮負荷および帯域 99.5% 削減（1080p で 900KB→4.2KB）、高解像度端末での超低遅延応答 | **実装済み (v0.9.0)** |
| **フェーズ 11**（Wayland Fractional Scale / HiDPI 制御） | `wp_fractional_scale_v1` / `wp_viewporter` プロトコル統合、`--scale <FACTOR>` オプション、出力スケールおよびサーフェス論理/物理座標マッピング | 高 | 4K/Retina高DPI環境でのUI・フォント鮮明表示、小数倍スケール（1.25x/1.5x等）による視認性向上 | **実装済み (v0.9.0)** |
| **フェーズ 12**（端末ネイティブ体験・相互運用性の向上） | OSC 22 マウスカーソル同期、Kitty Keyboard Protocol (CSI u)、Wayland Primary Selection (中クリックペースト)、OSC 2 端末タブタイトル動的同期 | 高 | デスクトップGUIと端末の境界をシームレス化、直感的なポインタ変化・クリップボード・キーボード操作 | **実装済み (v0.10.0)** |
| **フェーズ 13**（SIMD 最適化 zlib-ng エンジン） | `flate2`/`zlib-rs` による SIMD (AVX-512/AVX2/NEON) 最適化 zlib-ng エンジン（Level 2 動的ハフマン木チューニング）のデフォルト統合（`--no-default-features` 時に純粋 miniz_oxide フォールバック） | 中 | 帯域約 10% 削減、フルフレーム圧縮スループット 2,300MB/s 超、全端末との堅牢な互換性両立 | **実装済み (v0.10.0)** |
| **フェーズ 14**（16-bit RGB565 色量子化 SIMD ＆ 端末安定性保護） | AVX2/NEON による 16-bit RGB565 色量子化前処理（圧縮サイズ 55〜60% 削減）、端末クラッシュ防止固定画像 ID 方式（ADR-0002）、動的描画ゴースト根絶（全画面標準更新 & `TERMWAY_KITTY_PARTIAL` オプトイン） | 最優先 | YouTube 2分間超連続再生ノーカット安定動作、描画FPS 2倍（3.9→7.4 FPS）、フレームドロップ 76% 激減、xeyes/ウィンドウ移動時の残像完全解消 | **実装済み (v0.11.0)** |
| **フェーズ 15**（Kitty Graphics `f=101` JPEG 拡張 ＆ 動的フォールバック） | 純粋 Rust ゼロアロケーション Baseline JPEG エンコーダ、Noctty `f=101` ストリーミング対応、クエリ＆DAバリア動的検知、未対応端末の透過 `f=32,o=z` 安全フォールバック | 高 | 動画・高エントロピー画面での圧縮率大幅向上、PTY 送信遅延短縮、全 Kitty 互換端末でのシームレス動作両立 | **実装済み (v0.12.0)** |
| **フェーズ 16**（Wayland 30fps イベントループ即時フラッシュ ＆ JPEG 高速化） | Wayland `send_frames_surface_tree` 後の即時 `flush_clients()` による 50ms ポーリング待機遅延解消、固定小数点逆数乗算（除算排除）＆境界バイパスによる JPEG エンコード高速化、デフォルト品質 75 適正化、JSON プロファイル出力 | 最優先 | 実効描画レート 2.27 倍向上（8.7 fps → 19.8 fps / 約 20 fps 達成）、無駄な待機スキップ 81% 削減、PTY 書込遅延 21.5% 短縮、フレーム当たり帯域 18.6% 削減 | **実装済み (v0.13.0)** |

---

## 14. 今後の展望 (Future Horizons)

| テーマ | 概要 | 期待される価値 |
| :--- | :--- | :--- |
| **Direct Scanout / DRM バックエンド探索** | ヘッドレス/仮想端末に加え、DRM/KMS 直接描画バックエンドの実験的検討 | 端末を介さないベアメタル/コンソール環境での直接GUI動作 |
| **ローカル接続時の動的無圧縮（Raw RGBA）バイパス** | Kitty プロトコルの無圧縮 RGBA 転送を活用し、ローカル実行環境（非SSH）において zlib 圧縮処理をバイパス（`--no-compress`） | 帯域制限のないローカル環境での CPU 負荷削減と極小描画レイテンシの実現 |
| **次世代圧縮アルゴリズム（Zstandard / LZ4 等）の検証** | Kitty 規格外（RFC 1950 zlib 以外）の超高速圧縮方式（Zstandard / LZ4 / QOI 等）のフィージビリティ調査。端末エミュレータ側の標準規格拡張やカスタム拡張ハンドシェイクの可能性を検証 | zlib を超える極小圧縮遅延（GB/s 級）の実現可能性の模索 |


