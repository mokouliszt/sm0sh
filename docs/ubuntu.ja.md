# Ubuntuでの運用

## ローカルで試す

Rust 1.85以降のCargoでビルドする。
Ubuntu 24.04の通常の`cargo`パッケージは古い場合があるので、`rustc --version`を確認する。
Rustupのstableツールチェーン、またはUbuntuで提供される新しい版のパッケージを使う。
Ubuntu 24.04で`cargo-1.91`を使う例：

```bash
sudo apt update
sudo apt install --no-install-recommends cargo-1.91 rustc-1.91 gcc libc6-dev
cargo-1.91 build --release --locked
```

Rustupを使っている場合は通常の`cargo build --release --locked`でよい。
初回のビルドではcrates.ioへの接続が必要。閉域に置く場合は事前にビルドした実行ファイルを持ち込む。
実行時のRustツールチェーン、Python、参考クライアントのインストールは不要。
同梱済みバイナリがある場合はそのアーキテクチャと検証環境を`validation.md`で確認する。

```bash
./target/release/sm0sh --config examples/sm0sh.toml --check-config
./target/release/sm0sh --config examples/sm0sh.toml
```

別の端末から：

```bash
python3 examples/client.py -- /usr/bin/printf '%s\n' 'hello PLC'
python3 examples/client.py -- /bin/sh -c 'printf out; printf err >&2'
```

## PLC用ネットワーク

Ubuntuが`192.168.10.20`、PLCが`192.168.10.10`なら次のように設定する。
5000は例であり、PLCと一致すれば別の非特権ポートでもよい。

```toml
bind = "192.168.10.20:5000"
transport = "tcp"
allowed_ips = ["192.168.10.10"]
```

PLC側も**3E・バイナリ**に合わせる。
SLMPの一括書込みで相手先のD/W/SMを指定する機能がなければ、
ソケット送受信機能で3E要求を組み立てる。具体的な命令・設定項目はPLCとユニットにより異なる。
現在の同梱例はワイヤ形式の接続確認用で、特定CPU向けのラダープログラムではない。

UFWを有効にしている場合のTCP用ルール例：

```bash
sudo ufw allow from 192.168.10.10 to 192.168.10.20 port 5000 proto tcp
```

UDPを選んだ場合は対応するUDPルールが必要。
公開インターネットへのポート公開は想定しない。

## 常駐

ビルドしたバイナリをインストールする。
このスクリプトは既存設定を上書きせず、サービスも起動しない。

```bash
sudo sh deploy/install.sh target/release/sm0sh
sudoedit /etc/sm0sh/sm0sh.toml
sudo -u sm0sh /usr/local/bin/sm0sh --config /etc/sm0sh/sm0sh.toml --check-config
sudo systemctl enable --now sm0sh
```

`sm0sh`専用ユーザーの権限でコマンドを実行する。
既定のサービス設定で書込み可能なのは`/var/lib/sm0sh`とサービス専用の一時領域。
ホームディレクトリ配下は見えない。実行したいスクリプトは`/var/lib/sm0sh`等に配置する。
別の場所へのアクセスが業務上必要なら、サービスの`ReadWritePaths`や`ProtectHome`を調整する。
サービス設定はコマンドごとの隔離環境を作るものではない。

```bash
sudo systemctl status sm0sh
sudo journalctl -u sm0sh -f
sudo systemctl stop sm0sh
```

常駐側のログには起動先、実行番号、状態、終了コードを出力する。
コマンドのstdout/stderrはSDに保存する。コマンド文字列を常駐側ログへ列挙しない。
SIGINT/SIGTERMで新しい通信を止め、実行中のプロセスグループを終了させる。
systemdの`KillMode=control-group`も、サービス配下のプロセスを停止するために設定している。

## 設定項目

| 項目 | 既定値 | 内容 |
| --- | --- | --- |
| bind | `127.0.0.1:5000` | IPアドレスとポート。起動時に1回だけ解決する数値アドレス |
| transport | `tcp` | `tcp`、`udp`、`both` |
| allowed_ips | loopback 2件 | 許可するIPの完全一致リスト。空リスト不可 |
| d_words | 8192 | 1〜65535 |
| w_words | 128 | 2〜1024。最後のマーカーも含む |
| sd_words | 8192 | 1〜65535 |
| command_timeout_ms | 30000 | 1〜86400000ミリ秒 |
| connection_timeout_ms | 30000 | 100〜3600000ミリ秒。要求受信と応答送信それぞれに適用 |
| max_connections | 16 | TCP接続数。1〜1024 |
| working_directory | `.` | コマンドの作業ディレクトリ。起動時のcwdからの相対パスも可 |
| environment | PATH/LANG | 子プロセスへ渡す環境変数。サーバーの環境変数は引き継がない |

`[environment]`を指定した場合は、その表が子プロセス用の環境変数になる。
PATHやLANGも必要に応じて表へ含める。設定変更はサービス再起動で反映する。
未知の設定項目、範囲外、存在しない作業ディレクトリは起動エラーとして扱う。
