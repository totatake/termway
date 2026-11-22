# 要求仕様書 (`termway`)

本書は、`termway` の開発において策定された機能要件（`FR-*`）および非機能要件（`NFR-*`）を定義します。システム全体構成は [architecture.md](architecture.md)、要件トレーサビリティおよびテスト仕様一覧は [testing.md](testing.md) を参照してください。

---

## 1. 機能要件 (Functional Requirements)

| 要件ID | 要件名 | 詳細説明 | 対応テスト | 結果 |
| :--- | :--- | :--- | :--- | :--- |
| **FR-1** | 端末グラフィックス描画 (Kitty) | コンポジタの RGBA フレームバッファを RFC 1950 zlib 圧縮 (`o=z`) + Base64 で Kitty Graphics Protocol にエンコードし、最背面レイヤ (`z=-2147483648`) および `rows - 1` 領域へ出力すること。応答抑制 (`q=2`)・カーソル移動抑制 (`C=1`) を指定すること。 | `tests/unit_kitty_encoder.rs`<br/>`tests/system_e2e_gui.rs` | **PASS** |
| **FR-2** | 差分検出・更新 (Damage) | 画面を 32x32 タイルに分割し、AVX2 SIMD 並列ハッシュ比較により変化のあった汚損領域のみを検出して無駄な再エンコード・転送を抑止すること。 | `tests/unit_damage_tracker.rs`<br/>`tests/system_e2e_gui.rs` | **PASS** |
| **FR-3** | マウス入力・ホバー変換 | 端末からの xterm SGR 1006 マウスシーケンス（クリック、ドラッグ、リリース、ホバー移動、ホイール）を Any-event モード（`1003h`）で検知し、セル座標からコンポジタ空間のピクセル座標へマッピングして Smithay `Seat` のポインタイベントへ送出すること。 | `tests/unit_sgr_mouse.rs`<br/>`tests/integration_pipeline.rs`<br/>`tests/unit_compositor.rs` | **PASS** |
| **FR-4** | キー入力・修飾キー解析 | 端末 stdin からの UTF-8 文字・特殊キー・Shift/Ctrl 修飾キー付きカーソルシーケンスをノンブロッキングで解析し、Smithay `Seat` のキーボードイベントとして即時ディスパッチすること。 | `tests/unit_keyboard.rs` | **PASS** |
| **FR-5** | 端末制御と安全復元 | 起動時に `TIOCGWINSZ` で端末サイズを取得し、端末を Raw モードおよび Alternate Screen に切り替えること。終了時やパニック発生時には `RawModeGuard` (`Drop`) により確実に通常画面・通常モードに復元すること。 | `tests/unit_terminal.rs` | **PASS** |
| **FR-7** | 端末プロトコル適合性検証 | 起動時に環境変数（`TERM`, `TERM_PROGRAM`, `WT_SESSION`, `TERMWAY_FORCE_PROTOCOL` 等）およびターミナルへのエスケープクエリ（Kitty `a=q` / Primary DA `\x1b[c`）を用いて Kitty / Sixel の対応可否を自動判定すること。VS Code 統合ターミナル等（xterm.js）が Kitty クエリに偽陽性 ACK を返しつつ描画未対応となる問題に対し、環境変数ファストパス（`TERM_PROGRAM=vscode`, `VSCODE_SHELL_INTEGRATION` 等）および DA1 Sixel 宣言（`4`）とのクロス検証により正確に Sixel を優先選択すること。 | `src/terminal/query.rs`<br/>`tests/integration_pipeline.rs` | **PASS** |
| **FR-10** | 双方向クリップボード同期 | Wayland (`wl_data_device`) および XWayland セレクション更新時にテキストをパイプ経由（同時読込数・1MiB容量・1000msタイムアウト制限あり）で安全に読み取り、OSC 52 エスケープシーケンス（`\x1b]52;c;...` / `\x1b]52;p;...`）でホスト端末へ同期すること（平文クリップボード内容を標準エラーログに出力せず、`--no-osc52` / `TERMWAY_NO_OSC52=1` で無効化可）。また、Bracketed Paste 等によるホストからの貼り付け（1MiB 上限保護付き）を双方向サポートすること。 | `tests/unit_compositor.rs` | **PASS** |
| **FR-12** | Sixel対応＆ピクセル追従 | Sixel プロトコルでの RGB332 減色・RLE 描画をサポートし、`\x1b[16t` / `\x1b[14t` により端末の物理セルピクセル寸法を動的取得して 1:1 アスペクト比とマウス座標の完全一致を実現すること。 | `tests/unit_sixel_encoder.rs`<br/>`tests/unit_terminal.rs` | **PASS** |
| **FR-16** | Smithay Wayland コンポジタ＆XWayland 統合 | Smithay ベースのヘッドレス Wayland コンポジタおよび XWayland (`X11Wm`) を統合し、Wayland ネイティブアプリとレガシー X11 アプリの双方を単一エンジンで実行・描画できること（仮想解像度は最大 7680x4320 に上限クランプ）。 | `tests/unit_compositor.rs`<br/>`tests/system_e2e_gui.rs`<br/>`tests/integration_pipeline.rs` | **PASS** |
| **FR-18** | ハイブリッド D-Bus セッション＆Secret Service 中継 | 単一インスタンス型アプリ用の隔離 D-Bus セッションと Snap アプリ用の Ambient バス自動ルーティング（`XDG_RUNTIME_DIR` 検証付き PATH ラッパーシム、`--no-host-keyring` / `TERMWAY_NO_HOST_KEYRING=1` オプトアウト対応）を両立し、さらにホストの `org.freedesktop.secrets` キーリングを Allow-list 検証・破壊的コレクション削除遮断・メッセージサイズ/保留件数上限（4MiB / 256件）付きで安全に中継すること。 | `tests/unit_session.rs`<br/>`tests/unit_secret_bridge.rs` | **PASS** |
| **FR-19** | Polkit 認証エージェント連携 | システム内の Polkit 認証エージェント（`polkit-gnome`, `lxpolkit` 等）を自動検出し、特権 GUI ツール（`gparted`, `pkexec` 等）のパスワードダイアログをターミナル内に表示すること（`--no-polkit` / `TERMWAY_NO_POLKIT=1` で自動起動を無効化可能）。 | `tests/unit_session.rs`<br/>`tests/integration_pipeline.rs` | **PASS** |
| **FR-21** | コマンド存在性・PATH 事前検証 | 起動時に指定されたコマンドの存在を `PATH` から事前検証し、存在しない場合は終了コード 127 で即時終了すること。 | `tests/unit_session.rs` | **PASS** |
| **FR-22** | `/tmp/.X11-unix` 書込権限事前検査 | XWayland 起動前に `/tmp/.X11-unix` ディレクトリへの書込可否を検証し、リードオンリー環境でのハングアップやホスト DISPLAY へのエスケープを防止すること。 | `tests/unit_session.rs` | **PASS** |
| **FR-23** | Wayland データデバイスフォーカス & シリアル整合性 | Wayland Seat においてフォーカス切り替え時に `set_data_device_focus` を通知し、単調増加シリアル番号を保証すること。 | `tests/unit_compositor.rs` | **PASS** |
| **FR-24** | Wayland XDG Popup & X11 Override-Redirect メニュー対応 | Wayland `xdg_popup` および XWayland override-redirect ウィンドウを追跡・最前面描画・優先ヒットテストし、ドロップダウンメニューやコンテキストメニューを正常操作できること。 | `tests/unit_compositor.rs` | **PASS** |
| **FR-25** | ウィンドウ最小化・最大化・タスクバー管理・画面外ドラッグおよび 8方向リサイズ (`WindowManager`) | タイトルバーの最小化 `[-]`・最大化 `[□]`・閉じる `[×]` ボタンおよび最下部タスクバーチップによるウィンドウ状態制御・80%中央サイズ復元・ドメイン独立管理（`WindowManager`）、画面外ドラッグ移動（パーシャル・オフスクリーン移動、最低48px安全マージン保証）、および外枠4辺・4隅の8方向リサイズ（最小サイズガード、loc & size 協調計算）を行うこと。 | `tests/unit_compositor.rs` | **PASS** |
| **FR-26** | 動的ターミナルリサイズ追従 (`SIGWINCH`) | `SIGWINCH` シグナル検知時にコンポジタ出力・Pixman バッファ・DamageTracker・最大化/浮動ウィンドウを動的にリサイズ追従させること。 | `tests/unit_compositor.rs` | **PASS** |
| **FR-27** | リアルタイムパフォーマンス HUD (`--hud`) | `--hud` 指定時に FPS、解像度、転送帯域（KB/s）、エンコード時間（ms）を画面右上にオーバーレイ表示すること。 | `tests/unit_compositor.rs` | **PASS** |
| **FR-28** | 起動時スプラッシュスクリーン (`--no-splash`) | 最初のウィンドウが表示されるまでの間、ブルー〜シアンのグラデーションロゴ `>_ TERMWAY` とカーネル風テレメトリログを描画し、ウィンドウ表示時に自動消去すること（`--no-splash` で非表示化可能）。 | `tests/unit_compositor.rs` | **PASS** |
| **FR-29** | パフォーマンスプロファイリング (`--profile`) | `--profile` オプションまたは環境変数 `TERMWAY_PROFILE=1` 指定時に、レンダリング・エンコード・I/Oステージごとの詳細レイテンシテレメトリ（avg/max/p95）を出力すること。また環境変数 `TERMWAY_PROFILE_FILE=<PATH>` 指定時に JSON 形式での出力に対応すること。専用ベンチマークスイートによる継続的リグレッション検知を保証すること。 | `tests/perf_pipeline.rs`<br/>`tests/unit_compositor.rs` | **PASS** |
| **FR-30** | Sixel ダーティバンドキャッシュ & Kitty ストリーミング・部分矩形プレースメント | Sixel エンコーダにおいて 6 行バンド単位の FNV-1a ハッシュキャッシュによる完全同一バンドスキップを行い、Kitty エンコーダにおいては `KittyStreamEncoder` によるゼロアロケーションバッファ再利用ストリーミングおよび累積バウンディングボックス部分矩形プレースメント転送（75%以下の累積汚損領域の部分配置転送、スパース膨張時の適応的リベース、定点動画の無制限連続部分転送、1パスRGB565量子化）を実現すること。 | `tests/unit_sixel_encoder.rs`<br/>`tests/unit_kitty_encoder.rs` | **PASS** |
| **FR-31** | 統一 FD ポーリング & `--fps` フレームペーシング | `calloop` イベントループにおいて stdin, Wayland display, XWayland, clipboard, signal の全記述子を単一 epoll で統一監視し、`--fps`（デフォルト 30fps）による適応型フレームペーシング制御を行うこと。 | `tests/unit_compositor.rs`<br/>`tests/integration_pipeline.rs` | **PASS** |
| **FR-34** | Wayland Fractional Scale (`wp_fractional_scale_v1`) & Viewporter (`wp_viewporter`) & HiDPI制御 (`--scale`) | `--scale <FACTOR>`（0.25〜4.0）による動的スケール指定、`wp_fractional_scale_v1`（120分の1単位）および `wp_viewporter` 拡張プロトコルのアドバタイズ、サーフェスコミット時の `preferred_scale` 通知、論理/物理座標系の分離（ウィンドウ管理・装飾・マウス入力・Pixman合成）を保証すること。 | `tests/unit_compositor.rs` | **PASS** |
| **FR-35** | OSC 22 マウスカーソル形状同期 | ウィンドウの枠線（4辺・4隅の 8 方向リサイズ: `ew-resize`, `ns-resize`, `nwse-resize`, `nesw-resize`）、タイトルバー（`grab` / `grabbing`）、ボタン（`pointer`）などのホバーおよびドラッグ対象に応じて、ホスト端末のマウスカーソル形状を動的に同期（`\x1b]22;<name>\x07`）すること。 | `tests/unit_compositor.rs` | **PASS** |
| **FR-36** | Kitty Keyboard Protocol (CSI u) | 起動時に `\x1b[>1u` を送信してプログレッシブなキーボード拡張を有効化し、修飾キーやエスケープシーケンスの曖昧性を解消すること（終了時に `\x1b[<u` で安全復元）。 | `tests/unit_keyboard.rs` | **PASS** |
| **FR-37** | Wayland Primary Selection & 中クリックペースト | `zwp_primary_selection_device_manager_v1` および `gtk_primary_selection` を実装し、マウスによるテキスト選択範囲の即時共有とマウス中クリックによる即時貼り付けをサポートすること。 | `tests/unit_compositor.rs` | **PASS** |
| **FR-38** | OSC 2 端末ウィンドウ/タブタイトル動的同期 | アクティブな GUI ウィンドウのタイトルを検知し、ホスト端末のウィンドウ/タブタイトルへ動的同期（`\x1b]2;termway: <Title>\x07`）すること（終了時に安全復元）。 | `tests/unit_compositor.rs` | **PASS** |
| **FR-39** | Kitty Graphics JPEG 拡張 (`f=101`) & 30fps 即時フレームフラッシュ | 純粋 Rust によるフルスクラッチ Baseline JPEG (YCbCr 4:2:0) エンコーダと Kitty `f=101` 拡張をサポートし、固定小数点逆数乗算による除算排除やフレームバッファ直読み境界バイパス高速化を行うこと。Wayland コンポジタ描画直後に `flush_clients()` を行い、クライアントの `wl_callback.done` 待ちによる 50ms ポーリング遅延を解消して 30fps 以上の高フレームレートを実現すること。未対応端末では自動的に標準 Kitty (`f=32`) へフォールバックすること。 | `src/lib.rs`<br/>`tests/unit_kitty_encoder.rs`<br/>`tests/integration_pipeline.rs`<br/>`tests/unit_compositor.rs` | **PASS** |
| **FR-40** | SGR-Pixels 高精度 1px マウストラッキング (`\x1b[?1016h`) | 対応端末（Kitty, WezTerm, Foot, Ghostty）の自動検知および `--pixel-mouse` / `--no-pixel-mouse` CLI 制御に対応し、セル単位（10〜30px）の量子化を排除して端末から受信した 1-based ピクセル座標を Wayland ポインタへ 1px 単位でダイレクトマッピングすること。YouTube シークバー等の極小 UI 要素の正確なクリック・ドラッグ操作を保証すること。 | `src/terminal/raw.rs`<br/>`src/terminal/query.rs`<br/>`tests/unit_sgr_mouse.rs`<br/>`tests/unit_compositor.rs`<br/>`tests/integration_pipeline.rs` | **PASS** |
| **FR-41** | シグナル安全 & パニック安全な端末復元 | SIGINT/SIGTERM/SIGHUP 受信時および万が一の panic 発生時に、端末 raw モード、マウス追従、タイトル、Kitty キーボードプロトコル、および同期描画モード（`\x1b[?2026l`）を確実に復元・解除すること。 | `src/terminal/raw.rs`<br/>`src/compositor/env.rs`<br/>`tests/unit_terminal.rs` | **PASS** |
| **FR-42** | 端末機能診断 (`--query`) & 拡張プロファイル (p99) | `--query` フラグにより端末解像度、グラフィックスプロトコル、Kitty JPEG、マウストラッキング、XWayland、D-Bus、Polkit の対応状況を即時表示すること。`TERMWAY_NO_CURSOR_SHAPE` による OSC 22 抑制、`TERMWAY_FPS` による環境変数フレームレート指定、および `--profile` での p99 レイテンシ集計・JSON 出力をサポートすること。 | `src/main.rs`<br/>`src/compositor/profile.rs`<br/>`tests/integration_pipeline.rs`<br/>`tests/unit_compositor.rs` | **PASS** |
| **FR-43** | ゼロアロケーション・タイムライントレーシング (`--trace`, Perfetto) | `--trace`, `--trace-file`, `--trace-buffer`, `--trace-flush-secs`, `--trace-no-input` オプションおよび対応環境変数をサポートし、32バイト固定長イベントとスレッド独立リングバッファによるホットパスヒープ確保ゼロのタイムライン記録、Chrome Trace Event JSON (および `.gz`) 出力、`frame_id` 非同期フロー追跡、インプレースマウス共結成、連番スナップショット、panic 時安全ダンプ、HUD 溢れ表示 (`Ovf`) を保証すること。 | `src/trace/`<br/>`tests/unit_trace.rs` | **PASS** |
| **FR-44** | Wayland ポインタフレーム分離 & 水平スクロール (GTK/Tauri 互換) | SGR マウスシーケンスにおける水平スクロール（`66`/`67`）を `WheelLeft`/`WheelRight` としてパースし、Wayland `Axis::Horizontal` としてディスパッチすること。`wl_pointer.motion` と `wl_pointer.axis`（および `button`）を独立した論理フレーム（`wl_pointer.frame`）で分離送出することで、GTK3 / WebKitGTK / Tauri アプリ（openidea 等）におけるスクロールイベント早期破棄を防止すること。さらに `under_from_surface_tree` によりサブサーフェス（`wl_subsurface`）への正確なヒットテストを行うこと。 | `src/protocol/sgr_mouse.rs`<br/>`src/compositor/input.rs`<br/>`tests/unit_sgr_mouse.rs`<br/>`tests/unit_compositor.rs` | **PASS** |

