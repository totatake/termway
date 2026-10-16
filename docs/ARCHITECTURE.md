# termway システムアーキテクチャ詳細設計書 (v0.7.0)

本書は、`termway` の**静的構造（モジュール構成・クラス/構造体関係・責務分割）**および**動的構造（起動シーケンス・非同期レンダリングパイプライン・ウィンドウ状態遷移・入力ディスパッチ）**について解説する技術設計ドキュメントです。

---

## 1. 静的構造 (Static Architecture)

### 1.1 レイヤー構成とディレクトリツリー

`termway` は単一バイナリの Smithay ベース自立型 Wayland コンポジタとして設計されており、以下の 5 つの疎結合なコアレイヤーで構成されています（v0.6.0 にてレガシー Xvfb エンジンを廃止し Smithay + XWayland アーキテクチャへ一本化、v0.7.0 にてコンポジタのモジュール分割とストリーム最適化を実施）。

```text
src/
├── main.rs                         # CLIパーサー、グラフィックスプロトコル自動判定、エントリポイント
├── lib.rs                          # クレート公開モジュール定義
├── backend/                        # 【描画バックエンド・非同期I/O層】
│   ├── mod.rs
│   └── terminal.rs                 # TerminalBackend: 非同期エンコードワーカースレッド & 最新フレーム間引き
├── compositor/                     # 【Smithay Wayland コンポジタ & WM コア層】
│   ├── mod.rs                      # TermwayCompositor: 統一 FD ポーリング、フレームペーシング、メインループ
│   ├── env.rs                      # XDG ランタイム環境設定、X11 ディスプレイ候補検索、シグナル処理、ログ出力
│   ├── taskbar.rs                  # Taskbar & TaskbarChip: 下部ステータスバー・タスクバーチップ生成とクリック判定
│   ├── profile.rs                  # CompositorMetrics: パイプライン各段レイテンシ集計、帯域測定、レポート出力
│   ├── screenshot.rs               # save_screenshot: フレームバッファからの PNG スクリーンショット出力
│   ├── state.rs                    # TermwayState: コンポジタ全体のオーケストレータ状態 & JSON 状態ダンプ
│   ├── window_manager.rs           # WindowManager: Z-order・フォーカス・ドラッグ・最小化/最大化・リサイズ再配置
│   ├── window.rs                   # ManagedWindow / WindowSurface / WindowDragState: ウィンドウジオメトリ・ヒットテスト
│   ├── handlers/                   # Smithay プロトコルハンドラ群（ドメイン別分割）
│   │   ├── mod.rs                  # delegate_*! マクロ集約
│   │   ├── compositor.rs           # CompositorHandler / BufferHandler / ShmHandler / OutputHandler
│   │   ├── xdg_shell.rs            # XdgShellHandler / XdgDecorationHandler (Toplevel・Popup・Configure制御)
│   │   └── seat.rs                 # SeatHandler / SelectionHandler / DataDeviceHandler (OSC 52 & クリップボード統合)
│   ├── xwayland.rs                 # XwmHandler / XWaylandShellHandler: X11 ウィンドウ管理 & Override-Redirect 制御
│   ├── input.rs                    # SGRマウス・ANSIキーイベントの座標変換、WM操作、Smithay Seat ディスパッチ
│   ├── renderer.rs                 # TermwayRenderer: Pixman オフスクリーン合成（パス統合）、ポップアップ・スプラッシュ描画
│   ├── decoration.rs               # Openbox/Fluxbox 風 SSD タイトルバー・ボタン・枠線描画
│   ├── bitmap_font.rs              # ビットマップフォント文字列レンダラ・グラデーション描画
│   └── font_data.rs                # 内蔵 8x8 ASCII ビットマップフォントデータ
├── display/                        # 【フレームバッファ・差分検出層】
│   ├── mod.rs
│   ├── damage.rs                   # DamageTracker: 32x32 タイル FNV-1a ハッシュ比較 (AVX2 SIMD 高速化)
│   └── framebuffer.rs              # Framebuffer: RGBA8888 バッファ & BGRX→RGBA 変換 (AVX2 SIMD 高速化)
├── protocol/                       # 【ターミナルプロトコル エンコード・デコード層】
│   ├── mod.rs
│   ├── kitty.rs                    # KittyEncoder & KittyStreamEncoder: RFC 1950 zlib 圧縮 (o=z) & ゼロアロケーション Base64 ストリーミング
│   ├── sixel.rs                    # SixelEncoder & SixelStreamEncoder: ダーティバンドキャッシュ、RGB332 減色、RLE 圧縮 (AVX2 + tzcnt)
│   ├── sgr_mouse.rs                # SgrMouseParser: xterm SGR 1006 (Any-event 1003h) マウスシーケンスパーサー
│   └── keyboard.rs                 # KeyboardParser: UTF-8・ANSI/CSI 修飾キー・Bracketed Paste パーサー
├── session/                        # 【セッション管理・IPC ブリッジ層】
│   ├── mod.rs                      # SessionManager: ハイブリッド D-Bus 管理、Snap シム生成、Polkit エージェント管理
│   └── secret_bridge.rs            # SecretServiceBridge: org.freedesktop.secrets D-Bus 中継ブリッジ
└── terminal/                       # 【端末制御・RAII 保護・解像度検出層】
    ├── mod.rs
    ├── raw.rs                      # RawModeGuard: termios Raw モード & Alternate Screen の RAII 復元保証
    ├── winsize.rs                  # TerminalSize: TIOCGWINSZ & SGR 14t/16t セルピクセル寸法測定・SIGWINCH 捕捉
    └── query.rs                    # GraphicsProtocol: 端末環境変数およびエスケープシーケンス応答によるプロトコル検出
```

