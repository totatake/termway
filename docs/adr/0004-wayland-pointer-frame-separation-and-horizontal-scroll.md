# ADR-0004: Wayland ポインタフレーム分離と水平スクロール対応による GTK/Tauri 互換性確保

* **ステータス**: 承認済 (Accepted)
* **日付**: 2026-11-09
* **対象コンポーネント**: `src/compositor/input.rs` (`dispatch_mouse_event`, `find_pointer_focus`), `src/protocol/sgr_mouse.rs` (`SgrMouseParser`, `MouseButton`), `docs/SPECIFICATION.md`, `docs/ARCHITECTURE.md`

---

## 1. コンテキストと課題 (Context)

ユーザーより、macOS 上の Ghostty 1.4.0 や Windows 上の cnotty 端末から `ssh` 経由で Linux 上の `openidea`（`termway --scale 1.75 -- openidea`）を起動した際、マウス移動やクリック・キーボード入力は問題なく動作するにもかかわらず、「**ファイル内のマウスホイールスクロールが一切機能しない**」という不具合が報告された。

### 課題 1: GTK3 / WebKitGTK によるホイールイベントの 100% 破棄
* `openidea`（`l0ng-ai/openidea`）は Tauri v2（`tao` / `wry`）で構築されたエディタであり、Linux Wayland では GTK3 (`libgdk-3.so`) + WebKitGTK バックエンドで動作する。
* Wayland 仕様（`wayland.xml` `wl_pointer.frame` v5+）では、ポインタの論理イベントグループごとに `wl_pointer.frame` を送出することが規定されている（`motion, frame, button, frame, axis, frame`）。
* 従来の `termway` 実装（`src/compositor/input.rs`）では、ホイールイベント（`WheelUp` / `WheelDown`）受信時に `pointer.motion(...)` の直後に `pointer.frame(state)` を呼ばず、後続の `pointer.axis(...)` とまとめて 1 つの `wl_pointer.frame` で送出していた。
* GTK3 の Wayland バックエンド（`gdkdevice-wayland.c` の `gdk_wayland_seat_flush_frame_event`）を逆アセンブルおよびデバッグ解析した結果：
  1. `pointer_handle_motion` が呼ばれると、`GDK_MOTION_NOTIFY` イベントを生成して `seat->pointer_info.frame.event` に保持する。
  2. `pointer_handle_axis` は `frame.event` を flush せず、蓄積バッファ（`delta_x/y`）に加算するのみ。
  3. `wl_pointer.frame`（`gdk_wayland_seat_flush_frame_event`）が実行されると、冒頭で `if (frame->event != NULL)` を判定し、`GDK_MOTION_NOTIFY` を配送して直ちに `return`（早期リターン）する。
  4. その結果、同一フレームに `motion` が含まれている場合、後続のスクロールイベント（`GDK_SCROLL`）生成ブロックへ一切到達せず、蓄積されたデルタ値も破棄されていた。
  5. これにより、GTK3 / Tauri アプリケーションにおいてホイールスクロールが 100% ドロップする現象が発生していた。

### 課題 2: SGR 水平スクロールビットマスク不備と 2D 入力誤認識
* `src/protocol/sgr_mouse.rs` において、ホイール判定時に `raw_btn & 65`（`0b0100_0001`）でマスクしていた。
* SGR マウスプロトコル（DECSET 1006h / 1016h）では、ホイール方向は下位 2 ビット（`raw_btn & 3`）で表される：
  - `64` (`64 | 0`): Wheel Up
  - `65` (`64 | 1`): Wheel Down
  - `66` (`64 | 2`): Wheel Left（左水平スクロール）
  - `67` (`64 | 3`): Wheel Right（右水平スクロール）
* マスク値 `65` では bit 1（`2`）が削ぎ落とされるため、`66` が `64`（WheelUp）、`67` が `65`（WheelDown）に誤変換されていた。
* macOS の Ghostty や Windows の cnotty におけるトラックパッド（2本指操作）や Magic Mouse、チルトホイールマウスでは、垂直スクロール時に微小な水平スクロール（`66`/`67`）が混入するため、意図しない逆スクロールや相殺が発生していた。

### 課題 3: Wayland サブサーフェス（`wl_subsurface`）へのヒットテスト不足
* 従来の `find_pointer_focus` は管理ウィンドウに対してルートの `WlSurface` のみを返していた。
* WebKitGTK やブラウザエンジン等の複合 Wayland クライアントでは、ハードウェアアクセラレーション描画領域に子 `wl_subsurface` が配置される場合があり、子サーフェスへのポインタフォーカスおよび相対座標の解決が必要であった。

---

## 2. 決定事項 (Decision)

1. **`wl_pointer.motion` と `wl_pointer.axis` のフレーム完全分離**:
   - `src/compositor/input.rs` のホイール処理において、`pointer.motion` 送出直後に必ず `pointer.frame(state)` を発行する。
   - その後 `pointer.axis`（`AxisFrame`）を送出して再度 `pointer.frame(state)` を発行する。
   - これにより、Wayland 仕様および GTK3 の期待通り `motion, frame, axis, frame` の独立した論理フレームが形成され、GTK3 側で `GDK_SCROLL` が正常に発火する。
   - 同様に、通常のマウスクリック（Press / Release）時にも `pointer.motion` と `pointer.button` の間に `pointer.frame(state)` を発行し、一貫したプロトコル準拠を徹底する。

2. **SGR 水平スクロール（WheelLeft / WheelRight）対応**:
   - `src/protocol/sgr_mouse.rs` の `MouseButton` 列挙型に `WheelLeft` および `WheelRight` を追加。
   - マスク演算を `raw_btn & 67`（`0b0100_0011`）へ修正し、`66`（Left）および `67`（Right）を正しく識別。
   - コンポジタ側で `Axis::Horizontal` としてディスパッチし、水平スクロールをサポート。

3. **`under_from_surface_tree` によるサブサーフェスヒットテスト導入**:
   - `find_pointer_focus` 内で、XDG ポップアップと同様に `smithay::desktop::utils::under_from_surface_tree` を用いてウィンドウ内のサーフェスツリーを再帰探索する。
   - クライアントがサブサーフェス（WebKitGTK 等）を利用している場合でも、直下の正しい `wl_surface` とサーフェスローカル座標を正確に特定してフォーカスを渡す。

---

## 3. 結果と効果 (Consequences)

* **GTK3 / Tauri アプリ（openidea 等）でのスクロール完全動作**:
  GTK3 のフレーム早期リターン問題を解消し、ローカル実機およびヘッドレステスト環境において `ScrolledWindow` の `vadj` スクロールが 100% 正常動作することを確認。
* **macOS / Windows 端末での 2D トラックパッド追従性向上**:
  Ghostty (macOS) や cnotty (Windows) からの 2 本指スクロールおよびチルトホイール入力時に、水平・垂直スクロールが分離され、誤動作や反転のない滑らかな操作性を実現。
* **テスト網羅性の維持**:
  `tests/unit_sgr_mouse.rs` に左右スクロールのパース検証を追加し、`tests/unit_compositor.rs` に左右ホイールディスパッチおよび Wayland ポインタフレーム分離検証テストを追加。全 128 テストが 100% パス。
