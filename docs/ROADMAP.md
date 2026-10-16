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

## 2. パフォーマンス最適化アーキテクチャ

### 2.1 差分更新（Damage Tracking / Partial Redraw）

#### 現状と課題
現在、Wayland コンポジタは画面の一部（例: xeyes の瞳移動、エディタのカーソル点滅、時計の更新など）が変化しただけでも、**毎フレーム全画面（全ピクセル・数MB）を再エンコードして端末へ送信**しています。
高解像度端末やリモート接続環境（SSH 経由など）において、不要な全画面データが大量に流れることでネットワーク帯域を逼迫させ、著しいフレームレート低下と転送遅延を引き起こしています。

#### 改善方針
- **ダメージ領域の局所抽出**: Smithay の `damage_tracker` から更新のあった矩形領域（Damage Rect）のみを抽出。
- **Kitty プロトコルの部分配置**: Kitty Graphics Protocol は配置座標（行・列）や画像位置を指定できるため、変化のあった矩形領域のみをピンポイントでエンコード・上書き配置。
- **Sixel の行バンド・矩形差分更新**: 変化のあった 6 ピクセル単位のバンドのみ送信するか、エスケープシーケンスによるカーソル移動（`\x1b[行;列H`）を併用して差分領域のみ転送。
- **期待効果**: データ転送量および CPU エンコード負荷を **80%〜95% 削減**。

### 2.2 フレームレンダリング・画像エンコードのパイプライン分離（非同期化）

#### 現状と課題
現在、フレームの Pixman レンダリングおよび端末用画像プロトコル（Kitty Graphics Protocol / Sixel）へのエンコード処理は、Smithay のメインイベントループ（Calloop）内で同期的に実行されています（`src/compositor/mod.rs`）。
高解像度端末や大量の画面更新（動画再生、激しいスクロール等）が発生した際、エンコード処理の CPU 負荷や stdout 書き込みの I/O ブロックによりイベントループが停止し、マウスポインタやキー入力の応答レイテンシが増加します。

#### 改善方針
レンダリングパイプラインをイベント駆動型のワーカースレッドへ分離します：
1. **イベントループ**: ダメージ領域の収集とサーフェスバッファのコミット通知のみを高速にディスパッチ（1000Hz 以上の超低レイテンシ入力応答）。
2. **レンダリング/エンコードスレッド**: ダメージ通知をチャネル経由で受け取り、バックグラウンドで画像エンコードを実行。
3. **最新フレーム間引き（Frame Dropping / Rate Limiting）**: ワーカースレッドが前のフレームを処理中に新しいフレームが到着した場合、古いフレームを破棄して「常に最新の1フレームのみ」を処理。端末や PTY のバッファ溢れを完全に防止。

### 2.3 メモリアロケーションの排除（ゼロアロケーション / バッファ再利用）

#### 現状と課題
`SixelEncoder` や `KittyEncoder` は毎フレーム数MBの `Vec<u8>` や `String` を動的に確保・破棄しています。毎秒数十回の巨大アロケーションにより、glibc malloc のロック競合と CPU キャッシュ汚染が発生し、フレーム時間のブレ（ジッター）の原因となっています。

#### 改善方針
- エンコード出力用バッファおよび一時作業バッファを構造体内で事前確保し、毎フレーム `buffer.clear()` して再利用。
- 描画ループ中のヒープアロケーションをほぼゼロ化。

### 2.4 エンコーダの並列化・SIMD 最適化

#### 改善方針
- **Sixel 量子化**: 1 画素ごとのビットシフト・パレット集計処理を、Rayon によるバンド並列化または SIMD 命令（AVX2 / NEON）を用いた一括変換に最適化。
- **Kitty Base64 変換**: SIMD 最適化 Base64 クレート（`base64-simd` など）を導入し、数MBのエンコード時間を 1/3〜1/4 に短縮。

### 2.5 バックプレッシャー制御（Frame Pacing）

#### 改善方針
端末や SSH ソケットの受信能力を超えたフレーム送出を検知・調停し、ソケットバッファの滞留による「操作が数秒遅れて反映されるバッファブロート現象」を根本防止。

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

## 5. ロードマップ概要

| マイルストーン | 項目 | 優先度 | 期待効果 | ステータス |
| :--- | :--- | :---: | :--- | :---: |
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