---

### 1.2 パッケージ・モジュール依存関係図

上位の `TermwayCompositor` が各ドメインモジュールを統括し、下位の `protocol` / `display` / `terminal` は Wayland や Smithay に依存しない独立した純粋モジュールとして単体テスト可能に設計されています。

```mermaid
flowchart TB
    subgraph CLI["エントリポイント (src/main.rs)"]
        Main["main() / CLI Parser"]
    end

    subgraph CompositorLayer["コンポジタコア層 (src/compositor/)"]
        Comp["TermwayCompositor<br/>(mod.rs / env.rs / profile.rs / screenshot.rs)"]
        State["TermwayState<br/>(state.rs)"]
        WM["WindowManager & ManagedWindow<br/>(window_manager.rs / window.rs)"]
        Taskbar["Taskbar & TaskbarChip<br/>(taskbar.rs)"]
        Handlers["Protocol Handlers<br/>(handlers/{compositor,xdg_shell,seat}.rs)"]
        XWL["XWayland & X11Wm Handler<br/>(xwayland.rs)"]
        Input["Input Dispatcher<br/>(input.rs)"]
        Renderer["TermwayRenderer & Decoration<br/>(renderer.rs / decoration.rs / bitmap_font.rs)"]
    end

    subgraph SessionLayer["セッション・IPC層 (src/session/)"]
        Sess["SessionManager<br/>(Isolated & Ambient D-Bus / Polkit)"]
        Secret["SecretServiceBridge<br/>(secret_bridge.rs)"]
    end

    subgraph BackendLayer["描画バックエンド層 (src/backend/)"]
        Backend["TerminalBackend<br/>(Async Worker Thread)"]
    end

    subgraph DisplayLayer["フレームバッファ・差分層 (src/display/)"]
        Damage["DamageTracker<br/>(AVX2 Tile Hash)"]
        FB["Framebuffer<br/>(AVX2 BGRX->RGBA)"]
    end

    subgraph ProtocolLayer["プロトコル変換層 (src/protocol/)"]
        Kitty["KittyEncoder & KittyStreamEncoder<br/>(Zero-Alloc zlib + Base64)"]
        Sixel["SixelEncoder & SixelStreamEncoder<br/>(Dirty Band Cache + AVX2 RGB332 + RLE)"]
        MouseP["SgrMouseParser"]
        KeyP["KeyboardParser"]
    end

    subgraph TerminalLayer["端末制御層 (src/terminal/)"]
        Raw["RawModeGuard"]
        WinSize["TerminalSize & SIGWINCH"]
        Query["GraphicsProtocol Query"]
    end

    Main --> Query
    Main --> Comp
    Comp --> State
    Comp --> Renderer
    Comp --> Backend
    Comp --> Sess
    Comp --> Damage
    Comp --> Input
    Comp --> Taskbar
    Comp --> Raw
    Comp --> WinSize

    State --> WM
    State --> Handlers
    State --> XWL
    Input --> State
    Input --> WM
    Input --> Taskbar
    Input --> MouseP
    Input --> KeyP

    Sess --> Secret
    Backend --> Kitty
    Backend --> Sixel
    Backend --> FB
```

