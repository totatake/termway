# ADR-0001: ハイブリッド D-Bus セッションルーティングと Snap アプリケーション対応

* **ステータス**: 承認済 (Accepted)
* **日付**: 2026-10-09
* **対象コンポーネント**: `src/session/mod.rs` (`SessionManager`, `is_snap_command`), `src/engine.rs`, `src/compositor/mod.rs`

---

## 1. コンテキストと課題 (Context)

`termway` は、PTY ターミナル内部で Wayland / X11 GUI アプリケーションを直接起動・表示するヘッドレスコンポジタブリッジです。クライアントアプリケーションをターミナル内で正しく動作させるにあたり、Linux の IPC 機構である D-Bus セッションバスとの統合において、互いに相反する2つの技術的課題が存在しました。

### 課題 1: 単一インスタンス型アプリのホスト画面吸い込まれ問題
`gnome-terminal`、`gedit`、`evince` などのデスクトップアプリケーションは、D-Bus アクティベーションまたは単一インスタンス機構（GApplication / D-Bus IPC）を採用しています。
* ホスト OS の環境変数 `DBUS_SESSION_BUS_ADDRESS` をそのまま共有した場合、すでにホストデスクトップ上でそのアプリ（またはバックグラウンドサーバー）が稼働していると、新規起動コマンドは既存のホストプロセスに「新しいウィンドウ／タブを開く」IPC リクエストを委譲します。
* その結果、クライアントプロセスは Termway のディスプレイ（`WAYLAND_DISPLAY` / `DISPLAY`）ではなく、**ホスト OS 側のデスクトップ画面にウィンドウを開いてしまい、ターミナル内には何も表示されない**現象が発生します。
* これを防ぐには、Termway 専用に隔離された新しい D-Bus セッション（`dbus-daemon --session`）を提供し、ホスト側の既存デーモンを見えなくする必要があります。

### 課題 2: Snap アプリケーション（Ubuntu 標準 Firefox 等）の cgroup 登録要件
Ubuntu 22.04 / 24.04 等のモダンな Linux ディストリビューションでは、`firefox` や `chromium` などの主要なブラウザが Snap パッケージとして配布されています。
* Snap アプリケーションは、セキュリティサンドボックスを構成する `snap-confine` ラッパー経由で起動されます。
* `snap-confine` はリソース制御とサンドボックス境界の構築のため、ホストの `systemd --user` セッションバス（`/run/user/<uid>/bus`）に対して D-Bus 経由で一時的な cgroup スコープ（`session-XX.scope`）の作成・登録を要求します。
* **もし Termway が D-Bus をすべてのアプリで完全に隔離してしまうと**、その孤立した D-Bus 上には `systemd` が存在しないため、以下のエラーを出力して 100% 起動に失敗（クラッシュ）します：
  ```text
  /user.slice/user-1000.slice/session-XX.scope is not a snap cgroup for tag snap.firefox.firefox
  ```

### 課題 3: URL ランチャー（`xdg-open` 等）やラッパースクリプト経由での起動
* `/usr/bin/firefox` 自体が単なる ELF バイナリではなく、内部で `exec /snap/bin/firefox "$@"` を実行するシェルスクリプトである場合があります。
* また、`xdg-open https://example.com` のように URL を開くコマンドを実行した場合、親コマンド自体は非 Snap（シェルスクリプトやシステムツール）ですが、実際に呼び出されるデフォルトハンドラが Snap 版ブラウザになります。親コマンドに隔離 D-Bus を渡してしまうと、子プロセスの Snap アプリがそれを引き継いでしまい、同様の cgroup エラーが発生します。

---

## 2. 決定事項 (Decision)

Termway では、**「ハイブリッド D-Bus セッションルーティング（ヒューリスティック・ホワイトリスト判定）」** を採用します。

1. **隔離 D-Bus セッション (`DbusSession`) の常駐**:
   * Termway 起動時に専用の `dbus-daemon --session --print-address` をバックグラウンドで起動し、Termway 独自の D-Bus アドレスを保持する。
