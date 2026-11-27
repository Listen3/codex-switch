# codex-switch

**[OpenAI Codex CLI](https://github.com/openai/codex) 多账号管理工具** — 保存本机 Codex 登录、监控配额，并在下一次会话前选出最佳账号。

[English README](README.md) · [**完整文档（Wiki）**](https://github.com/xjoker/codex-switch/wiki) · [中文指南](https://github.com/xjoker/codex-switch/wiki/Chinese-Guide) · [Releases](https://github.com/xjoker/codex-switch/releases)

> `codex-switch` 会在本机保存账号凭据。请勿分享 profile、`auth.json`、Token、代理凭据或未脱敏的 debug 输出。

## 快速开始

Codex 必须使用文件型凭据存储。如有需要，在 `$CODEX_HOME/config.toml`（通常为 `~/.codex/config.toml`）中加入下面一行；受管配置若设置了 `forced_login_method = "api"` 则不兼容：

```toml
cli_auth_credentials_store = "file"
```

安装正式版 — macOS / Linux：

```bash
curl -fsSL https://github.com/Listen3/codex-switch/releases/latest/download/install.sh | bash
```

Windows PowerShell：

```powershell
irm https://github.com/Listen3/codex-switch/releases/latest/download/install.ps1 | iex
```

Homebrew 用户：`brew install xjoker/tap/codex-switch`。

> **注意**：本项目不在 crates.io 分发——请勿 `cargo install codex-switch`，该包名属于另一个无关的同名项目。

然后添加账号并打开仪表盘：

```bash
codex-switch login        # 无浏览器服务器加 --device
codex-switch              # 交互式仪表盘（`tui` 仍然可用）
codex-switch use          # 自动选择并切换到最佳账号
codex-switch launch       # 用最佳账号启动 Codex
```

![TUI](docs/tui.png)

在终端界面选中账号后按 `o` 打开启动弹窗。按 **Tab** 切换 **Desktop / CLI**，按 **Enter** 启动。首次默认 Desktop，之后记住每个账号最近一次启动的方式。自定义提供商使用同一个弹窗。

桌面启动使用已安装的 Codex 或 ChatGPT 应用，登录 TUI 中选中的账号。每个账号保留自己的登录和桌面设置，所有 ChatGPT 账号窗口共用默认 Codex 的本地会话库，包括后来完成的回复。同一条会话被另一个进程占用时，Codex 会提示。重复启动同一账号会唤起已有窗口。

旧窗口退出并重新启动后接入共享库，之前的会话副本保留在原实例目录。自定义 API 提供商窗口使用独立会话库。重命名或删除配置前，请先退出对应窗口。可以用 `CODEX_SWITCH_DESKTOP_PATH` 指定安装位置，用 `codex-switch launch work -- <args>` 启动 CLI 并透传参数。

Windows 上如果看到“完成 Windows 设置以继续”，请在该实例里点击“继续”，由 Codex 为这个目录初始化原生沙盒。

## 功能一览

- 保存、导入、重命名、切换和可恢复地删除 Codex 账号。
- **Beta：自定义 API 提供方。** 保存兼容 Responses 协议的端点和 API 密钥，为每个端点配置多个模型及各自的思考等级、`web_search`，获取网关模型目录并启动 Codex。兼容性取决于网关和具体模型；提供方不展示 ChatGPT 额度，也不参与自动选号。TUI Providers 页可用同一张表单新增/编辑（`a` / `e`），按 `Enter` / `o` 启动 Codex；CLI 启动使用 Codex 原生的每次运行独立 profile（`$CODEX_HOME` 中的 `cs-*.config.toml`），共享认证与 ChatGPT 配置保持不变：

  ```bash
  codex-switch provider add openrouter \
    --base-url https://openrouter.ai/api/v1 \
    --model openai/gpt-5.3-codex \
    --model deepseek/deepseek-r1-0528 --reasoning medium
  codex-switch launch openrouter
  codex-switch launch openrouter --model deepseek/deepseek-r1-0528
  ```
- CLI 与 TUI 展示主额度池和每个模型的独立额度池。
- 自适应配速感知评分自动选号，并可直接用它启动 Codex。
- 切换账号后（`use`、TUI 的 `u`，或会激活凭据的 `login`），若 Codex app-server daemon（Codex 0.157+）正在运行，会执行 `codex app-server daemon restart`，让新会话使用所选账号；可用 `[use] restart_app_server = false` 关闭。ChatGPT 的 `launch` 在 Codex 支持时改为附加 `--no-daemon`。
- 支持重置卡、一次性配额预热（`warmup`）、强制刷新额度（`list -f`）、JSON 输出和代理。TUI 的 Accounts 页在未勾选账号时用 `u` 切换选中账号并显示进度，`t` 开启当前会话自动刷新，账号菜单中的 `w` 执行一次性预热；不运行常驻服务。
- 自动刷新即将过期的 Token；直装版本自更新：`self-update`、`self-update --stable`、`self-update --version <VERSION>`，或用 `self-update --dev` 切换滚动开发通道 — 新装开发版使用 dev release 的 [install.sh](https://github.com/Listen3/codex-switch/releases/download/dev/install.sh) / [install.ps1](https://github.com/Listen3/codex-switch/releases/download/dev/install.ps1)。
- 直装版 `self-update` 同时校验 SHA-256 与 GitHub 构建来源，执行时会调用 `gh attestation verify`；使用前需安装当前版 [GitHub CLI](https://cli.github.com/)。
- 支持 macOS、Linux、Windows。

## 本 fork 增加的功能

这个 fork 保留上游的账号管理，并增加：

- **账号同步。** TUI 的「同步」页按分组把已保存的 ChatGPT 账号存到 Redis。分组名默认是计算机名。上传和下载都会先列出分组：回车处理勾选的组，`a` 处理全部。全部上传还会删掉线上那些本机没有的分组。下载一组会覆盖本机同名分组，线上已经没有的账号会从这组里移除。支持 `redis://`、Upstash 的 `rediss://` 和 `https://` REST 地址。上传内容包含每个账号的 `auth.json`，并用同步密码加密。以前未加密的分组仍可下载。Redis 必须只给自己用。
- **语言。** 设置里默认英语，可以切换成中文，账号、提供方、设置、日志和同步页会一起切换。
- **订阅到期。** 账号表增加「到期」列，数据来自 `GET /backend-api/subscriptions` 的 `active_until`。刷新该行后才会出现。5 小时和 7 天列仍是额度窗口，不是订阅到期。
- **自动预热。** 设置里可以打开：某个账号的 5 小时或 7 天窗口重置满 1 分钟后自动预热一次。只在 TUI 窗口开着时运行。

> **从 `0.0.x` 旧版本升级？** 本轮发布刻意做了两个破坏性变更：版本号改为日历格式（`YYYYMMDD.N.0`，一眼可读版本分配日期且仍按 SemVer 正常排序升级），macOS/Linux 安装位置从 `/usr/local/bin` 改为用户级 `$HOME/.local/bin`（`self-update` 不再需要 `sudo`）。正常 `self-update` 或重跑一次安装脚本即可迁移；账号与配置全部保留。全部破坏性变更及原因见 [Updating](https://github.com/xjoker/codex-switch/wiki/Updating)。

> **从带旧 daemon 的版本升级？** daemon 已删除。旧的 LaunchAgent、systemd 单元、Windows 计划任务（或旧版 `self-update`）以 daemon 方式启动新二进制时，新版本会删除这条旧注册并退出，而不是每次启动都失败；也可以手动运行 `codex-switch daemon uninstall` 完成同样的清理。旧 `[daemon]` 配置中的 `cache_refresh_interval_secs`、`auto_warmup` 等字段可以删除。见 [Updating](https://github.com/xjoker/codex-switch/wiki/Updating#migrate-from-the-removed-daemon)。

## 文档

**[GitHub Wiki](https://github.com/xjoker/codex-switch/wiki)** 是完整文档：开始使用、功能指南、自定义 API 提供方、命令参考、配置、更新与通道、故障排查、FAQ 以及贡献者指南。

中文读者建议从 **[中文指南](https://github.com/xjoker/codex-switch/wiki/Chinese-Guide)** 入手：内含 ChatGPT 日常命令、自定义提供方 CLI、TUI 两页快捷键与表单操作摘要。行为细节与边界条件仍以英文 Wiki 为准。

维护者文档：[发布流程](docs/RELEASE.md) · [更新日志](docs/CHANGELOG.md) · [贡献指南](CONTRIBUTING.md)。

## 许可证

[MIT](LICENSE)

## 第三方声明

二进制内嵌了 [OpenAI Codex](https://github.com/openai/codex)（Copyright 2025 OpenAI）的默认模型指令文本，该文本以 Apache License 2.0 分发。被引入的文本、许可证与声明位于 [`assets/upstream-codex/`](assets/upstream-codex/)（[`LICENSE`](assets/upstream-codex/LICENSE)、[`NOTICE.md`](assets/upstream-codex/NOTICE.md)）。发布压缩包在 `THIRD-PARTY-LICENSES/upstream-codex/` 下附带这些文件。
