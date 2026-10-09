# ADR-0001: ハイブリッド D-Bus セッションルーティングと Snap アプリケーション対応

* **ステータス**: 承認済 (Accepted)
* **日付**: 2026-10-09
* **対象コンポーネント**: `src/session/mod.rs` (`SessionManager`, `is_snap_command`, `LauncherShims`), `src/session/secret_bridge.rs`, `src/engine.rs`, `src/compositor/mod.rs`

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

### 課題 4: 起動済みクライアントの子孫プロセスからの Snap 起動
* 課題 3 の判定は Termway が直接起動するトップレベルコマンドにしか適用されない。
* `termway -- code` で VS Code を起動し、GitHub ログインでブラウザを開く場合、VS Code（非 Snap）には隔離 D-Bus が渡され、その子孫である `xdg-open` → Snap 版 Firefox も隔離 D-Bus を継承するため、課題 2 と同じ cgroup エラーでブラウザが起動しない。
* また、SSH の非ログインシェルから起動した場合は `/etc/profile.d` が `XDG_DATA_DIRS` に `/var/lib/snapd/desktop` を追加しないため、GIO が Snap 版ブラウザの `.desktop` を見つけられず、https URL を gvfs 経由でテキストエディタに開いてしまう。

### 課題 5: OS キーリング（Secret Service）が隔離バスから見えない
* VS Code（Electron）や Chromium は、暗号化キーを libsecret 経由でホストの gnome-keyring（`org.freedesktop.secrets`）に保存する。
* 隔離 D-Bus には Secret Service が存在しない。D-Bus アクティベーションで起動される `gnome-keyring-daemon --start` は、制御ソケットでホストのデーモンを検出したまま名前を取得せずにハングする。
* さらに SSH 経由では `XDG_CURRENT_DESKTOP` が空のため、Electron はキーリングを使わない basic 暗号化を選び、「An OS keyring couldn't be identified」警告を表示する。

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
4. **子孫プロセス向けの PATH シム (`LauncherShims`)** (課題 4):
   * 隔離 D-Bus とホスト D-Bus の両方が利用可能で `/snap/bin` が存在する場合、`$XDG_RUNTIME_DIR/termway-shims-<pid>/` にラッパースクリプトを生成し、Termway プロセスの `PATH` 先頭に追加する（全クライアントとその子孫が継承する）。
   * `/snap/bin` の各エントリ: 常に `DBUS_SESSION_BUS_ADDRESS` をホスト D-Bus に差し替えて実体を exec する。
   * URL ランチャー（`xdg-open`, `sensible-browser`, `x-www-browser`, `gnome-www-browser`, `www-browser`）: 第1引数が `file:` 以外の URL（`*://*`, `mailto:*`）の場合のみホスト D-Bus に差し替える。ファイルを開く場合は隔離 D-Bus のまま（課題 1 の保護を維持）。`.desktop` の `Exec=/snap/bin/firefox` のような絶対パス起動も、ランチャー段階でバスを差し替えることで救済する。
   * シムディレクトリは終了時に削除し、シグナルで強制終了された場合の残骸は次回起動時に PID の生存確認をして削除する。
6. **Secret Service 単一サービスの中継 (`secret_bridge::ServiceBridge`)** (課題 5):
   * ホストバスに `org.freedesktop.secrets` の所有者がいる、または起動可能な場合に限り、Termway が隔離バス上でこの名前を所有する。
   * 隔離バスに届いたメソッド呼び出しをホストの Secret Service に転送し、応答は呼び出し元へ、ホストからのシグナル（`Prompt.Completed` 等）は隔離バスへ再送する。メッセージ本体は解釈せず、ヘッダのみ再エンコードする（D-Bus ワイヤプロトコルの最小実装を内蔵し、外部依存は追加しない）。
   * 中継が有効になった場合のみ、`XDG_CURRENT_DESKTOP` が空ならホストの `systemctl --user show-environment` から取り込み、Electron に libsecret を選択させる。
7. **`XDG_DATA_DIRS` の補完** (課題 4):
   * `/var/lib/snapd/desktop` が存在し `XDG_DATA_DIRS` に含まれていない場合は末尾に追加する（未設定時は XDG 仕様の既定値 `/usr/local/share:/usr/share` を基点とする）。

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
* **追記（課題 5）**: 汎用のポリシー付きプロキシは引き続き不採用とする。ただし、隔離の外に出す必要が明確な Secret Service 1 サービスに限り、外部依存なしの内蔵中継（決定事項 6）を採用した。対象を 1 つの名前に固定しているため、ポリシー保守の問題は生じない。

---

## 4. トレードオフと背反 (Consequences & Trade-offs)

### 肯定的な結果 (Positive)
* **ゼロ追加依存・超軽量**: 外部プロキシプロセスを追加することなく、単一バイナリとしての軽快な動作と移植性を維持。
* **両立の実現**: `gnome-terminal` などの専用サーバープロセス起動と、Snap 版 Firefox / `xdg-open` のクラッシュなし起動を同時に達成。

### 否定的な結果・背反 (Negative / Trade-offs)
* **ホスト起動済みアプリへの委譲リスク**:
  * ホスト OS のデスクトップ上ですでに Firefox や PDF ビューア（Evince 等）が起動している状態で、Termway から `termway xdg-open https://...` や `termway firefox` を実行した場合、ホスト D-Bus 経由でホスト側の既存ウィンドウへ新規タブやウィンドウが開いてしまう可能性がある。
* **キーリングの共有とプロンプト表示先**:
  * 中継先はホストのキーリングそのものであり、Termway 内のアプリはホストのアプリと同じ秘密情報にアクセスできる（同一ユーザーの権限範囲内）。
  * ホスト側のキーリングがロックされている場合、ロック解除プロンプトはホストの画面に表示され、Termway 内には表示されない。
  * 隔離バス上のクライアントはホストからは単一の接続に見えるため、クライアント切断時のセッション解放は Termway 終了時まで遅延する。
* **シムで救済できない経路**:
  * GTK アプリが `g_app_info_launch` 等で `.desktop` の `Exec=/snap/bin/...` を直接起動する場合など、PATH を経由せず URL ランチャーも通らない Snap 起動は、引き続き隔離 D-Bus を継承して失敗する。
* **ヒューリスティック判定の保守コスト**:
  * 新たな URL ランチャーや、独自のシェルスクリプトでラップされた Snap アプリケーションが登場した場合、`is_snap_command` の判定条件（ホワイトリスト）のメンテナンスが必要となる。