2. **ホスト D-Bus アドレス (`ambient_dbus_address`) の捕捉**:
   * Termway 起動時に、ホストの環境変数 `DBUS_SESSION_BUS_ADDRESS` または `/run/user/<uid>/bus` からホストの `systemd --user` セッションバスのアドレスを捕捉・保持する。
3. **コマンド種別に応じた動的ルーティング (`SessionManager::dbus_address_for_client`)**:
   * **Snap 関連アプリおよび URL ランチャー**:
     * コマンド名、絶対パス、シンボリックリンク先、およびスクリプトファイル内の記述（`/snap/bin/`, `snap run`, `/usr/bin/snap`）を検査。
     * Snap アプリケーション、またはそれを起動する代表的ランチャー（`xdg-open`, `sensible-browser`, `gio`, `gnome-open`）と判定された場合、**ホストの Ambient D-Bus** を環境変数 `DBUS_SESSION_BUS_ADDRESS` として注入する。
   * **その他の一般アプリケーション**:
     * `gnome-terminal` を含む上記以外のすべてのコマンドには、**Termway 隔離 D-Bus** を注入する。

---

## 3. 検討した代替案 (Alternatives Considered)

### 代替案 A: 全面 D-Bus 隔離 (Full D-Bus Isolation)
* **内容**: 例外を設けず、Termway が起動するすべてのクライアントプロセスに対して隔離 D-Bus を強制する。
* **却下理由**: Ubuntu 標準の Firefox や Chromium 等の Snap アプリケーションが cgroup スコープ登録不能により 100% 起動失敗するため、主要な実利用ユースケースを満たせない。

### 代替案 B: 全面ホスト D-Bus 共有 (Full Ambient D-Bus Sharing)
* **内容**: D-Bus の隔離を行わず、すべてのクライアントでホストの D-Bus をそのまま利用する。
* **却下理由**: `gnome-terminal` や `gedit` 等の単一インスタンス型アプリを起動した際、ホストデスクトップ側の既存プロセスにウィンドウが奪われ、ターミナル内に表示されないため不可。

### 代替案 C: D-Bus プロキシ (`xdg-dbus-proxy` 等) による通信レベルのフィルタリング
* **内容**: クライアントとホスト D-Bus の間にプロキシプロセスを介在させ、`org.freedesktop.systemd1`（cgroup）や `org.freedesktop.portal.Desktop`（ポータル）宛てのメッセージのみをホストへ通過させ、`org.gnome.Terminal` などのアプリ固有インターフェースは遮断・別空間にルーティングする（Flatpak が採用している手法）。
* **却下理由**:
  * `xdg-dbus-proxy` などの外部バイナリへの依存が発生し、Termway の「単一バイナリでどこでも動作するポータビリティ」が損なわれる。
  * プロキシルール（ポリシー設定）の保守が極めて複雑になり、システムの軽量性・保守性を著しく低下させる。
  * 現状の Termway のユースケースにおいては過剰設計（Overengineering）と判断。

---

## 4. トレードオフと背反 (Consequences & Trade-offs)

### 肯定的な結果 (Positive)
* **ゼロ追加依存・超軽量**: 外部プロキシプロセスを追加することなく、単一バイナリとしての軽快な動作と移植性を維持。
* **両立の実現**: `gnome-terminal` などの専用サーバープロセス起動と、Snap 版 Firefox / `xdg-open` のクラッシュなし起動を同時に達成。

### 否定的な結果・背反 (Negative / Trade-offs)
* **ホスト起動済みアプリへの委譲リスク**:
  * ホスト OS のデスクトップ上ですでに Firefox や PDF ビューア（Evince 等）が起動している状態で、Termway から `termway xdg-open https://...` や `termway firefox` を実行した場合、ホスト D-Bus 経由でホスト側の既存ウィンドウへ新規タブやウィンドウが開いてしまう可能性がある。
* **ヒューリスティック判定の保守コスト**:
  * 新たな URL ランチャーや、独自のシェルスクリプトでラップされた Snap アプリケーションが登場した場合、`is_snap_command` の判定条件（ホワイトリスト）のメンテナンスが必要となる。
