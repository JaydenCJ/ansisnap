# ansisnap

[English](README.md) | [中文](README.zh.md) | [日本語](README.ja.md)

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![Rust ≥1.75](https://img.shields.io/badge/rust-%E2%89%A51.75-orange)](Cargo.toml) [![Version 0.1.0](https://img.shields.io/badge/version-0.1.0-blue)](CHANGELOG.md) ![Tests](https://img.shields.io/badge/tests-91%20passed-brightgreen) [![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen)](CONTRIBUTING.md)

**ansisnap：开源的 CLI / TUI 输出快照测试工具——内置终端模拟器，比较渲染后的屏幕网格，而不是原始 ANSI 字节。**

![Demo](docs/assets/demo.svg)

```bash
git clone https://github.com/JaydenCJ/ansisnap.git && cargo install --path ansisnap
```

> 预发布：v0.1.0 尚未发布到 crates.io，请按上面方式从源码安装。单一二进制、零运行时依赖——转义序列解析器、终端模拟器、快照格式和 differ 全部只用 Rust 标准库实现。

## 为什么选 ansisnap？

终端 UI 正在复兴——ratatui、bubbletea、textual——而它们的输出测试却在默默受苦。一帧 TUI 不是文本：它是由光标跳转、整行擦除、`\r` 覆写和 SGR 颜色变换组成的字节流，任何按字节比较的快照工具都会在每次纯外观重构时挂掉。把 `ESC[1;31m` 重排成 `ESC[31;1m`、把追加改成整行重绘、捕获瞬间进度条走到 57% 而不是 58%——字节全都不同，视觉完全一致，CI 里全线飘红。常见的补救是快照前剥掉 ANSI 码，但这恰恰扔掉了 TUI 测试真正该断言的东西：内容在屏幕上的*位置*和它的*颜色*。ansisnap 用一个真正的终端模拟器终结这个两难：它运行你的命令，把字节流像 xterm 那样回放进一个 80×24（或任意尺寸）的单元格网格——光标寻址、滚动区域、CJK 宽字符、备用屏幕——然后把最终渲染出的网格连同样式存成一份可评审的文本文件。检查失败时它会告诉你「第 4 行：期望 `14 checks`，实际 `13 checks`」并在对应列下画出插入符，或者「第 0 行：文本相同，粗体绿色变成了红色」——绝不会甩给你一墙转义字节。

| | ansisnap | insta / insta-cmd | Jest 快照 | 手写 golden 文件 |
| --- | --- | --- | --- | --- |
| 比较的对象 | 渲染后的屏幕网格（逐单元格的文本 + 样式） | 原始字符串/字节，正则过滤器 | 序列化字符串 | 原始字节 |
| 理解光标移动 / `\r` 覆写 | 是——内置终端模拟器 | 否 | 否 | 否 |
| 字节不同但视觉一致的输出 | 通过 | 失败（或需逐例写过滤器） | 失败 | 失败 |
| 文本相同时的样式回归 | 如实报告（`green` → `red`） | 不可见，或一锅字节粥 diff | strip-ansi 之后不可见 | 不可见 |
| 被测对象 | 任何可执行文件、任何语言 | Rust crate | 同进程内的 JS | 任意 |
| 运行时依赖 | 无（Rust 标准库） | Rust 工具链 + crates | Node + Jest | 无 |
| 断言退出码 + stderr | 总是，且分开断言 | insta-cmd：是 | 否 | 常被遗忘 |

<sub>对比基于 2026-07 各工具的上游文档。insta 的过滤器工作在字符串层面；表中没有任何一个工具会解释光标寻址、擦除序列或备用屏幕。</sub>

## 特性

- **测试里有一个真正的终端模拟器** —— 光标移动、擦除/插入/删除、滚动区域、带延迟换行的自动折行、制表位、备用屏幕缓冲区以及完整 SGR（16/256/真彩色，`;` 与 `:` 两种形式），把任何字节流折叠成最终可见的屏幕。
- **人能直接行动的 diff** —— 行级文本 diff，在变化列下画出按显示宽度对齐的插入符；纯样式回归用英文单词单独报告（`bold,fg=green` → `fg=red`），与文本变化互不混淆。
- **天生框架无关** —— `record` 任何语言写的任何可执行文件；不集成测试运行器、没有宏、不注入进程。一个二进制通吃 ratatui、bubbletea、clap、argparse 和 shell 脚本。
- **为代码评审设计的快照** —— 带版本号的纯文本格式：`|` 前缀的屏幕行、用单词表示的样式区段、argv、退出码和净化后的 stderr。损坏的文件以 `line N: ...` 报错，绝不与垃圾数据比较。
- **跨机器确定性** —— 子进程在固定环境中运行（`TERM`、`COLUMNS`/`LINES`、`CLICOLOR_FORCE`、locale；移除 `NO_COLOR`），调色板索引 0–15 归一化为颜色名，文件里不含任何机器相关信息。
- **宽字符正确** —— CJK、假名、谚文、全角字符和 emoji 占两个单元格；网格、行宽校验和 diff 插入符对日文、中文输出同样对齐。
- **零依赖、零网络** —— 纯 Rust 标准库、一个静态二进制；ansisnap 只运行你的命令、读写本地文件，别无其他。由 91 个离线测试加一个端到端冒烟脚本验证。

## 快速开始

先录制一条吵闹的命令（`examples/greet.sh` 会打印进度条覆写、整行擦除，然后是粗体绿色结果）：

```bash
ansisnap record lint -- sh greet.sh
ansisnap check
```

真实捕获输出：

```text
recorded lint -> .ansisnap/lint.snap (exit 0, 80x24, 2 row(s) used, 2 styled span(s))
ok      lint
1 snapshot(s): 1 ok
```

快照是一个纯文本文件——直接提交。进度条的噪音消失了，只剩渲染后的屏幕：

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

当行为真的变了，失败信息读起来像一条评审评论（真实输出，修改脚本之后）：

```text
FAIL    lint
        row 0 text differs:
          expected |   PASS src/lib.rs (14 checks)
          actual   |   PASS src/lib.rs (13 checks)
                                         ^
1 snapshot(s): 0 ok, 1 failed
```

变更是有意的？`ansisnap check --update` 只重新祝福失败的快照。

## 命令

| 命令 | 退出码 | 作用 |
|---|---|---|
| `record <name> -- <cmd...>` | 0 / 2 | 运行命令，把输出经模拟器渲染后存入 `.ansisnap/<name>.snap` |
| `check [--update] [name...]` | 0 / 1 / 2 | 重新运行已录制的命令，比较渲染屏幕、样式、退出码和 stderr |
| `render [--styles] [file]` | 0 / 2 | 把 ANSI 字节（文件或 stdin）变成终端实际显示的纯文本 |
| `diff <a> <b>` | 0 / 1 / 2 | 把两个快照或原始 ANSI 捕获当作屏幕来比较 |
| `list` | 0 / 2 | 列出已录制的快照及其尺寸、退出码和命令 |

`--cols`/`--rows` 设置模拟终端尺寸（默认 80×24，按快照存储），`--dir` 更换快照目录（默认 `.ansisnap`）。

## 录制环境

`check` 必须看到与 `record` 相同的输出，因此子进程在固定的终端环境中运行：

| 键 | 值 | 效果 |
|---|---|---|
| `TERM` | `xterm-256color` | 程序选用模拟器实现的转义序列集 |
| `COLUMNS` / `LINES` | 取自 `--cols`/`--rows` | 感知尺寸的 CLI 按模拟网格渲染 |
| `CLICOLOR_FORCE` / `FORCE_COLOR` | `1` | 即使 stdout 是管道而非 PTY，颜色也保持开启 |
| `NO_COLOR` | 移除 | 录制机器的个人偏好不会泄漏进快照 |
| `LC_ALL` / `LANG` | `C.UTF-8` | 消息与数字格式不会在机器之间漂移 |

由于捕获走管道，模拟器会替 tty 行规程（ONLCR）补上裸 `\n` 本应带的回车——网格与终端实际显示一致。文件格式的完整细节见 [docs/snapshot-format.md](docs/snapshot-format.md)。

## 验证

本仓库不附带 CI；上面的每一条主张都由本地运行验证：`cargo test`（78 个单元测试 + 13 个 CLI 集成测试）和 `bash scripts/smoke.sh`，后者必须打印 `SMOKE OK`。

## 架构

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

## 路线图

- [x] 核心工具：VT/xterm 模拟器（光标、擦除、滚动区域、备用屏幕、SGR 16/256/真彩色、CJK 宽度）、带版本号的快照格式、record/check/render/diff/list、样式感知的网格 differ、固定录制环境
- [ ] PTY 捕获模式，服务那些即使有 `CLICOLOR_FORCE` 也拒绝在管道上输出颜色的程序
- [ ] 滚动回看捕获（`ED 3` 历史），支持高于网格的输出
- [ ] 多帧断言：对运行中的 TUI 的中间屏幕做快照，而不只是最终屏幕
- [ ] 易变区域掩码（忽略时钟单元格、耗时列），直接声明在快照文件里

完整列表见 [open issues](https://github.com/JaydenCJ/ansisnap/issues)。

## 贡献

欢迎贡献——请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)，从一个 [good first issue](https://github.com/JaydenCJ/ansisnap/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) 开始，或发起一个 [discussion](https://github.com/JaydenCJ/ansisnap/discussions)。

## 许可证

[MIT](LICENSE)