---

## 2. 非機能要件 (Non-Functional Requirements)

| 要件ID | 項目 | 目標基準 | 検証結果 |
| :--- | :--- | :--- | :--- |
| **NFR-1** | レスポンス・低遅延 | 非同期エンコードワーカースレッド分離と最新フレーム間引きにより、高解像度でも入力遅延ゼロを実現すること。 | `TerminalBackend` 非同期ワーカー＆フレーム間引きにより達成 |
| **NFR-2** | 堅牢性・端末保護 | プログラムが異常終了しても、端末のRawモードやマウストラッキングが残留してシェルが壊れる事故を100%防止すること。 | `RawModeGuard::drop` の RAII パターンにより保証 |
| **NFR-3** | 依存関係の最小化 | C `libx11` / `Xvfb` / `openbox` への依存を排除し、Smithay + XWayland の単一バイナリとして動作すること。 | v0.6.0 にてレガシー Xvfb エンジンおよび `x11` クレート依存を完全排除 |
| **NFR-6** | SIMD 最適化 zlib エンジン (`zlib-ng`) | Kitty グラフィックスの RFC 1950 zlib 圧縮において SIMD 最適化エンジン（`flate2` / `zlib-rs`）を Level 2 動的ハフマン最適化によりデフォルトで提供し、高圧縮率と高速スループットを両立すること（`--no-default-features` 時に純粋 `miniz_oxide` へ安全フォールバック）。 | 1080p フルフレーム圧縮スループット +37.4% 高速化 (376MB/s→517MB/s)、通常 GUI 転送サイズ約 10% 削減、Noctty/Ghostty 端末を含む全端末での安定動作とフォールバック両立を検証 |