---

### 1.3 コア構造体クラス図 (Class / Ownership Diagram)

`v0.6.0`〜`v0.7.0` では、`TermwayState` に集中していたウィンドウ管理ロジックを [`WindowManager`](../src/compositor/window_manager.rs) (`state.wm`) として分離し、Smithay のトレイト実装を [`handlers/`](../src/compositor/handlers/mod.rs) 配下へ分割、さらにコンポジタオーケストレータの周辺責務を [`Taskbar`](../src/compositor/taskbar.rs)・[`CompositorMetrics`](../src/compositor/profile.rs)・[`env`](../src/compositor/env.rs)・[`screenshot`](../src/compositor/screenshot.rs) へ独立させています。

```mermaid
classDiagram
    class TermwayCompositor {
        -Display~TermwayState~ display
        -ListeningSocket listener
        -TermwayState state
        -TermwayRenderer renderer
        -TerminalBackend backend
        -DamageTracker damage_tracker
        -SessionManager session
        -Option~Child~ client_child
        -Taskbar taskbar
        -CompositorMetrics metrics
        +new(config: EngineConfig) Result~Self~
        +run() Result~()~
        -apply_terminal_size(new_size: TerminalSize)
        -process_terminal_input()
    }

    class Taskbar {
        +Vec~TaskbarChip~ chips
        +build(windows: &[ManagedWindow], cols: u16, has_opened: bool) Option~String~
        +chip_at_col(col: u16) Option~usize~
    }

    class CompositorMetrics {
        +bool enabled
        +u64 rendered_frames
        +u64 skipped_frames
        +record_render(dur_us: u64, damaged_tiles: usize, bytes: usize)
        +maybe_report_and_reset()
    }

    class TermwayState {
        +CompositorState compositor_state
        +XdgShellState xdg_shell_state
        +XdgDecorationState xdg_decoration_state
        +ShmState shm_state
        +SeatState~Self~ seat_state
        +DataDeviceState data_device_state
        +PopupManager popups
        +WindowManager wm
        +Option~X11Wm~ xwm
        +Seat~Self~ seat
        +KeyboardHandle~Self~ keyboard
        +PointerHandle~Self~ pointer
        +u32 width
        +u32 height
        +bool dirty
        +bool needs_clear
        +bool running
        +new(dh: &DisplayHandle, width: u32, height: u32, enable_wm: bool) Self
        +raise_window(window_id: usize)
        +minimize_window(window_id: usize)
        +unminimize_window(window_id: usize)
        +dump_state_json() String
    }

    class WindowManager {
        +Vec~ManagedWindow~ windows
        +WindowDragState drag_state
        +Option~(usize, Instant)~ last_titlebar_click
        -usize next_id
        +alloc_id() usize
        +get(id: usize) Option~&ManagedWindow~
        +get_mut(id: usize) Option~&mut ManagedWindow~
        +bring_to_top(id: usize) bool
        +set_minimized(id: usize, minimized: bool) bool
        +top_visible_id() Option~usize~
        +cascade_offset() i32
        +resize_windows(new_w: u32, new_h: u32)
    }

    class ManagedWindow {
        +usize id
        +WindowSurface surface
        +String title
        +Point~i32, Logical~ loc
        +Size~i32, Logical~ size
        +bool active
        +bool has_ssd
        +bool is_maximized
        +bool is_minimized
        +Option~(Point, Size)~ saved_geometry
        +toggle_maximize(max_w: u32, max_h: u32)
        +update_maximized_size(max_w: u32, max_h: u32)
        +frame_geometry() Rectangle
        +titlebar_geometry() Rectangle
        +close_button_geometry() Rectangle
        +maximize_button_geometry() Rectangle
        +minimize_button_geometry() Rectangle
    }

    class WindowSurface {
        <<enumeration>>
        Wayland(ToplevelSurface)
        X11(X11Surface)
        Dummy
    }

    class WindowDragState {
        <<enumeration>>
        None
        Moving(window_id, start_mouse, start_win_loc)
        Resizing(window_id, start_mouse, start_win_size)
    }

    class TermwayRenderer {
        -PixmanRenderer renderer
        -PixmanRenderBuffer buffer
        -u32 width
        -u32 height
        +render(state: &TermwayState) Result~&[u8]~
        +resize(width: u32, height: u32) Result~()~
    }

    class TerminalBackend {
        +TerminalSize term_size
        -bool use_sixel
        +bool show_hud
        +bool needs_clear
        -Arc~(Mutex~AsyncRenderState~, Condvar)~ async_state
        -Option~JoinHandle~ worker_handle
        +render_pixels(pixels: &[u8], w: u32, h: u32, taskbar: Option~&str~)
        +resize(term_size: TerminalSize)
    }

    class DamageTracker {
        -u32 width
        -u32 height
        -u32 tile_size
        -Vec~u64~ prev_hashes
        +track_damage(rgba_pixels: &[u8]) Vec~DamageRect~
        +reset()
        +resize(width: u32, height: u32)
    }

    class SessionManager {
        -Option~Child~ dbus_child
        -Option~String~ isolated_dbus_address
        -Option~String~ ambient_dbus_address
        -Option~SecretServiceBridge~ secret_bridge
        -Option~Child~ polkit_child
        +dbus_address_for_client(cmd: &str) Option~&str~
        +start_polkit(wayland_socket: Option~&str~, x11_display: Option~&str~)
    }

    TermwayCompositor *-- TermwayState
    TermwayCompositor *-- TermwayRenderer
    TermwayCompositor *-- TerminalBackend
    TermwayCompositor *-- DamageTracker
    TermwayCompositor *-- SessionManager
    TermwayCompositor *-- Taskbar
    TermwayCompositor *-- CompositorMetrics
    TermwayState *-- WindowManager
    WindowManager *-- "0..*" ManagedWindow
    WindowManager *-- WindowDragState
    ManagedWindow *-- WindowSurface
```

