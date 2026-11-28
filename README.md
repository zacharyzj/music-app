# music-app

Slint + Rust + libmpv 音乐播放器，使用 QuickJS 执行独立 JavaScript 音乐源。默认只提供本地音乐。网易云、哔哩哔哩、24bit、Audius 和 Incompetech 音乐源均可单独安装。运行播放器无需安装 Flutter 或 Node.js，也无需自行部署 API 服务。

当前版本为 **0.2.19**，应用内名称为 Music Player。版本和工具链分别由 [Cargo.toml](Cargo.toml) 与 [rust-toolchain.toml](rust-toolchain.toml) 管理。

## Android 使用

[下载 Music Player 0.2.19 安卓安装包（ARM64）](https://github.com/zacharyzj/music-app/releases/download/plugins-v0.2.19/MusicPlayer-android-arm64-v8a-0.2.19.apk)。支持 Android 8.0 及以上的 64 位 ARM 设备。下载后打开 APK，按系统提示允许此次安装。

首次打开后，点顶部的 ＋ 选择手机上的音频文件，可以多选。歌曲会复制到应用自己的曲库，点歌名即可播放。打开右上角的设置，在「在线插件」里安装并启用网易云、哔哩哔哩等音乐源。手机的系统媒体控件支持暂停、继续和切歌。

## Windows 使用

下载 Windows x64 便携包，解压后运行 `music-app.exe`，并将 `mpv-2.dll` 放在同一目录。[下载 Music Player 0.2.19](https://github.com/zacharyzj/music-app/releases/download/plugins-v0.2.19/MusicPlayer-windows-x64-0.2.19.zip)

1. 设置与插件 → 添加音乐目录，扫描标签、封面和同名 `.lrc`。
2. 在「设置与插件」打开「在线插件」，从 GitHub 列表选择音乐源并点击「安装」。也可以粘贴其他 HTTPS 索引或插件 ZIP 的地址，或用「导入本地 ZIP」。安装后点击「启用」，卡片会以主题色高亮。再次点击「已启用」可停用。
3. 网易云音乐支持搜索、推荐、排行榜、歌单、专辑、歌手、歌词和扫码登录。每日推荐、我的云盘、个人收藏需要登录。
4. 歌曲行的 ＋ 默认加入队列。在设置中选定本地歌单后，本地歌曲的 ＋ 加入该歌单。鼠标悬停在底部播放栏的按钮或滑条上，可查看操作和快捷键。

在「本地音乐 → 导入歌单」选择已启用的来源插件，再创建新歌单或选一个已有歌单。可以填写来源歌单 ID、粘贴网易云歌单链接，或从支持「我的歌单」的插件读取账号歌单。插件的歌单详情页也有「导入到本地歌单」入口。每首歌按原来源播放，停用或卸载插件后对应歌曲置灰，重新启用即可恢复。遇到下架歌曲时，可以保留原曲，也可以查找其他已启用来源，按来源筛选候选结果并逐首选择替换版本。

本地歌单和「本地音乐 → 喜欢的歌曲」共用歌单内搜索，可以按曲名、歌手或专辑查找。右侧的来源筛选默认选中「全部来源」，包括无源歌曲。也可选「仅看无源」或具体音乐源，与搜索词一起使用。双击可用歌曲即可播放。单击无源歌曲或点击歌曲行右侧的详情按钮，可查看歌曲信息并按曲名和歌手搜索版本。可播放的歌曲默认搜索当前来源，无源歌曲默认搜索全部可选来源。可以修改关键词、切换来源或试听候选版本。点击勾选键保存后会自动返回刚才的歌单。从本地歌单点爱心会加入本地喜欢。在「喜欢的歌曲」中换源或取消喜欢，只更新本地收藏。

Windows 跨屏拖动按屏幕长边、短边计算统一缩放倍数，保持窗口长宽比，支持横竖屏切换。界面与输入坐标按目标 DPI 更新，最小内容区为 720×480 逻辑像素。具体规则与测试结果见 [验收记录](docs/validation.md)。

支持拖动边框调整大小、双击标题栏及最大化。窄窗口自动调整侧栏、播放栏和列表，封面切换为两列。Windows 11 标题栏的背景、文字与边框跟随应用深浅主题；更早系统使用原生深浅主题。

播放器启动时会回到上次浏览的音乐源和页面，包括打开的歌单。原音乐源停用后会回到本地音乐，本地歌单已删除时会回到歌单列表。桌面端可在「设置与插件 → 音乐与外观 → 关闭窗口时」选择「退出播放器」（默认）或「隐藏到系统托盘」。隐藏后继续播放和下载，通过托盘图标或菜单可重新显示窗口或退出。

播放器启动时会在后台检查 GitHub 上的正式版，也可以打开「设置与插件 → 播放器更新」手动检查。出现新版提示后，点击「下载新版」即可用系统浏览器下载对应平台和架构的安装包。Windows 便携版下载后，退出播放器并将新版解压到原目录。点击提示上的关闭按钮可留到稍后，下载入口仍在设置里。缺少对应平台安装包时，按钮会打开该版本的 GitHub 发布页。

### 播放与音效

播放器会记住退出时正在听的曲目和进度。再次打开时，原先在播放的曲目会从该位置继续，已暂停的曲目会在原位置等待播放。隐藏到托盘时可继续收听。

在「设置与插件 → 播放与音效」打开「响度均衡」。默认关闭。歌单、搜索、随机和心动模式逐曲调整。从专辑页开始顺序播放或列表循环时，共用一个增益以保留曲间强弱。离开专辑页面继续播放也会沿用该增益。

播放器优先使用文件或音乐源提供的响度数据。缺少数据时，它用自带的 libmpv 静音分析，本地文件最多等待 5 秒，在线音频最多等待 3 秒并取开头最多 24 秒估算。实测响度以 −18 LUFS 为目标，并为峰值留出 1 dB 余量。局部样本只用于衰减，遇到静音、超时且没有有效读数时沿用原音量。缺少专辑数据时，以第一首播放曲目的安全衰减值作为整张专辑的增益。

输出设备可跟随系统，也可指定耳机或音箱。每个设备独立保存手动选择的曲风预设和低、中、高三个频段（±9 dB）。选择「原声」可将三个频段归零。桌面端支持自动识别系统默认设备，Android 13 及以上可读取系统媒体路由。系统未报告可识别的默认设备时，可以手动选择列表中的具体输出。

### 播放栏快捷键

快捷键在播放器窗口内生效。搜索框、歌单名称和插件地址输入框保留各自的文字编辑操作。

| 操作 | 快捷键 |
| --- | --- |
| 播放 / 暂停 | 空格，也可用 F7 |
| 上一首 / 下一首 | Ctrl + ← / →，F8 也可切到下一首 |
| 后退 / 前进 5 秒 | Shift + ← / → |
| 切换播放模式（顺序播放 → 列表循环 → 单曲循环 → 随机播放） | Ctrl + R，或点击「上一首」左边的模式图标。在网易云「喜欢的歌曲」里，随机播放后可再切到「心动模式」，图标为心电波形 |
| 将当前歌曲加入或移出所属音乐源的「喜欢的歌曲」 | Ctrl + D，或点击播放列表左侧的爱心。实心表示已喜欢 |
| 进入随机播放，再按恢复先前的模式 | Ctrl + S |
| 打开 / 关闭歌词页或视频页（随当前曲目来源切换） | Ctrl + L |
| 打开 / 关闭播放列表 | Ctrl + P |
| 静音 / 恢复音量 | Ctrl + M |
| 调低 / 调高音量 10 个百分点 | Ctrl + ↓ / ↑ |

### 本地搜索与下载

打开本地歌单、「喜欢的歌曲」或底部播放队列，点击云朵上箭头可将整个列表上传到网易云云盘。请先启用网易云音乐插件 1.4.0 或更高版本并登录。播放器优先使用本地文件，缺少文件时按原插件的账号权限临时下载。上传面板显示每首歌的结果，可在当前歌曲完成后停止。

在「设置与插件」中导入并启用想用的音乐源，然后回到「本地音乐」搜索。本地有匹配时直接显示本地结果。本地没有匹配时，播放器同时搜索支持该类型的已启用音乐源，结果随各源返回逐批显示。可以切换来源，或点「仅看免登录可下载」查看发布者明确开放完整下载的歌曲。同名录音保留各自的歌手、来源和下载状态。

- **24bit 无损音乐** 使用官网的两个游客搜索分库，支持按歌名和歌手查找录音。播放时读取歌曲页的公开音轨。点击下载才向站点查询本次下载额度，额度用完时会显示原因。
- **哔哩哔哩** 支持游客搜索 B 站视频，结果显示标题、UP 主和时长。双击条目收听第一分 P 的音轨。播放支持视频的来源时，点击底部的视频页按钮或按 `Ctrl+L` 即可在主窗口接着当前进度观看，支持暂停、拖动进度、切换倍速和画质。关闭视频页后继续听音频。播放普通音乐源时，同一入口显示歌词。点击搜索结果或播放队列里的「看视频」图标可打开独立视频窗口。移动鼠标可显示控制栏，点击或拖动进度条即可跳转。「倍速」（S）提供 0.5× 至 2×，「画质」（Q）列出当前视频对该账号开放的分辨率。切换画质时保留播放进度和暂停状态。空格暂停或继续，F 切换全屏，关闭画面后继续听音频。选择该源后，点击侧栏「登录」，用哔哩哔哩 App 扫码确认。本地搜不到歌曲时，已启用的 B 站也会参与并行搜索。「仅看免登录可下载」会过滤 B 站结果。
- **Audius 音乐** 可以免登录、免 API Key 搜索中文流行、独立音乐和翻唱。输入「七里香」可以查找同名录音，输入「不动守候」可以找到 MUGUNASHII 的繁体标题《不動守候》。只有发布者向游客开放了完整下载的录音才会显示可点击的「下载」。歌曲下载入库后，也可用简体或繁体歌名离线搜索。列表显示 Audius 上的发布者名称。切换到该源的首页可发现开放下载的华语音乐。[平台下载设置](https://help.audius.co/product/editing-your-release)
- **Incompetech 开放音乐** 支持按曲名、乐器或作者 Kevin MacLeod 检索，无需账号或 API Key。可用 `Monkeys Spinning Monkeys` 试搜。下载的 MP3 旁会保存同名 `.attribution.txt`，内含作者、作品页和 CC BY 4.0 许可链接。[官方曲库说明](https://incompetech.com/agent-section/)
- **网易云音乐** 支持按歌曲、专辑、歌手和歌单检索。登录后点击歌曲右侧的「下载」，由网易云下载接口按当前账号和所选音质决定是否提供完整音频。旧版插件可以直接导入新版升级。
- 连续点击多首歌的「下载」即可按顺序排队。左侧「下载队列」显示已传输字节，已知总大小时也显示百分比。等待中的任务可移除，失败任务可重试。重启后会重新获取下载地址并从头下载未完成的任务。
- Incompetech 用户可在「设置与插件」直接导入 1.0.1 版本，让开放曲目进入免登录筛选。
- 下载成功后立即加入本地音乐库，可离线搜索和播放。Windows 默认保存到 `%APPDATA%/music-api/MusicPlayer/data/downloads`，设置了 `MUSIC_API_DATA_DIR` 时使用该目录下的 `downloads`。

0.1.x 升级：导入新版 JS 插件即可沿用同一 `netease` ID 的加密会话、设备标识和音质设置。不要先卸载旧源；卸载会清除该源的账号数据。新版不加载 `.wasm`，旧文件保留供旧程序使用。不要同时运行两个版本并修改同一账号或音乐库。

Windows 数据目录为 `%APPDATA%/music-api/MusicPlayer/data`，可用 `MUSIC_API_DATA_DIR` 指定其他位置。播放器会记住音量、静音状态、视频倍速和首选分辨率。静音时退出，下次启动仍是静音，取消静音会恢复先前音量。队列、索引、歌单和外观也存入 SQLite。插件 KV 使用 AES-GCM 加密，主密钥保存在系统凭据服务。退出网易云只清理本插件会话。

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
./scripts/build-windows.ps1 -OutputName MusicPlayer-windows-x64-0.2.19
```

发布包位于 `dist/MusicPlayer-windows-x64-0.2.19.zip`；可选插件为 `dist/netease.ypm.zip`、`dist/bilibili.ypm.zip`、`dist/24bit.ypm.zip`、`dist/audius.ypm.zip` 和 `dist/incompetech.ypm.zip`。打包脚本会重新构建插件和 release 主程序，`-Debug` 可生成调试包。

Windows 打包可以在旧播放器或开发版运行期间进行。打包成功后，`dist/` 保留本次的播放器目录和 ZIP、插件包、`plugins.json` 及 `native/mpv-2.dll`。被占用的旧目录会完整保留，关闭旧播放器后，下次成功打包会自动清理。若同名目录仍在使用，新便携版会放在带 `-2`、`-3` 等后缀的目录，ZIP 文件名保持固定。调试包也遵循这个规则。构建或压缩失败时，上一次成功的产物仍可使用。

主程序包不预装在线源。`cargo xtask plugin` 分别将五个音乐源打包为 `.ypm.zip`。安装器也接受兼容的 YesPlayMusic 插件 ZIP，支持根目录或一层外包装目录中的 `plugin.json`。本宿主实现了网易云使用的 API 子集，不承诺兼容所有 YesPlayMusic 扩展能力。

发布其他平台的播放器时，附件沿用 `MusicPlayer-平台-架构-版本` 命名，例如 `MusicPlayer-macos-arm64-0.2.19.dmg`、`MusicPlayer-linux-x64-0.2.19.AppImage`、`MusicPlayer-android-arm64-v8a-0.2.19.apk`。macOS 和 Android 通用包的架构可写 `universal`。检查更新会优先选择当前架构的包，再选通用包。

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
| `plugins/24bit-js` | 24bit 双库搜索、公开音轨及按站点额度下载 |
| `plugins/bilibili-js` | 哔哩哔哩视频搜索、音视频播放及扫码登录插件 |
| `plugins/audius-js` | Audius 公开曲库 JS 插件，支持简繁体搜索和按曲下载 |
| `plugins/incompetech-js` | Kevin MacLeod 开放曲库 JS 插件 |
| `plugins/netease-js` | 网易云 JS 插件，基于 MIT 上游源码 |
| `xtask` | JS 打包、联网探测及真实播放检查 |
| `android` | Android 文件选择器、媒体服务、Keystore 和安装包资源 |
| `scripts` | Windows 便携包与 Android APK 构建 |
| `docs` | 插件协议、验收记录与历史版本说明 |
| `.github/workflows` | 桌面构建测试与 Android APK 构建、签名校验 |
| `legacy`、`crates/plugin-sdk`、`plugins/netease` | 旧 WASM 实现留档；当前应用不加载它 |

`.ref` 仅作为参考，不参与构建。依赖精确版本见 `Cargo.lock`。

## 仓库约定

- 提交源代码、Slint 界面与图标、插件源码和授权文件、构建脚本、CI 配置及文档。
- 提交 `Cargo.lock`，固定应用和工作区依赖版本；`.cargo/config.toml` 中的 `xtask` 别名也随仓库维护。
- `target/`、`dist/`、原生库、可执行文件和插件压缩包属于生成产物，由 [.gitignore](.gitignore) 排除。
- `.ref/`、`.test-data/`、本地数据库、加密账号文件、日志和 `.env` 不入库；需要共享环境配置时使用不含凭据的 `.env.example`。
- [.gitattributes](.gitattributes) 统一 Git 文本换行，[.editorconfig](.editorconfig) 定义 UTF-8、缩进和行尾约定；Rust 格式以 `cargo fmt` 为准。

## 平台范围

正式包覆盖 Windows x64 和 Android ARM64。macOS 和 Linux 配置了桌面 CI。

### 自行构建 Android 正式包

安装 Python 3.11+、JDK 21、Android SDK Platform 36、Build Tools 36.0.0、NDK 30.0.16248370 和用于 bindgen 的主机 libclang。设置 `ANDROID_HOME`、`ANDROID_NDK_HOME` 和 `JAVA_HOME`，需要时用 `LIBCLANG_PATH` 指向主机 libclang 所在目录。

```sh
rustup target add aarch64-linux-android
python scripts/build-android.py
```

签名从环境变量 `MUSIC_ANDROID_KEYSTORE` 和 `MUSIC_ANDROID_STORE_PASSWORD` 读取。默认密钥别名为 `music-player`，可以通过 `MUSIC_ANDROID_KEY_ALIAS`、`MUSIC_ANDROID_KEY_PASSWORD` 指定其他别名或独立密码。请备份密钥与密码，后续覆盖升级须使用同一证书。脚本从 Cargo 版本生成 versionCode，校验 libmpv 下载哈希、APK 对齐和签名，正式包输出到 `dist/android/`。

界面提供明暗主题、封面卡片、歌曲列表、歌词和队列。Coverflow、媒体系统快捷键和插件市场尚未实现。

详见 [插件协议](docs/plugins.md)、[验收记录](docs/validation.md)、[第三方授权说明](THIRD_PARTY_NOTICES.md)。
