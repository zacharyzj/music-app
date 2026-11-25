# 首版验收记录

日期：2026-11-25。环境：Windows x64、Rust 1.99.0、MSVC、Slint 1.19.1、libmpv bcde6b93cc。

## 0.1.1：登录后列表耗尽 fuel 修复

- 使用现有登录会话的临时加密副本，复现了云盘 `all fuel consumed by WebAssembly`：宿主序列化响应约 179 KB，原固定 1 亿 fuel 在列表转换时耗尽。
- 插件的数组访问改为借用，去掉完整响应数组及嵌套元数据的深拷贝。宿主按 HTTP body 字节数补充解析预算，整次调用累计最多 5 亿 fuel；没有 HTTP 响应的死循环仍只有初始 1 亿预算。
- 回归样本用合成数据覆盖 50 首歌曲、约 438 KB 的每日推荐和约 442 KB 的云盘响应，检查条目数、云盘 songId 与分页信息。旧版大列表样本失败；修复后通过。另检查反复补充预算的累计上限，保留死循环和越界测试。
- 真实云盘请求修复后返回 50 首、`more=true`。每日推荐的真实接口在本次探测中返回空列表；大列表情况用上述合成响应验证，不将空列表探测当作真实推荐内容验收。
- 0.1.1 更新需要使用新主程序并重新导入 `dist/netease.wasm`。插件 ID 和数据目录不变，原登录会话保留。

下表保留首版验收范围；0.1.1 新增的两项回归使测试总数变为 13 项。

## 已执行

| 检查 | 结果与范围 |
|---|---|
| `cargo fmt --all --check` | 通过 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 通过，包含界面快照辅助功能 |
| `cargo test --workspace -- --include-ignored` | 11 项通过；实际编译的网易云 WASM 参与宿主集成测试 |
| `cargo xtask plugin` | 产出独立 `dist/netease.wasm`；读取内嵌清单并验证导入、导出及 ABI |
| `cargo xtask probe 晴天` | 真实 EAPI 搜索返回 50 条，有下一页；无 Node 服务 |
| `cargo xtask probe 晴天 --play` | WASM 解析真实完整音频 URL，libmpv 播放进度超过 1 秒，总时长约 270.74 秒；发送暂停、定位及恢复命令后进度继续推进 |
| `cargo xtask play <本地 WAV>` | 4 秒中文文件名 WAV 播放进度超过 1 秒；发送暂停、定位、恢复命令 |
| 原生界面启动 | 在 Windows 上创建 Music Player 窗口 |
| Slint 界面与播放 | 应用自身调用 UI 回调并生成渲染快照；推荐卡片、封面、搜索列表、底栏均有实际渲染。真实搜索结果的首曲播放到 8.44 秒，`playing=true` |
| Android 共享代码 | `cargo +stable check -p music-core -p music-plugin-sdk -p netease --target aarch64-linux-android` 通过 |

自动测试包括：队列结尾与移除、LRC 多时间戳和偏移及大时间戳、本地歌单持久化与搜索、增量扫描/取消/离线目录保护、API 域名规则、错误模块、WASM 越界、禁止 start 函数、fuel 耗尽、EAPI 加密向量、ID 转换。实际 WASM 集成用受控 HTTP 响应检查搜索映射、拒绝试听并降级音质、二维码等待/取消/过期 key，并检查取消登录保留原账号。未以假响应冒充真实联网结果。

## 交付与复现

运行 `scripts/build-windows.ps1` 构建 release 并打包。主程序包为 `dist/MusicPlayer-windows-x64.zip`，网易云插件为 `dist/netease.wasm`，二者分开交付。主程序包不预装网易云，不包含 `.test-data` 或测试账号数据。

可通过 README 中的命令重跑检查。界面渲染辅助功能仅在显式启用 `ui-smoke` 时编译，不在正常交付包中：

```powershell
cargo build -p music-app --features ui-smoke
$env:MUSIC_API_DATA_DIR = "$PWD/.test-data"
$env:MUSIC_API_MPV = "$PWD/dist/native/mpv-2.dll"
$env:MUSIC_API_CAPTURE = "$PWD/.test-data/render.png"
# 在测试数据目录手动导入并启用插件后，可选择：
$env:MUSIC_API_CAPTURE_SOURCE = 'netease'
$env:MUSIC_API_CAPTURE_QUERY = '晴天'
$env:MUSIC_API_CAPTURE_PLAY = '1'
./target/debug/music-app.exe
```

15 秒后应用保存自身 Slint 渲染图并退出；开启 PLAY 会实际低音量播放。该路径检查 UI 回调、后台任务、真实播放及渲染，不等同于鼠标/键盘端到端操作。Windows 窗口自动化授权等待超时，因此未完成系统级点击验收。

## 尚待验收或适配

- 没有使用个人账号扫码完成真实登录；个人收藏增删、云盘、账号切换、会员/地区受限曲目需实际账号验收。二维码状态逻辑和存储隔离已有代码与部分契约测试。
- macOS/Linux 构建及 Android 完整 Slint/NDK 构建已配置 CI，但未在远端执行。当前机器未配置 Android NDK，只有共享纯 Rust 代码完成本地交叉检查。
- Android 仍需 SAF、Keystore、音频焦点、后台服务与 libmpv 打包，不交付 APK。
- 当前为首版界面，参考 YesPlayMusic 的布局与音乐浏览流程；Coverflow、完整动效、媒体系统快捷键和托盘未实现。
- 本地索引支持启动扫描和手动重扫，未实现文件系统实时监听。当前扫描、源请求在后台串行执行，超大音乐库的性能尚未压测。
- 宿主约束插件 API 请求和初始媒体 URL；libmpv 内部媒体重定向不由 WASM HTTP 白名单拦截。没有声明对任意不可信媒体内容进行完整网络隔离。