---

## 2. 動的構造 (Dynamic Architecture & Runtime Behavior)

### 2.1 起動・セッション確立・スプラッシュ描画シーケンス

`termway` 起動時からクライアント GUI アプリケーションが最初のウィンドウをマップするまでの動的シーケンスです。

```mermaid
sequenceDiagram
    autonumber
    actor User as ユーザー / Terminal
    participant Main as main.rs
    participant Term as TerminalLayer<br/>(RawModeGuard / SGR 14t)
    participant Comp as TermwayCompositor
    participant XWL as XWayland / Calloop
    participant Sess as SessionManager<br/>(D-Bus / Polkit)
    participant App as Client GUI App

    User->>Main: termway [OPTIONS] -- <COMMAND>
    Main->>Term: setup_sigwinch() & query_graphics_protocol()
    Term-->>Main: GraphicsProtocol (Kitty or Sixel)
    Main->>Comp: TermwayCompositor::new(config)
    Comp->>Term: TerminalSize::query() (TIOCGWINSZ + \x1b[16t / \x1b[14t)
    Term-->>Comp: cols, rows, cell_width_px, cell_height_px
    Comp->>Comp: Wayland ソケット生成 (wayland-N)
    Comp->>Sess: SessionManager::new()<br/>(隔離 D-Bus 起動 + SecretServiceBridge + Snap シム生成)
    Comp->>XWL: XWayland::spawn() (/tmp/termway_wrap 経由)

    Main->>Comp: compositor.run()
    Comp->>Term: RawModeGuard::enter()<br/>(Raw mode, Alternate Screen \x1b[?1049h, Mouse \x1b[?1003h\x1b[?1006h)
    XWL-->>Comp: XWaylandEvent::Ready (:DISPLAY)
    Comp->>XWL: X11Wm::start_wm() & X11 shape 接続

    loop 最初のウィンドウがマップされるまで (最大4秒)
        Comp->>Comp: TermwayRenderer::render_splash()<br/>(グラデーション ">_ TERMWAY" + カーネル風テレメトリログ)
    end

    Comp->>Sess: update_activation_environment() & start_polkit()
    Comp->>App: Command::spawn()<br/>(WAYLAND_DISPLAY, DISPLAY, DBUS_SESSION_BUS_ADDRESS)
    App->>Comp: wl_surface.commit / X11 MapRequest
    Comp->>Comp: state.show_splash = false<br/>state.needs_clear = true (全画面クリア後 GUI 表示へ移行)
```

---

### 2.2 イベント駆動メインループと非同期レンダリングパイプライン

`TermwayCompositor::run` のメインスレッド（統一 FD ポーリング + Calloop + Wayland ディスパッチ）と、`TerminalBackend` の専用ワーカースレッド（`termway-async-render`）間の非同期協調動作を示します。

