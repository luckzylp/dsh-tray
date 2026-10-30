# DSH Tray

DSH Tray 是一个原生、跨平台的系统托盘控制器，用于控制
[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（`dsh`）的 Web
界面。它负责启动、监控、更新并打开本地的 `dsh web` 会话，并把这一切都放到系统
托盘中，一键即可完成。

它是一个单一的 Rust 可执行文件：没有 Electron，没有内嵌浏览器，也没有常驻后台服务。
唯一的界面就是托盘菜单（tao + tray-icon/muda，Linux 上使用 GTK/AppIndicator）。

## 目录

- [功能特性](#功能特性)
- [架构概览](#架构概览)
- [环境要求](#环境要求)
- [安装](#安装)
- [快速开始](#快速开始)
- [托盘菜单](#托盘菜单)
- [配置](#配置)
- [通道与更新](#通道与更新)
- [登录时启动](#登录时启动)
- [诊断与调试](#诊断与调试)
- [故障排查](#故障排查)
- [开发](#开发)
- [许可证](#许可证)
- [致谢](#致谢)

## 功能特性

- **启动、停止和重启 `dsh web`** —— 托盘监控自己启动的子进程，执行期间不会阻塞
  菜单。
- **打开 Web 界面** —— 托盘解析 `dsh web` 打印出的带认证信息的 URL，并用默认浏览器
  打开它（包含 token）。
- **状态色鲸鱼图标** —— 托盘图标使用 DeepSeek 鲸鱼：dsh 停止时为灰色，Web 界面运行中
  为绿色，启动/停止/更新失败或异常退出后为红色。
- **一眼看到已安装版本** —— 菜单标题栏显示已安装的 `dsh` 版本和当前生效的通道。
- **切换通道** —— 无需离开托盘即可在 `latest`、`next` 和 `alpha` 三个 npm
  dist-tag 之间切换。
- **检查更新与一键更新** —— 托盘在后台按可配置的间隔轮询 npm dist-tag，发现新版本时
  给出提示，并可以替你执行 `npm install -g`。
- **登录时启动** —— 各平台原生的自启动集成（Linux 使用 XDG autostart，macOS 使用
  LaunchAgent，Windows 使用 `HKCU\...\Run`）。
- **日志与配置触手可及** —— 菜单项可以打开日志文件和配置目录。
- **单实例** —— 第二次启动托盘时会把请求交给已在运行的实例，而不是再显示一个托盘
  图标；如果 dsh 正在服务，会打开已有的 Web 界面。
- **只管理自己启动的进程** —— `停止`/`重启` 只作用于托盘启动的 `dsh` 进程，不会
  接管或停止其他服务。
- **英文与简体中文界面** —— 在 **语言** 子菜单中切换（跟随系统、English、简体中文），
  选择会保存到配置。
- **诊断命令行** —— `--check`、`--check --json` 和 `--smoke`，用于健康检查和 CI
  冒烟测试。

## 架构概览

`dsh-tray` 在主线程运行 tao 事件循环，用一个控制器线程持有配置和状态，并用短生命
周期的工作线程执行耗时任务（npm 网络请求、安装、等待就绪）。托盘只负责渲染不可变
快照并回传命令，从不直接操作子进程。

```text
主线程                          控制器线程                        工作线程
-----------                     -----------------                 -------
tao 事件循环                    持有 Config + Snapshot            网络 / npm / 等待
  托盘图标 + 菜单   <--快照--      命令循环                          |
  菜单事件 -> Command ---------->                                  |
  托盘点击 -> OpenWeb
```

模块划分和完整的进程模型见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

## 环境要求

| 组件 | 要求 |
| --- | --- |
| Rust（从源码构建） | 1.85 或更高版本；edition 2021 |
| Node.js 与 npm | 在 `PATH` 中，或在配置里设置 `node_path` / `npm_path` |
| `dsh` 命令行 | 通过 npm 全局安装：`@deepseek-ai/dsh` |
| Linux 运行时 | GTK3 以及 StatusNotifier/AppIndicator 宿主（见下文） |
| Linux 构建依赖 | `libgtk-3-dev` 和 `libayatana-appindicator3-dev` |
| macOS / Windows | 无额外运行时依赖；发布的可执行文件未签名 |

目前 Linux 上的手工测试最充分。macOS 和 Windows 的代码路径会在 CI 中编译和测试，
但人工验证较少；遇到问题请提交报告，而不要假设它们已被完整支持。

## 安装

### 预编译产物

维护者推送 `v*` 标签后，发布工作流会构建以下产物并附加到该标签对应的 GitHub
Release 上（请使用托管此代码仓库的项目的 Releases 页面；本文档不硬编码 URL）：

| 平台 | 产物 |
| --- | --- |
| Linux x86_64 | `dsh-tray-linux-x86_64.tar.gz` |
| Linux aarch64 | `dsh-tray-linux-aarch64.tar.gz` |
| Windows x86_64 | `dsh-tray-windows-x86_64.zip` |
| macOS x86_64 | `dsh-tray-macos-x86_64.tar.gz` |
| macOS Apple Silicon | `dsh-tray-macos-aarch64.tar.gz` |
| macOS 应用包 | `DSH-Tray-macos-<arch>.app.tar.gz` |
| 校验和 | `checksums.txt`（SHA-256，每个产物一行） |

压缩包中包含可执行文件、`LICENSE`、`README.md` 和 `README.zh-CN.md`。

Linux：

```bash
tar xzf dsh-tray-linux-x86_64.tar.gz
install -Dm755 dsh-tray "$HOME/.local/dsh-tray/dsh-tray"
# 可选：使用源码仓库中的 packaging/install.sh 安装桌面启动项/图标并生成备份
```

macOS：

```bash
tar xzf DSH-Tray-macos-aarch64.app.tar.gz
mv "DSH Tray.app" /Applications/
xattr -dr com.apple.quarantine "/Applications/DSH Tray.app"   # 未签名构建
open "/Applications/DSH Tray.app"
```

Windows：解压 zip，把 `dsh-tray.exe` 放到 `PATH` 中的某个目录。

### 从源码构建

先安装 Linux 构建依赖：

```bash
sudo apt-get update
sudo apt-get install -y libgtk-3-dev libayatana-appindicator3-dev
```

然后构建或安装：

```bash
cargo build --release --locked
# 可执行文件：target/release/dsh-tray        （Windows 上是 target\release\dsh-tray.exe）

cargo install --path . --locked
# 安装到 ~/.cargo/bin
```

### Linux 桌面集成

在源码仓库中，`packaging/install.sh` 会为单个用户安装全部内容（不使用 `sudo`）：

```bash
packaging/install.sh --binary target/release/dsh-tray
```

默认安装到独立目录 `~/.local/dsh-tray`（例如 `/home/Kevin/.local/dsh-tray`）：

```text
~/.local/dsh-tray/
  dsh-tray              可执行文件
  icon.png              桌面图标副本
  dsh-tray.desktop      使用绝对路径的启动项
  backups/<时间戳>/     本次安装替换掉的旧文件备份
```

启动项和图标还会注册到 `~/.local/share/applications` 与
`~/.local/share/icons/hicolor/256x256/apps`；加 `--no-desktop` 则只保留安装目录。
用 `--home DIR` 可为其他用户的主目录安装，`--prefix DIR` 自定义安装根目录，
`APPDIR` / `ICONDIR` 可覆盖 XDG 目录。所有被替换的文件（可执行文件、图标、桌面
启动项，包括 XDG 目录中的副本）都会备份到 `<安装根目录>/backups/<时间戳>/`，
因此桌面图标在该目录下始终有备份。生成的启动项使用绝对 `Exec`/`Icon` 路径，
无需把安装目录加入 `PATH`。手动安装时，`packaging/dsh-tray.desktop` 是一个现成的
启动项，要求 `dsh-tray` 在 `PATH` 中。

## 快速开始

1. 如果还没有安装 `dsh` 命令行：

   ```bash
   npm install -g @deepseek-ai/dsh@latest
   dsh --version
   ```

2. 启动托盘：

   ```bash
   dsh-tray
   ```

3. 首次启动时托盘使用内置默认值；由于 `start_dsh_on_launch` 默认为 `true`，它会
   同时启动 `dsh web`。之后可以通过托盘菜单打开 Web 界面，或停止/重启服务。
   `config.json` 会在下一次修改设置（或退出托盘）时写入。

默认情况下，托盘以 `--no-open` 和 `--port 0` 的方式启动 `dsh web`，让操作系统
分配空闲端口，因此它既不需要猜测端口，也不会与已有的服务冲突。

## 托盘菜单

| 菜单项 | 作用 |
| --- | --- |
| 标题行 | 已安装的 `dsh` 版本和当前生效的通道 |
| 状态行 | 运行/停止状态；运行时显示去掉 token 的 URL |
| **打开 Web 界面** | 打开带认证的 URL；如有必要先启动 `dsh web` |
| **启动 DSH** | 启动由托盘管理的 `dsh web` 进程 |
| **停止 DSH** | 停止托管进程（先 SIGTERM，宽限期后 SIGKILL） |
| **重启 DSH** | 停止并重新启动托管进程 |
| 版本子菜单 | 已安装版本、各通道的目标版本、**检查更新**、**更新到 …** |
| 通道子菜单 | 在 `latest`、`next` 和 `alpha` 之间切换 |
| 语言子菜单 | 跟随系统区域、English 或简体中文（会持久保存） |
| **登录时启动托盘** | 切换平台自启动项 |
| **托盘启动时启动 DSH** | 切换 `start_dsh_on_launch` |
| **打开日志** / **打开配置目录** | 打开 `dsh-tray.log` / 配置目录 |
| **退出** | 停止托管子进程（当 `stop_dsh_on_quit` 为真时）并退出 |

## 配置

### 文件位置

| 系统 | 配置目录 | 数据目录 |
| --- | --- | --- |
| Linux | `$XDG_CONFIG_HOME/dsh-tray`（默认 `~/.config/dsh-tray`） | `$XDG_DATA_HOME/dsh-tray`（默认 `~/.local/share/dsh-tray`） |
| macOS | `~/Library/Application Support/ai.deepseek.dsh-tray` | 同上 |
| Windows | `%APPDATA%\deepseek\dsh-tray\config` | `%APPDATA%\deepseek\dsh-tray\data` |

`DSH_TRAY_CONFIG_DIR` 和 `DSH_TRAY_DATA_DIR` 可以覆盖这两个位置。托盘会在启动时
创建所需的目录。

| 路径 | 用途 |
| --- | --- |
| `config.json` | 用户设置（位于配置目录） |
| `logs/dsh-tray.log` | 托盘日志；超过 2 MiB 时轮转为 `dsh-tray.log.1` |
| `logs/dsh-web.log` | 托管的 `dsh web` 进程的原始 stdout/stderr |
| `runtime.json` | 供第二次启动使用的运行时交接状态 |
| `instance.lock` | 单实例锁 |
| `tray-icon/` | Linux AppIndicator 使用的临时图标文件 |

### `config.json` 字段

磁盘上的所有字段都可省略：缺失或未知的键会回退到下面的默认值。损坏的配置文件会被
忽略（在日志中给出警告），而不会阻止程序启动。

| 字段 | 默认值 | 含义 |
| --- | --- | --- |
| `schema_version` | `1` | 配置结构版本，预留给未来的迁移 |
| `channel` | `"latest"` | 首选的 npm 通道：`latest`、`next` 或 `alpha` |
| `port` | `0` | 传给 `dsh web` 的端口；`0` 表示由操作系统选择空闲端口（推荐）。非零值会固定端口，可能冲突 |
| `host` | `"127.0.0.1"` | 传给 `dsh web --host` 的 IP 字面量；通配地址或非 IP 值会被替换为 `127.0.0.1` |
| `open_browser_when_ready` | `true` | 托盘启动时自动启动 dsh 并在就绪后打开浏览器（`--hidden` 会抑制；**Open Web UI** 始终会打开） |
| `start_dsh_on_launch` | `true` | 托盘启动时启动 `dsh web` |
| `autostart` | `false` | 登录时启动托盘（与平台自启动项保持一致） |
| `language` | `"auto"` | 托盘界面语言：`auto`（跟随系统区域）、`en` 或 `zh`；也可在 **语言** 子菜单中切换 |
| `dsh_path` | `null` | 显式指定 dsh 入口（`bin.js`、shim 或符号链接），绕过 PATH 探测 |
| `node_path` | `null` | 显式指定 `node` 可执行文件 |
| `npm_path` | `null` | 显式指定 `npm` 可执行文件 |
| `registry` | `null` | npm registry 基础 URL；`null` 表示自动探测 |
| `update_check_hours` | `6` | 后台轮询 dist-tag 的间隔，单位小时（限制在 1–720） |
| `stop_dsh_on_quit` | `true` | 退出托盘时停止由托盘管理的 `dsh` 进程 |

完整的默认配置文件：

```json
{
  "schema_version": 1,
  "channel": "latest",
  "port": 0,
  "host": "127.0.0.1",
  "open_browser_when_ready": true,
  "start_dsh_on_launch": true,
  "autostart": false,
  "language": "auto",
  "dsh_path": null,
  "node_path": null,
  "npm_path": null,
  "registry": null,
  "update_check_hours": 6,
  "stop_dsh_on_quit": true
}
```

加载时无效的值会被修复：非 IP 或通配的 host 会变成 `127.0.0.1`，
`update_check_hours` 会被限制在有效范围内，无效的 registry URL 会回退到自动探测。
修复结果会写入日志。

### 环境变量

| 变量 | 作用 |
| --- | --- |
| `DSH_TRAY_CONFIG_DIR` | 覆盖配置目录 |
| `DSH_TRAY_DATA_DIR` | 覆盖数据目录（日志和运行时状态） |
| `DSH_TRAY_DEBUG` | 任意非空值都会把日志同时输出到 stderr |
| `DSH_TRAY_LANG` | 覆盖语言探测（`en`/`zh`，也接受区域设置风格的取值） |

## 通道与更新

`dsh` 命令行以 `@deepseek-ai/dsh` 为名发布在 npm 上，有三个 dist-tag：

| 通道 | npm dist-tag | 含义 |
| --- | --- | --- |
| `latest` | `latest` | 稳定版本 |
| `next` | `next` | 预览版本 |
| `alpha` | `alpha` | 实验性版本 |

托盘从不硬编码版本号。它从包的 `package.json` 读取已安装版本（读不到时回退到
`dsh --version`），并从 npm registry 的 dist-tags 读取可用版本。

- **生效通道** —— 版本与某个 dist-tag 精确匹配时以它为准；多个 tag 指向同一版本时，
  由保存的偏好决定；预发布后缀只是最后的提示。
- **切换通道** —— 偏好总是会保存。只有当目标 tag 指向不同版本时才会调用 npm；
  如果托管进程正在运行，随后会重启它。
- **检查更新** —— 获取 dist-tags，并用 semver 比较版本。
- **更新** —— 先停止托管的子进程，执行
  `npm install -g @deepseek-ai/dsh@<tag>`，重新读取已安装版本，如果之前正在运行则
  重新启动子进程。
- **失败处理** —— 在托盘标题栏显示并写入日志。npm 权限错误会原样输出，并附带关于
  全局 prefix 的提示。

更新检查需要访问所配置的 npm registry；除此之外的功能都可以离线使用。

## 登录时启动

菜单中的 **登录时启动托盘** 复选框会写入各平台原生的自启动项：

| 系统 | 机制 |
| --- | --- |
| Linux | XDG 自启动文件 `$XDG_CONFIG_HOME/autostart/ai.deepseek.dsh-tray.desktop`，以 `--hidden` 启动托盘 |
| macOS | LaunchAgent `~/Library/LaunchAgents/ai.deepseek.dsh-tray.plist`（`RunAtLoad`） |
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 下的 `dsh-tray` 值 |

`--hidden` 标志表示这次是登录项启动：托盘会启动，但不会自行打开浏览器。切换该
复选框会同时更新 `autostart` 配置和操作系统中的条目；通过系统工具删除该条目同样
没有问题。

## 诊断与调试

| 命令 | 用途 |
| --- | --- |
| `dsh-tray --check` | 打印健康/诊断报告（dsh 安装、node/npm、配置、进程状态） |
| `dsh-tray --check --json` | 以机器可读的 JSON 输出同样的报告 |
| `dsh-tray --smoke` | 启动 `dsh web`、等待 URL、通过 HTTP 验证、停止并退出 |
| `dsh-tray --version` | 打印托盘版本 |
| `dsh-tray --help` | 打印用法 |
| `dsh-tray --hidden` | 启动托盘但不打开浏览器（登录项模式） |

环境提供的实用调试开关：

```bash
DSH_TRAY_DEBUG=1 dsh-tray                       # 同时把日志输出到 stderr
DSH_TRAY_CONFIG_DIR=/tmp/dsh-tray-config \
DSH_TRAY_DATA_DIR=/tmp/dsh-tray-data dsh-tray --check
```

日志写入数据目录（托盘日志为 `logs/dsh-tray.log`，子进程日志为
`logs/dsh-web.log`）。设置 `DSH_TRAY_DEBUG` 后同样的内容也会出现在 stderr，
从终端或 CI 任务启动托盘时非常方便。在离线机器上诊断时，`--check` 还支持
`DSH_TRAY_OFFLINE` 来跳过 npm registry 请求。

> **Token 提醒：** `dsh web` 打印的 URL 带有认证 token。`dsh-web.log` 会记录子进程的
> 原始输出，`runtime.json` 会在服务运行期间保存该 URL，因此两者都包含 token。在 Unix
> 上这些文件以仅当前用户可读的权限创建；即便如此，也请把它们当作私密数据，不要粘贴
> 到公开的 issue 中。

## 故障排查

### 托盘提示未安装 dsh

全局安装命令行并确认它在 `PATH` 中：

```bash
npm install -g @deepseek-ai/dsh@latest
dsh --version
```

如果 `dsh` 位于非标准 prefix，请在 `config.json` 中设置 `dsh_path`（可选地还有
`node_path` / `npm_path`）。

### npm 报 EACCES / 权限错误

不要用 `sudo` 运行 npm。检查全局 prefix，改用用户自己的 prefix 或 Node 版本管理
工具：

```bash
npm config get prefix
npm config set prefix "$HOME/.npm-global"   # 然后把 $HOME/.npm-global/bin 加入 PATH
```

托盘会把 npm 的错误文本原样写入日志和托盘提示。

### Linux 上看不到托盘图标

托盘需要桌面会话中有 StatusNotifier/AppIndicator 宿主。在 GNOME 上，请安装并启用
**AppIndicator and KStatusNotifierItem Support** 扩展（或等效的 shell 扩展），然后
注销并重新登录。在 Wayland 上，图标通过合成器的 StatusNotifier 桥接显示；X11 会话通常由面板或
系统托盘实现提供宿主。如果会话中完全没有宿主，AppIndicator 应用都无法显示托盘
图标。运行时还必须安装 `libayatana-appindicator3`。

```bash
sudo apt-get install -y libayatana-appindicator3-1
```

### 端口已被占用

保持 `port` 为 `0`，让操作系统选择空闲端口 —— 托盘会读取 `dsh web` 打印的 URL，
不需要固定端口。如果你固定了端口，请改回 `0` 或释放该端口。

### 浏览器打开了但认证失败

带认证的 URL 包含 `?token=…` 查询参数，必须原样使用。请通过托盘菜单的
**打开 Web 界面** 打开，而不是手动输入地址；也不要使用从 `dsh-web.log` 中复制出来
的、可能已被截断的 URL。

### dsh 启动后一直没有就绪

这会产生启动超时的提示。查看 `logs/dsh-web.log`，并手动执行同样的命令：

```bash
dsh web --no-open --host 127.0.0.1 --port 0
```

如果手动执行也失败，问题出在 `dsh` 安装而不是托盘。用 `dsh-tray --check` 确认。

### 已经有一个外部 dsh 实例在运行

托盘始终会在空闲端口（`port: 0`）上启动自己的 `dsh web`，因此与已运行的服务不会
冲突。托盘不会接管或杀掉那个服务；如果你只想保留托盘启动的实例，请自行停止它。

### 重置配置

退出托盘，然后删除或重命名配置目录中的 `config.json`。下次启动会使用默认值
（文件会在下一次修改设置时重新写入）。日志也可以用同样的方式删除。

### macOS：Gatekeeper 阻止打开

发布构建没有签名，也没有公证。如果 macOS 隔离了下载的应用包，可以右键点击应用并
选择 **打开**，或者清除隔离属性：

```bash
xattr -dr com.apple.quarantine "/Applications/DSH Tray.app"
```

请只对你信任的构建这样做。

### Windows：SmartScreen 警告

发布的可执行文件未签名，因此 SmartScreen 可能显示“Windows 保护了你的电脑”。在
选择 **更多信息 → 仍要运行** 之前，请先用 `checksums.txt` 核对 SHA-256；如果无法
接受这一点，请从源码构建。

### macOS 和 Windows 的测试较少

Linux 是主要的开发平台。macOS 和 Windows 构建会在每次发布时由 CI 生成，但它们的
托盘、自启动和进程控制路径人工测试较少。欢迎附带相关日志的缺陷报告。

## 开发

常用命令：

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

CI（`.github/workflows/ci.yml`）在 Linux 上运行 rustfmt 和 clippy，并在 Linux、
macOS 和 Windows 上运行测试套件。发布工作流
（`.github/workflows/release.yml`）构建原生的 Linux（x86_64、aarch64）、Windows 和
macOS（Intel、Apple Silicon）产物，把 macOS 构建包装成 `DSH Tray.app` 应用包，生成
SHA-256 校验和，并把所有内容附加到带标签的 GitHub Release。产物布局和版本号更新
清单见 [docs/PACKAGING.md](docs/PACKAGING.md)。

图标：

托盘图标使用 DeepSeek 鲸鱼，并按状态重新着色（灰色 = 停止，绿色 = 运行中，红色 =
异常）。官方鲸鱼蒙版和应用图标已提交在 `assets/` 下，状态变体由它们生成：

```bash
python3 scripts/gen-icons.py                # 需要 Pillow；重新生成已提交的图标资源
scripts/make-macos-icns.sh                  # 仅限 macOS；输出 build/AppIcon.icns
```

仓库结构：

| 路径 | 用途 |
| --- | --- |
| `src/` | Rust 源码（见 `docs/ARCHITECTURE.md`） |
| `assets/` | 托盘和应用图标 |
| `packaging/` | 桌面启动项和按用户安装脚本 |
| `scripts/` | 图标生成辅助脚本 |
| `docs/` | 架构与打包文档 |

交叉编译注意事项：GUI 技术栈会链接平台框架。构建 macOS 产物需要 macOS（Xcode
命令行工具），构建 Windows 产物需要 MSVC 工具链；因此发布工作流在各自的 runner 上
原生构建，而不是交叉编译。不要指望在 Linux 主机上用一个简单的 `--target` 参数就能
产出可用的 macOS 或 Windows 二进制。

## 许可证

MIT —— 见 [LICENSE](LICENSE)。

## 致谢

- [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) —— 本托盘
  所控制的 `dsh` 运行时和 Web 界面。
- [tao](https://github.com/tauri-apps/tao)、
  [tray-icon](https://github.com/tauri-apps/tray-icon) 和
  [muda](https://github.com/tauri-apps/muda) —— 窗口/事件循环、托盘和菜单基础。
- [Pillow](https://python-pillow.org/) —— `scripts/gen-icons.py` 使用。
