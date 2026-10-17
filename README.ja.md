# sm0sh

SLMPでUbuntuのコマンドを実行する、Rust製のSLMP 3Eサーバー。

Dにコマンドと引数の文字列を書き、Wにそれぞれの終端D番号を書く。
SM0をONにすると実行し、ログをSDに格納してSM0をOFFに戻す。
SMはsm0sh側の仮想特殊リレー、SDは仮想特殊レジスタ。

## 起動と接続確認

同梱の `sm0sh` はUbuntu 24.04 / x86_64で確認した実行ファイル。
Rustのインストールなしで起動できる。

```bash
chmod +x sm0sh
./sm0sh
```

設定なしでは `127.0.0.1:5000/TCP` で待ち受ける。
別のターミナルから、Python 3のクライアントで接続確認する。

```bash
python3 examples/client.py -- /bin/echo 'hello PLC'
```

## PLCからの接続

`examples/sm0sh.toml` の `bind` をUbuntuのイーサネットIP、
`allowed_ips` をPLCのIPに変更する。

```toml
bind = "192.168.10.20:5000"
allowed_ips = ["192.168.10.10"]
```

設定ファイルを指定して起動する。

```bash
./sm0sh --config examples/sm0sh.toml
```

TCPを標準とし、`transport = "udp"` または `"both"` も選べる。
Ubuntuでの常駐手順は [docs/ubuntu.md](docs/ubuntu.md) を参照。

## `echo hello` を送る例

文字列はUTF-8。1ワードに下位バイトから2バイト格納する。
奇数バイトの要素は最後の上位バイトを0にする。

| デバイス | 値 | 内容 |
| --- | --- | --- |
| D0 | `0x6365` | `ec` |
| D1 | `0x6F68` | `ho` |
| D2 | `0x6568` | `he` |
| D3 | `0x6C6C` | `ll` |
| D4 | `0x006F` | `o` + 0 |
| W0 | `1` | コマンドはD0〜D1 |
| W1 | `4` | 第1引数はD2〜D4 |
| W2 | `65535` | 要素表の終わり |

Wには各要素の終端D番号を格納する。最初の要素はD0、次の要素は前要素の終端+1から始まる。
Wのデバイス番号は16進表記。格納する値はDの0起点の番号で、
D100の終端なら100（`0x0064`）を入れる。

PLCの処理順は次のとおり。

1. DとW（最後の`65535`を含む）をそれぞれ一括書き込みする。
2. SM0を1点のビット単位書き込みでONにする。
3. SM0のOFF、またはSM1のONを待つ。
4. SWの状態とログ長を読み、SD0以降からログを取得する。

SM0の書き込み応答は受付結果。コマンドの完了を待って返す応答ではない。
実行中にD/Wを書いたりSM0を再操作すると、SLMP終了コード `0xCEE0` を返す。

## デバイス

| 領域 | 既定の点数 | 用途 |
| --- | ---: | --- |
| D0〜D8191 | 8192ワード | コマンドと引数のUTF-8文字列 |
| W0〜W7F | 128ワード | 各要素の終端D番号、最後に65535 |
| SM0 | 1ビット | 実行受付から完了までON |
| SM1〜SM4 | 4ビット | 完了、異常、タイムアウト、ログ切詰め |
| SD0〜SD8191 | 8192ワード | stdoutとstderrを合わせたUTF-8ログ |
| SW0〜SW7 | 8ワード | 状態、終了コード、ログ長、実行番号、ABI版 |

SM0の実行受付時に、割り当て済みのSD全域をクリアする。
SD、SW、SM1以降は読み取り専用。ログと実行結果は完了時にまとめて公開する。
詳しいレジスタ定義は [docs/protocol.md](docs/protocol.md) に記載。

## 引数とシェル構文

Wで分割した各要素を、そのままOSのコマンド・引数として渡す。
空白を含む引数、日本語、空文字列を扱える。引用符の再解釈はしない。

パイプやリダイレクトを使う場合は、明示的にシェルを指定する。

```bash
python3 examples/client.py -- /bin/sh -c 'printf hello | tr a-z A-Z'
```

この場合の3要素は `/bin/sh`、`-c`、`printf hello | tr a-z A-Z`。

送信前にD/Wの値を確認するには：

```bash
python3 examples/client.py --dry-run -- /bin/echo 'hello PLC'
```

## 対応範囲と運用

- 3Eバイナリ、TCP/UDP、一括読出し`0401`、一括書込み`1401`。
- Q/L形式`0000/0001`、iQ-R形式`0002/0003`のデバイス指定。
- D/W/SD/SWのワードアクセス、SMのビットアクセスとSM0のワードアクセス。
- 1つの共有メールボックス。D/Wの準備から結果の取得まで、書き手は1台にする。
- メモリはRAM上のみ。再起動時に初期化し、コマンドを自動再実行しない。
- ログは既定で最大16383バイト。超過分は読み捨て、SM4で切詰めを通知する。

SLMPにはこの用途の認証・暗号化はない。許可した接続元は、サービス実行ユーザーの
権限で任意のコマンドを起動できる。PLC用ネットワークと専用ユーザーで運用する。
IP制限は認証の代わりにはならず、コマンド自体のサンドボックス機能もない。

UDPでは応答紛失後にSM0のONを再送すると、同じコマンドが再実行される可能性がある。
副作用のある処理にはTCPを使い、応答が不明な場合は状態を確認してから判断する。

4E、ASCIIフレーム、ランダム/ブロックアクセス、CPU操作・ラベル操作は対象外。
確認済みの環境と結果は [docs/validation.md](docs/validation.md) を参照。

## 開発

ソースからのビルドにはRust 1.85以降が必要。

```bash
cargo build --release --locked
./target/release/sm0sh --config examples/sm0sh.toml
```

書式とテストの確認：

```bash
cargo fmt --check
cargo test --locked
```

## ライセンス

[MIT License](LICENSE)。依存ライブラリのライセンスは [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) を参照。