- **統一 FD ポーリングとフレームペーシング (`--fps`)**: `stdin`・Wayland `ListeningSocket`・`display.backend().poll_fd()` の 3 記述子を単一の `nix::poll` で一括監視することでアイドル時の CPU 使用率を 0% に保ちつつ、`--fps`（デフォルト 30fps）のフレーム間隔に基づいて連続コミット時の再描画頻度を適応的に制御します。
- **入力とイベント処理の非ブロッキング化**: 画像エンコード（Sixel 減色や Kitty zlib 圧縮）と標準出力フラッシュは別スレッドで実行されるため、高解像度描画中でもマウス・キー入力は遅延なく処理されます。
- **最新フレーム間引き (Frame Dropping) とゼロアロケーション再利用**: ワーカースレッドが前フレームをエンコード中に次のフレームが到着した場合、`pending_job` のピクセルバッファを上書き置換することで古いフレームを破棄し、PTY バッファブロートを完全に防ぎます。また、描画完了後の `Vec<u8>` は `spare_buffer` としてメインスレッドへ返却・再利用されます。

```mermaid
sequenceDiagram
    autonumber
    participant TermIn as Terminal stdin &<br/>Wayland FDs
    participant Main as Main Thread<br/>(TermwayCompositor)
    participant State as TermwayState & WM
    participant Pixman as TermwayRenderer &<br/>DamageTracker (AVX2)
    participant Queue as AsyncRenderState<br/>(Mutex + Condvar)
    participant Worker as Worker Thread<br/>(termway-async-render)
    participant TermOut as Terminal stdout

    loop イベントループ (統一 nix::poll + --fps フレームペーシング)
        Main->>State: event_loop.dispatch(0ms) (XWayland / X11Wm イベント処理)
        opt SIGWINCH 受信時
            Main->>Main: apply_terminal_size()<br/>(Renderer / DamageTracker / WM.resize_windows)
        end

        Main->>TermIn: nix::poll([stdin, listener, display_fd], poll_timeout_ms)
        opt 端末入力あり (SGR Mouse / Key / Bracketed Paste / OSC 52)
            TermIn-->>Main: Raw bytes
            Main->>State: process_terminal_input()<br/>(SgrMouseParser / KeyboardParser -> Seat / WM 操作)
        end

        Main->>State: display.dispatch_clients() & flush_clients()
        Note over State: クライアントの commit / メニュー操作 / 移動により<br/>state.dirty = true がセットされる

        opt state.dirty == true && frame_interval 経過
            Main->>Pixman: renderer.render(&state) [単一パス合成]
            Pixman-->>Main: RGBA8888 pixels (&[u8])
            Main->>Pixman: damage_tracker.track_damage(pixels) [AVX2 SIMD]
            Pixman-->>Main: Vec<DamageRect>
            opt !damage.is_empty() || taskbar_changed || needs_clear
                Main->>Queue: render_pixels(pixels, w, h, taskbar)<br/>(pending_job 更新・古い未処理フレームは上書き破棄)
                Queue->>Worker: cvar.notify_one()
            end
            Main->>State: send_frames_surface_tree(time)<br/>state.dirty = false
        end
    end

    par バックグラウンド非同期エンコード
        Worker->>Queue: pending_job.take()
        alt Sixel モード
            Worker->>Worker: SixelStreamEncoder::encode_frame_into()<br/>(6行ダーティバンド FNV-1a キャッシュ + AVX2 RGB332 減色 + tzcnt RLE)
        else Kitty モード
            Worker->>Worker: KittyStreamEncoder::encode_fullscreen_frame_into()<br/>(ゼロアロケーション CompressorOxide 再利用 + Base64 4096B ストリーミング)
        end
        Worker->>TermOut: Synchronized Output (\x1b[?2026h ... 画像 + タスクバー + HUD ... \x1b[?2026l)
        Worker->>Queue: spare_buffer = Some(job.pixels) (バッファ返却・再利用)
    end
```

---

### 2.3 ウィンドウライフサイクルと状態遷移 (Window State Machine)

内蔵ウィンドウマネージャ（[`WindowManager`](../src/compositor/window_manager.rs) / [`ManagedWindow`](../src/compositor/window.rs)）における各ウィンドウの状態遷移図です。

