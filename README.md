# termway: The Ultimate X11-to-Terminal GUI Bridge

ターミナル（端末エミュレータ）の画面内に、X11 GUI アプリケーションを直接埋め込んで実行・操作できる高速ブリッジツールです。

---

## 主な特徴

- **ターミナル画面内でのGUI実行**:
  - **Kitty Graphics Protocol** を使用し、端末セル内に高精細なピクセル画像としてインライン描画。
  - SSH の単一ポート（PTY 1本）のみが通る踏み台環境やクラウド、tmux セッション内でも、追加のポートフォワーディングやクライアント側XサーバーなしでGUIツールが利用可能。
- **フルマウス＆キーボード対話 (ホバー対応)**:
  - **xterm SGR 1006 マウスプロトコル (Any-event 1003h)** により、クリック、ドラッグ、ホイールスクロールに加えてマウスカーソルの移動（ホバー追従）を完全検知。
  - 端末文字セル座標を仮想ディスプレイのピクセル解像度へ正確にマッピングし、**XTest** 拡張経由でX11クライアントに合成注入。
  - 矢印キー、Ctrlキー、Enter/Tab/Backspace 等のキー入力もノンブロッキングで即時転送（1キー遅延なし）。
- **タイル差分レンダリング (Damage Tracking)**:
  - 画面をタイル（32x32ピクセル等）に分割し、高速ハッシュ比較により変化があった矩形領域のみを端末へ差分転送。帯域とターミナル負荷を大幅に削減。
- **適合性検証＆堅牢な端末保護 (RAII Guard)**:
  - 起動時に Kitty Graphics Protocol の対応可否を自動検証し、非対応環境では親切なエラーメッセージを出力。
  - `RawModeGuard` の `Drop` 実装により、プログラムが強制終了（Ctrl+C 等）やパニックを起こしても、端末のマウス追跡解除・カーソル表示・画面復元を100%保証。

---

## アーキテクチャ概要

```mermaid
flowchart LR
    subgraph ClientTerminal["Client Terminal (Kitty, WezTerm, Ghostty)"]
        TermIn["stdin (SGR 1006 / 1003h Mouse & Keys)"]
        TermOut["stdout (Kitty Graphics Protocol)"]
    end

    subgraph termway["termway Core"]
        subgraph TerminalLayer["Terminal & Safety"]
            RawGuard["RawModeGuard (RAII Drop)"]
            WinSize["TIOCGWINSZ / Cell-to-Pixel"]
            Query["Kitty Protocol Capability Query"]
        end

        subgraph EngineLayer["Event Engine"]
            TermBackend["TerminalBackend (Non-blocking I/O)"]
        end

        subgraph DisplayInput["Display & Input Bridge"]
            Damage["Tile Damage Tracker"]
            XTest["XTest Event Injector"]
        end
    end

    subgraph X11["X11 Virtual Display"]
        Xvfb["Xvfb (:99)"]
        App["GUI Application (Tkinter, xeyes, xclock, GTK, Qt, etc.)"]
    end

    TermIn --> TermBackend
    TermBackend --> WinSize
    WinSize --> XTest
    XTest -->|"Fake Events (Keys / Mouse)"| Xvfb
    Xvfb <-->|"Render / Events"| App
    Xvfb -->|"XImage"| Damage
    Damage --> TermBackend
    TermBackend -->|"\\e_Ga=T,q=2...\\e\\"| TermOut
```

---

## 必要条件・環境

### クライアント端末
- **Kitty Graphics Protocol** 対応ターミナル:
  - [Kitty](https://sw.kovidgoyal.net/kitty/)
  - [WezTerm](https://wezfurlong.org/wezterm/)
  - [Ghostty](https://ghostty.org/)
  - [Foot](https://codeberg.org/dnkl/foot) 等

### サーバー / 実行環境
- Linux (x86_64)
- `Xvfb` (仮想フレームバッファXサーバー)
- X11 ライブラリ (`libX11`, `libXtst`)
- Rust ツールチェーン (1.70 以上)

---

## インストールとビルド

```bash
# リポジトリのクローン
git clone <repository-url>
cd termway

# ビルド
cargo build --release
```

バイナリは `target/release/termway` に生成されます。

---

## 使い方

### 1. 内蔵デモGUIの起動
```bash
cargo run -- --demo
```
ボタンクリックでカウンタが増加し、テキストボックスにキー入力ができる Tkinter GUI がターミナル内に起動します。
終了するには `Ctrl+C` を押すか、GUIウィンドウを閉じます。

### 2. 任意のX11アプリケーションの実行
`--` の後に実行したいコマンドと引数を指定します：

```bash
# マウス追従アプリ（ホバー動作確認）
cargo run -- -- xeyes

# 時計アプリ
cargo run -- -- xclock

# Python GUI スクリプトの実行
cargo run -- -- python3 my_gui_app.py

# エディタや解析ツール
cargo run -- -- gedit
cargo run -- -- wireshark
```

### 3. 主なコマンドラインオプション

| オプション | 引数 | 説明 | デフォルト |
| :--- | :--- | :--- | :--- |
| `--width` | `<PIXELS>` | 仮想ディスプレイ解像度の幅 | 端末サイズから自動算出 |
| `--height` | `<PIXELS>` | 仮想ディスプレイ解像度の高さ | 端末サイズから自動算出 |
| `--fps` | `<FPS>` | 目標フレームレート | `30` |
| `--demo` | - | 内蔵 Tkinter デモアプリを起動 | - |
| `-h`, `--help` | - | ヘルプメッセージを表示 | - |

---

## テスト実行 (V字開発モデル検証)

本プロジェクトは要件定義からシステム受入テストまでの V字開発モデル に基づいて開発されており、単体テスト・結合テスト・E2Eシステムテストが整備されています。

```bash
# 全テスト (単体・結合・E2E: 計24テスト) の実行
cargo test

# システムE2Eテストのみ実行 (Xvfb + 実Tkinter GUIの連携検証)
cargo test --test system_e2e_gui -- --nocapture
```

詳細なテスト仕様と要件マッピングについては [docs/V_MODEL_SPEC.md](docs/V_MODEL_SPEC.md) を参照してください。

---

## 関連ドキュメント

- [システムアーキテクチャ詳細設計書 (docs/ARCHITECTURE.md)](docs/ARCHITECTURE.md)
- [V字開発 要件定義書 & テスト仕様書 (docs/V_MODEL_SPEC.md)](docs/V_MODEL_SPEC.md)
