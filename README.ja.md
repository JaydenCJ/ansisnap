# ansisnap

[English](README.md) | [中文](README.zh.md) | [日本語](README.ja.md)

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![Rust ≥1.75](https://img.shields.io/badge/rust-%E2%89%A51.75-orange)](Cargo.toml) [![Version 0.1.0](https://img.shields.io/badge/version-0.1.0-blue)](CHANGELOG.md) ![Tests](https://img.shields.io/badge/tests-91%20passed-brightgreen) [![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen)](CONTRIBUTING.md)

**ansisnap：CLI / TUI 出力のためのオープンソース・スナップショットテストツール——内蔵ターミナルエミュレータが、生の ANSI バイトではなく、レンダリング後のスクリーングリッドを比較する。**

![Demo](docs/assets/demo.svg)

```bash
git clone https://github.com/JaydenCJ/ansisnap.git && cargo install --path ansisnap
```

> プレリリース：v0.1.0 はまだ crates.io に公開されていないため、上記の方法でソースからインストールしてください。単一バイナリ・ランタイム依存ゼロ——エスケープシーケンスパーサ、エミュレータ、スナップショット形式、differ はすべて Rust 標準ライブラリのみで実装。

## なぜ ansisnap？

ターミナル UI はいまルネサンスの真っ只中——ratatui、bubbletea、textual——なのに、その出力テストはひっそりと苦しんでいる。TUI の 1 フレームはテキストではない：カーソルジャンプ、行消去、`\r` 上書き、SGR の色変化からなるバイトストリームであり、バイト単位で比較するスナップショットツールは見た目だけのリファクタのたびに壊れる。`ESC[1;31m` を `ESC[31;1m` に並べ替える、追記を行の再描画に変える、キャプチャの瞬間にプログレスバーが 58% ではなく 57% を指す——どれもバイトは異なり、見た目は同一で、CI は真っ赤になる。よくある対処はスナップショット前に ANSI コードを剥ぎ取ることだが、それは TUI テストが本来アサートすべきもの——画面上の*位置*と*色*——をまさに捨てている。ansisnap は本物のターミナルエミュレータを同梱してこの偽の二択を終わらせる：コマンドを実行し、バイトストリームを xterm と同じように 80×24（または任意サイズ）のセルグリッドへ再生し——カーソルアドレッシング、スクロール領域、CJK 全角文字、代替スクリーン——最終的にレンダリングされたグリッドとスタイルを、レビュー可能なテキストファイルとして保存する。チェックが失敗すると「4 行目：期待 `14 checks`、実際 `13 checks`」と該当カラムの下にキャレットを描くか、「0 行目：テキストは同一、太字の緑が赤になった」と告げる——エスケープバイトの壁を投げつけることは決してない。

| | ansisnap | insta / insta-cmd | Jest スナップショット | 手書き golden ファイル |
| --- | --- | --- | --- | --- |
| 比較対象 | レンダリング後のスクリーングリッド（セル単位のテキスト + スタイル） | 生の文字列/バイト、正規表現フィルタ | シリアライズ済み文字列 | 生バイト |
| カーソル移動 / `\r` 上書きを理解 | はい——内蔵ターミナルエミュレータ | いいえ | いいえ | いいえ |
| バイトは違うが見た目が同一の出力 | 通る | 落ちる（またはケース毎のフィルタが必要） | 落ちる | 落ちる |
| テキスト同一時のスタイル退行 | そのまま報告（`green` → `red`） | 見えない、またはバイトの粥のような diff | strip-ansi 後は見えない | 見えない |
| テスト対象 | 任意の実行ファイル・任意の言語 | Rust クレート | 同一プロセス内の JS | 任意 |
| ランタイム依存 | なし（Rust 標準ライブラリ） | Rust ツールチェーン + クレート | Node + Jest | なし |
| 終了コード + stderr のアサート | 常に、かつ別々に | insta-cmd：はい | いいえ | 忘れられがち |

<sub>比較は 2026-07 時点の各ツールの公式ドキュメントに基づく。insta のフィルタは文字列レベルで動作し、表中のどのツールもカーソルアドレッシング・消去シーケンス・代替スクリーンを解釈しない。</sub>

## 特徴

- **テストの中に本物のターミナルエミュレータ** —— カーソル移動、消去/挿入/削除、スクロール領域、遅延折り返し付きオートラップ、タブストップ、代替スクリーンバッファ、完全な SGR（16/256/トゥルーカラー、`;` と `:` の両形式）が、あらゆるバイトストリームを最終的に見えるスクリーンへ畳み込む。
- **人がそのまま行動できる diff** —— 行レベルのテキスト diff は変化したカラムの下に表示幅対応のキャレットを描き、スタイルのみの退行はテキスト変更とは別に英単語で報告される（`bold,fg=green` → `fg=red`）。
- **設計からフレームワーク非依存** —— どの言語のどんな実行ファイルでも `record` できる；テストランナー統合もマクロもプロセス注入も不要。1 つのバイナリが ratatui、bubbletea、clap、argparse、シェルスクリプトを等しく扱う。
- **コードレビューのためのスナップショット** —— バージョン付きプレーンテキスト形式：`|` プレフィックスのスクリーン行、単語で表すスタイルスパン、argv、終了コード、正規化済み stderr。壊れたファイルは `line N: ...` で失敗し、ゴミデータと比較することは決してない。
- **マシンをまたぐ決定性** —— 子プロセスは固定環境で走り（`TERM`、`COLUMNS`/`LINES`、`CLICOLOR_FORCE`、ロケール；`NO_COLOR` は除去）、パレットインデックス 0–15 は色名に正規化され、マシン固有の情報は一切ファイルに入らない。
- **全角文字に正確** —— CJK、かな、ハングル、全角形、絵文字は 2 セルを占有；グリッド、行幅検証、diff のキャレットは日本語・中国語出力でもずれない。
- **依存ゼロ・ネットワークゼロ** —— 純粋な Rust 標準ライブラリ、1 つの静的バイナリ；ansisnap はコマンドを実行しローカルファイルを読み書きするだけ。91 件のオフラインテストとエンドツーエンドのスモークスクリプトで検証済み。

## クイックスタート

騒がしいコマンドを一度録画する（`examples/greet.sh` はプログレスバーの上書き、行消去、そして太字緑の結果を出力する）：

```bash
ansisnap record lint -- sh greet.sh
ansisnap check
```

実際にキャプチャした出力：

```text
recorded lint -> .ansisnap/lint.snap (exit 0, 80x24, 2 row(s) used, 2 styled span(s))
ok      lint
1 snapshot(s): 1 ok
```

スナップショットはプレーンテキストファイル——そのままコミットできる。プログレスバーのノイズは消え、レンダリングされたスクリーンだけが残る：

```text
ansisnap snapshot v1
cmd: ["sh","greet.sh"]
term: 80x24
exit: 0
--- screen: 24 rows x 80 cols ---
|   PASS src/lib.rs (14 checks)
|   PASS src/cli.rs (9 checks)
...
--- styles: 2 spans ---
r0 c0-c6 bold,fg=green
r1 c0-c6 bold,fg=green
```

挙動が本当に変わると、失敗はレビューコメントのように読める（スクリプト編集後の実出力）：

```text
FAIL    lint
        row 0 text differs:
          expected |   PASS src/lib.rs (14 checks)
          actual   |   PASS src/lib.rs (13 checks)
                                         ^
1 snapshot(s): 0 ok, 1 failed
```

意図した変更なら？ `ansisnap check --update` が失敗したスナップショットだけを再祝福する。

## コマンド

| コマンド | 終了コード | 動作 |
|---|---|---|
| `record <name> -- <cmd...>` | 0 / 2 | コマンドを実行し、出力をエミュレータで描画して `.ansisnap/<name>.snap` に保存 |
| `check [--update] [name...]` | 0 / 1 / 2 | 録画済みコマンドを再実行し、描画スクリーン・スタイル・終了コード・stderr を比較 |
| `render [--styles] [file]` | 0 / 2 | ANSI バイト（ファイルまたは stdin）を端末が実際に表示するプレーンテキストへ変換 |
| `diff <a> <b>` | 0 / 1 / 2 | 2 つのスナップショットまたは生 ANSI キャプチャをスクリーンとして比較 |
| `list` | 0 / 2 | 録画済みスナップショットをサイズ・終了コード・コマンド付きで一覧表示 |

`--cols`/`--rows` はエミュレートする端末サイズを設定（既定 80×24、スナップショット毎に保存）、`--dir` はスナップショットディレクトリを変更（既定 `.ansisnap`）。

## 録画環境

`check` は `record` と同じ出力を見なければならないため、子プロセスは固定されたターミナル環境で実行される：

| キー | 値 | 効果 |
|---|---|---|
| `TERM` | `xterm-256color` | プログラムがエミュレータの実装するエスケープ集合を選ぶ |
| `COLUMNS` / `LINES` | `--cols`/`--rows` から | サイズを意識する CLI がエミュレートされたグリッドに合わせて描画する |
| `CLICOLOR_FORCE` / `FORCE_COLOR` | `1` | stdout が PTY ではなくパイプでも色が有効のまま |
| `NO_COLOR` | 除去 | 録画マシンの個人設定がスナップショットへ漏れない |
| `LC_ALL` / `LANG` | `C.UTF-8` | メッセージや数値の書式がマシン間でぶれない |

キャプチャはパイプ経由なので、tty のライン制御（ONLCR）が裸の `\n` に付けるはずの CR をエミュレータが補う——グリッドは端末が実際に表示するものと一致する。ファイル形式の詳細は [docs/snapshot-format.md](docs/snapshot-format.md) を参照。

## 検証

このリポジトリは CI を同梱しない；上記の主張はすべてローカル実行で検証される：`cargo test`（ユニット 78 件 + CLI 統合 13 件）と `bash scripts/smoke.sh`（`SMOKE OK` を出力しなければならない）。

## アーキテクチャ

```mermaid
flowchart LR
    C[your command] -->|pinned env, piped| R[Runner]
    R -->|stdout bytes| P[ANSI parser]
    P -->|actions| E[Screen emulator: cell grid]
    E --> F[Frame: rows + style spans]
    F --> S[.snap file]
    S --> D[Differ]
    F --> D
    D --> O[row/style/exit/stderr report]
```

## ロードマップ

- [x] コアツール：VT/xterm エミュレータ（カーソル、消去、スクロール領域、代替スクリーン、SGR 16/256/トゥルーカラー、CJK 幅）、バージョン付きスナップショット形式、record/check/render/diff/list、スタイル対応グリッド differ、固定録画環境
- [ ] `CLICOLOR_FORCE` があってもパイプでは色を出さないプログラム向けの PTY キャプチャモード
- [ ] スクロールバックキャプチャ（`ED 3` 履歴）でグリッドより高い出力に対応
- [ ] マルチフレームアサーション：最終画面だけでなく、実行中の TUI の中間スクリーンもスナップショット
- [ ] 揮発領域マスク（時計セルや所要時間カラムを無視）をスナップショットファイル内で宣言

完全なリストは [open issues](https://github.com/JaydenCJ/ansisnap/issues) を参照。

## コントリビュート

コントリビューション歓迎——[CONTRIBUTING.md](CONTRIBUTING.md) を読み、[good first issue](https://github.com/JaydenCJ/ansisnap/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) から始めるか、[discussion](https://github.com/JaydenCJ/ansisnap/discussions) を開いてほしい。

## ライセンス

[MIT](LICENSE)