- **初期配置**: プライマリウィンドウおよび独立アプリウィンドウは、ターミナルの限られた表示領域を最大限活かすため初期状態で**最大化 (`Maximized`)** され、同時に画面解像度の **80%・中央配置** の矩形が `saved_geometry` として保存されます（ダイアログや子ウィンドウはクライアント指定サイズの `Floating` で開きます）。
- **最大化解除 (`Unmaximize`)**: タイトルバーの `[□]` ボタンクリックまたはタイトルバーのダブルクリックにより、保存された 80% 中央サイズへ復元されます。Wayland クライアントが次フレームをコミットするまでの間、`unmaximize_pending` フラグにより古いフルスクリーンバッファのはみ出し描画を抑止し、背景をクリーンに保ちます。
- **最小化 (`Minimized`)**: `[-]` ボタンまたは最下部タスクバーチップのクリックにより非表示化され、レンダリング対象から除外されるとともに、背後の可視ウィンドウへ自動的にキーボードフォーカスが移譲されます。

```mermaid
stateDiagram-v2
    [*] --> Maximized: プライマリ / 独立ウィンドウ生成<br/>(saved_geometry = 画面の80%中央)
    [*] --> Floating: 子ダイアログ / ツールウィンドウ生成

    state Maximized {
        [*] --> FullscreenActive: loc=(0, 23), size=(W, H-23)<br/>is_maximized = true
        FullscreenActive --> FullscreenResized: SIGWINCH 端末リサイズ<br/>(update_maximized_size で新解像度へ即時追従 &<br/>saved_geometry も新解像度の80%中央に更新)
        FullscreenResized --> FullscreenActive
    }

    state Floating {
        [*] --> NormalIdle: WindowDragState::None
        NormalIdle --> Moving: タイトルバー左ドラッグ開始
        Moving --> NormalIdle: マウスボタン解放<br/>(画面境界クランプ適用)
        NormalIdle --> Resizing: 右下ハンドル左ドラッグ開始
        Resizing --> NormalIdle: マウスボタン解放<br/>(最小 160x100 / 画面境界クランプ)
    }

    Maximized --> Floating: [□] ボタン or タイトルバーWクリック<br/>(toggle_maximize: 80%中央へ復元,<br/>unmaximize_pending = true, needs_clear = true)
    Floating --> Maximized: [□] ボタン or タイトルバーWクリック<br/>(現在位置・サイズを saved_geometry に保存し最大化)

    Maximized --> Minimized: [-] ボタン or アクティブ時タスクバーチップクリック<br/>(is_minimized = true, 次の可視ウィンドウへフォーカス移譲)
    Floating --> Minimized: [-] ボタン or アクティブ時タスクバーチップクリック<br/>(is_minimized = true, 次の可視ウィンドウへフォーカス移譲)

    Minimized --> Maximized: タスクバーチップ [id: title _] クリック<br/>(最小化前に Maximized だった場合、最前面へ復元)
    Minimized --> Floating: タスクバーチップ [id: title _] クリック<br/>(最小化前に Floating だった場合、最前面へ復元)

    Maximized --> [*]: [×] ボタン (send_close) / アプリ終了
    Floating --> [*]: [×] ボタン (send_close) / アプリ終了
    Minimized --> [*]: アプリ終了
```

---

### 2.4 マウスイベントのヒットテスト・ルーティング優先順位

ターミナルから受信した SGR マウスイベントは、`src/compositor/input.rs` の `dispatch_mouse_event` において以下の優先順位（Z-order の手前側優先）でルーティングされます。

