# music-app

Slint + Rust + libmpv 音乐播放器，使用 QuickJS 执行独立 JavaScript 音乐源。默认只提供本地音乐。网易云插件单独安装，不需要 Flutter、Node.js 或外部 API 服务。

当前版本为 **0.2.5**，应用内名称为 Music Player。版本和工具链分别由 [Cargo.toml](Cargo.toml) 与 [rust-toolchain.toml](rust-toolchain.toml) 管理。

## Windows 使用

解压 `dist/MusicPlayer-windows-x64-0.2.5.zip`，运行 `music-app.exe`，保留同目录的 `mpv-2.dll`。

`dist/` 为本地构建产物，不随 Git 仓库分发；首次克隆请按下文构建。

1. 设置与插件 → 添加音乐目录，扫描标签、封面和同名 `.lrc`。
2. 可选：安装 `dist/netease.ypm.zip`，查看网络权限并启用，再从左侧切换网易云音乐。
3. 支持搜索、推荐、排行榜、歌单、专辑、歌手、歌词和扫码登录；每日推荐、我的云盘、个人收藏需要登录。
4. 歌曲行的 ＋ 默认加入队列；设置中选定本地歌单后，本地歌曲的 ＋ 加入该歌单。底栏提供歌词页、队列、循环、随机和音量。F7 播放/暂停，F8 下一首。

Windows 跨屏拖动按屏幕长边、短边计算统一缩放倍数，保持窗口长宽比，支持横竖屏切换。界面与输入坐标按目标 DPI 更新，最小内容区为 720×480 逻辑像素。具体规则与测试结果见 [验收记录](docs/validation.md)。

支持拖动边框调整大小、双击标题栏及最大化。窄窗口自动调整侧栏、播放栏和列表，封面切换为两列。Windows 11 标题栏的背景、文字与边框跟随应用深浅主题；更早系统使用原生深浅主题。

0.1.x 升级：导入新版 JS 插件即可沿用同一 `netease` ID 的加密会话、设备标识和音质设置。不要先卸载旧源；卸载会清除该源的账号数据。新版不加载 `.wasm`，旧文件保留供旧程序使用。不要同时运行两个版本并修改同一账号或音乐库。

Windows 数据目录：`%APPDATA%/music-api/MusicPlayer/data`。`MUSIC_API_DATA_DIR` 可覆盖位置。队列、索引、歌单和外观存入 SQLite；插件 KV 使用 AES-GCM 加密，主密钥保存在系统凭据服务。退出网易云只清理本插件会话。

仓库名为 `music-app`，数据目录、环境变量 `MUSIC_API_*` 和系统凭据中的历史标识继续兼容已有安装，以便升级后读取原来的音乐库和登录状态。

## 开发与构建

需要 Rust 1.99.0 和 Windows MSVC C++ 工具链；QuickJS 由 Rust 依赖构建，不需要额外 JS 运行时。

克隆仓库，在仓库根目录执行以下命令：

```powershell
git clone git@github.com:zacharyzj/music-app.git
cd music-app
./scripts/get-mpv.ps1
cargo xtask plugin
$env:MUSIC_API_MPV = "$PWD/dist/native/mpv-2.dll"
cargo run -p music-app --locked
```

`get-mpv.ps1` 下载固定版本的 libmpv 并校验 SHA-256。生成 Windows 发布包：

```powershell
./scripts/build-windows.ps1 -OutputName MusicPlayer-windows-x64-0.2.5
```

发布包位于 `dist/MusicPlayer-windows-x64-0.2.5.zip`；可选网易云插件位于 `dist/netease.ypm.zip`。打包脚本会重新构建插件和 release 主程序，`-Debug` 可生成调试包。

主程序包不预装在线源。`cargo xtask plugin` 将 `plugins/netease-js` 打包为 `.ypm.zip`。安装器也接受兼容的 YesPlayMusic 插件 ZIP，支持根目录或一层外包装目录中的 `plugin.json`。本宿主实现了网易云使用的 API 子集，不承诺兼容所有 YesPlayMusic 扩展能力。

libmpv 在运行时动态加载，无需开发头文件。macOS 使用 `libmpv.2.dylib`，Linux 使用 `libmpv.so.2`；可用 `MUSIC_API_MPV` 指定路径。

## 验证

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo xtask plugin
```

普通测试使用受控 HTTP 响应，不需要账号。Windows 窗口回归使用独立的隐藏窗口，不操作正在运行的播放器。

可选联网诊断：

```powershell
# 以下命令会联网；--play 会低音量播放一小段音频
cargo xtask probe 晴天 --play
cargo xtask probe-account "$env:APPDATA/music-api/MusicPlayer/data"
```

账号诊断需要已有登录状态，使用临时加密副本，不写回现有会话，只输出条目数、分页和耗时。

## 工程结构

| 目录 | 职责 |
|---|---|
| `crates/core` | 领域模型、Source 接口、队列、LRC |
| `crates/library` | SQLite、本地标签索引与歌单 |
| `crates/player` | libmpv 动态 API、播放器线程 |
| `crates/plugin-host` | QuickJS、模块加载、YPM 适配、权限与加密存储 |
| `crates/app` | Slint 页面、后台任务、封面缓存 |
| `plugins/netease-js` | 网易云 JS 插件，基于 MIT 上游源码 |
| `xtask` | JS 打包、联网探测及真实播放检查 |
| `scripts` | Windows 原生依赖下载与发布包构建 |
| `docs` | 插件协议、验收记录与历史版本说明 |
| `.github/workflows` | 桌面构建测试与 Android 交叉检查 |
| `legacy`、`crates/plugin-sdk`、`plugins/netease` | 旧 WASM 实现留档；当前应用不加载它 |

`.ref` 仅作为参考，不参与构建。依赖精确版本见 `Cargo.lock`。

## 仓库约定

- 提交源代码、Slint 界面与图标、插件源码和授权文件、构建脚本、CI 配置及文档。
- 提交 `Cargo.lock`，固定应用和工作区依赖版本；`.cargo/config.toml` 中的 `xtask` 别名也随仓库维护。
- `target/`、`dist/`、原生库、可执行文件和插件压缩包属于生成产物，由 [.gitignore](.gitignore) 排除。
- `.ref/`、`.test-data/`、本地数据库、加密账号文件、日志和 `.env` 不入库；需要共享环境配置时使用不含凭据的 `.env.example`。
- [.gitattributes](.gitattributes) 统一 Git 文本换行，[.editorconfig](.editorconfig) 定义 UTF-8、缩进和行尾约定；Rust 格式以 `cargo fmt` 为准。

## 平台范围

本次交付 Windows x64。macOS/Linux 与 Android 交叉检查已配置 CI，未在本机完成运行验收。Android 的 SAF、Keystore、音频焦点、后台服务和 libmpv 打包尚未适配，不提供 APK。

界面提供明暗主题、封面卡片、歌曲列表、歌词和队列；未实现 Coverflow、托盘、媒体系统快捷键、下载、云盘上传或插件市场。

详见 [插件协议](docs/plugins.md)、[验收记录](docs/validation.md)、[第三方授权说明](THIRD_PARTY_NOTICES.md)。