```mermaid
flowchart TD
    MouseEv["SGR 1006 マウスイベント受信<br/>(col, row, button, action)"] --> CheckTaskbar{"1. 最下行 (row == term_rows)<br/>タスクバーチップ上の左クリック？"}

    CheckTaskbar -->|"Yes"| TaskbarAction["対象ウィンドウが Minimized なら Unminimize & Raise<br/>Active なら Minimize<br/>非Active なら Raise & Focus"]
    CheckTaskbar -->|"No"| MapCoord["セル座標 (col, row) を<br/>コンポジタピクセル座標 (px, py) へ変換"]

    MapCoord --> CheckDrag{"2. ドラッグ中？<br/>(WindowDragState::Moving / Resizing)"}
    CheckDrag -->|"Yes (Drag)"| UpdateDrag["ウィンドウ位置 / サイズ更新<br/>(画面境界クランプ + Wayland/X11 Configure)"]
    CheckDrag -->|"Yes (Release)"| EndDrag["WindowDragState::None へリセット"]
    CheckDrag -->|"No"| CheckPopup{"3. Wayland XDG Popup または<br/>X11 Override-Redirect メニュー上？"}

    CheckPopup -->|"Yes"| RoutePopup["最前面ポップアップ / メニューサーフェスへ<br/>pointer.motion / pointer.button を直接転送"]
    CheckPopup -->|"No (ポップアップ表示中の外部クリック)"| DismissPopup["PopupManager::dismiss_popup()<br/>& X11 メニューフォーカス解除"]

    DismissPopup --> CheckWM
    CheckPopup -->|"No"| CheckWM{"4. 可視 ManagedWindow<br/>(Z-order 逆順: 最前面から検査)"}

    CheckWM -->|"ヒット"| RaiseWin["raise_window(id)<br/>& キーボード / DataDevice フォーカス更新"]
    RaiseWin --> CheckButtons{"タイトルバー / 装飾領域？"}
    CheckButtons -->|"[×] 閉じるボタン"| CloseAct["Wayland send_close() / X11 close()"]
    CheckButtons -->|"[□] 最大化ボタン or Wクリック"| MaxAct["toggle_maximize() & Configure"]
    CheckButtons -->|"[-] 最小化ボタン"| MinAct["minimize_window(id)"]
    CheckButtons -->|"タイトルバー本体"| StartMove["WindowDragState::Moving 開始"]
    CheckButtons -->|"右下リサイズハンドル"| StartResize["WindowDragState::Resizing 開始"]
    CheckButtons -->|"クライアント描画領域"| ForwardClient["クライアント相対座標 (sx, sy) を算出し<br/>Smithay PointerHandle (motion / button / axis) へ転送"]
```

---

### 2.5 セッション終了判定ステートマシン (Forking GUI App Lifecycle)

`gvim` や `gnome-terminal` のように起動直後に親プロセスが `fork()` して `exit(0)` するアプリケーションや、スプラッシュウィンドウを閉じてからメインウィンドウを開くアプリケーションに対応するため、`TermwayCompositor` はプロセス状態とウィンドウ枚数の組み合わせで終了判定を行います。

```mermaid
stateDiagram-v2
    [*] --> WaitingFirstWindow: クライアントプロセス起動<br/>(has_opened_windows = false)

    WaitingFirstWindow --> ActiveSession: ウィンドウが1枚以上オープン<br/>(has_opened_windows = true)
    WaitingFirstWindow --> StartupGracePeriod: 親プロセスが exit(0) で終了<br/>(gvim / ランチャー等の fork 想定)
    WaitingFirstWindow --> ErrorExit: 親プロセスが非ゼロ終了コードで異常終了<br/>(stderr ログを抽出してエラー表示)

    StartupGracePeriod --> ActiveSession: 6秒以内に子プロセスがウィンドウをオープン
    StartupGracePeriod --> ErrorExit: 6秒経過してもウィンドウが1枚も開かない

    ActiveSession --> CloseGracePeriod: 全ウィンドウがクローズ (active_window_count == 0)<br/>かつ子プロセスがまだ生存中
    ActiveSession --> CleanExit: 全ウィンドウがクローズ (active_window_count == 0)<br/>かつ子プロセスも終了済み

    CloseGracePeriod --> ActiveSession: 3秒以内に次のウィンドウがオープン<br/>(ダイアログ→メイン画面遷移など)
    CloseGracePeriod --> CleanExit: 3秒経過してもウィンドウが0枚のまま

    ActiveSession --> CleanExit: ユーザーが Ctrl-Q を押下
    CleanExit --> [*]: RawModeGuard::drop() で端末復元
    ErrorExit --> [*]: RawModeGuard::drop() で端末復元
```

---

## 3. 主要サブシステムの技術詳細

### 3.1 プロトコルエンコードと SIMD 高速化 (`src/protocol/`, `src/display/`)

1. **Kitty Graphics Protocol (`src/protocol/kitty.rs`)**:
   - **RFC 1950 zlib 圧縮 (`o=z`) & ゼロアロケーション再利用 (`KittyStreamEncoder`)**: RGBA8888 ピクセルデータを `miniz_oxide` の `CompressorOxide` インスタンス再利用（`reset()`）によりヒープ確保なしで圧縮し、中間文字列を介さず直接 Base64 4096 バイトチャンクへストリーミング出力（`a=T,f=32,o=z,i=1,q=2,C=1,z=-2147483648`）。
   - **最背面レイヤ配置 (`z=-2147483648`) と最下行保護 (`rows - 1`)**: 画像をテキストレイヤより背面に配置し、さらに描画行数を `term_rows - 1` に制限することで、最下行のタスクバーおよびステータスオーバーレイ文字列との重なりを防止。
2. **Sixel Graphics Protocol (`src/protocol/sixel.rs`)**:
   - **6 行ダーティバンドキャッシュ (`SixelStreamEncoder`)**: 6 ピクセル高の水平バンドごとに FNV-1a ハッシュとエンコード済み Sixel バイト列をキャッシュし、前フレームと同一のバンドは減色・走査を完全スキップしてキャッシュから即時結合（カーソル移動等の部分更新を約 4.3ms → 0.34ms へ 92〜95% 高速化）。
   - **AVX2 SIMD RGB332 減色 (`quantize_rgb332_avx2`)**: 8 ピクセル（32 バイト）単位で RGBA から 8-bit RGB332 カラーインデックスへ一括ベクトル変換。
   - **AVX2 + `tzcnt` ビットマスク走査**: 6 ピクセル高のバンドごとに 32 列単位で `_mm256_cmpeq_epi8` + `_mm256_movemask_epi8` を実行し、存在する色だけを `trailing_zeros()` でジャンプ走査して RLE 出力（1080x720 フルフレーム初回を約 3.7ms でエンコード）。
3. **タイル差分検出 (`src/display/damage.rs`)**:
   - 画面を 32x32 ピクセルのタイルに分割し、`x86_64` AVX2 環境では `_mm256_mullo_epi32` と `_mm256_xor_si256` による 8 レーン並列ハッシュで前回フレームと比較（1080x720 画面の差分スキャン約 0.14〜0.20ms）。
4. **Synchronized Output (`\x1b[?2026h` ... `\x1b[?2026l`)**:
   - フレーム画像・タスクバー・HUD の出力全体を DEC Mode 2026（同期更新ブロック）で囲み、ティアリングや画面のチラつきを防止。

---

### 3.2 双方向クリップボード同期 (`src/compositor/handlers/seat.rs`, `src/compositor/input.rs`)

- **GUI → ホスト端末 (Copy)**:
  - Wayland クライアントが `wl_data_device.set_selection` を発行、または XWayland クライアントが X11 `CLIPBOARD` / `PRIMARY` セレクションを更新した際、`SelectionHandler::new_selection` が発火。
  - パイプ経由で UTF-8 テキストを読み取り、Base64 エンコードして OSC 52 シーケンス（`\x1b]52;c;<base64>\x07`）を端末へ送信し、ホスト OS のクリップボードへ即時同期。
- **ホスト端末 → GUI (Paste)**:
  - 端末側でのペースト（Bracketed Paste `\x1b[200~...\x1b[201~`）または `Ctrl+V` 押下時、ホスト側のテキストをコンポジタ側セレクション（`set_data_device_selection` / XWM セレクション）へセットした上で、フォーカス中のクライアントへ貼り付けキーイベントまたは Wayland データパイプを供給。

---

### 3.3 ハイブリッド D-Bus セッションと Secret Service 中継 (`src/session/`)

- **デュアル D-Bus ルーティング**:
  - `gnome-terminal` などの単一インスタンス型アプリがホストの既存デスクトップ（`:0`）へ吸い込まれるのを防ぐため、専用 `dbus-daemon --session` による隔離バスを生成。
  - 一方、Ubuntu の Snap 版 `firefox` や `xdg-open` など、ホストの `systemd --user` による cgroup スコープ生成（`snap-confine`）を要求するアプリには、起動コマンド判定および `PATH` ラッパーシムによってホストの Ambient バスを自動ルーティング（詳細は [ADR-0001](adr/0001-hybrid-dbus-session-routing.md) を参照）。
- **Secret Service ブリッジ (`src/session/secret_bridge.rs`)**:
  - 隔離 D-Bus セッション内で動く VS Code や Chromium 等がホストの GNOME Keyring / KWallet にアクセスできるよう、隔離バス上の `org.freedesktop.secrets` メソッド呼び出し・シグナルをホストの Ambient バスへ双方向中継。
