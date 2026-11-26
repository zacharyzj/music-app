# JavaScript 插件协议 v2

宿主为 `rquickjs 0.14.0`（QuickJS）。在线音乐源独立交付 `.ypm.zip`，不再编译 WASM。网易云源码来自 `.ref/ypm-plugin-netease`，保留 MIT 授权；其 ES 模块直接运行，宿主负责能力接口和应用数据转换。

## 包结构

```text
plugin.json
index.js
client.js
models.js
...其他相对引用的 .js 模块
LICENSE
```

清单使用 YPM 格式：`id`、`name`、`version`、`main`、`permissions.network`、`navigation`、`settings`。见 `plugins/netease-js/plugin.json`。`local` 为保留 ID；只允许小写字母、数字、点、下划线和连字符。`minAppVersion` 属于 YPM 的版本体系，本宿主不据此判断自身版本。

包大小及解压总大小各最多 32 MiB，256 个文件，单文件 4 MiB。拒绝重复文件、路径穿越、符号链接、多个清单。ZIP 不解压至文件系统；ES 模块只能从包内加载 `.js`。安装读取元数据并校验包结构，不执行模块；语法和导出错误会在调用时显示。

更新原子替换 `{id}.ypm.zip`，备份上一包为 `{id}.previous`，保留 KV。首次安装或权限扩大后需在应用中启用。卸载删除插件包、备份及该源加密数据。

## JavaScript 能力

| 能力 | 当前支持 |
|---|---|
| 模块 | `import` / `export`，包内相对引用，持久上下文 |
| 异步 | Promise、微任务、setTimeout / setInterval 和清除定时器 |
| HTTP | `fetch` GET/POST、字符串 body、headers、Response.text/json、Headers.get/getSetCookie |
| 超时 | AbortSignal.timeout；AbortController 的已取消状态检查 |
| 编码 | URLSearchParams 的构造、append/set/toString |
| 存储 | `host.storage.get/set/remove`，JSON 值，按插件 ID 隔离 |
| 加密 | `host.crypto.md5(text)`、`aesEcbEncrypt(key,text)`，AES-128-ECB/PKCS7、大写十六进制 |
| 设置 | `host.setting(key)`，应用当前提供音质选择 |
| 登录提示 | `host.signInPrompt({qr,text})` |
| 语言 | `host.language()` 返回 zh-CN，`host.t(text)` 保留中文原文 |

这些是适配接口，不是完整浏览器或 Node.js。没有 DOM、require、原生文件系统、进程或原生扩展。未开放 `host.openURL`、自定义菜单操作、多语言切换及任意设置表单。HTTP 在插件工作线程执行，不阻塞 Slint 主线程；Promise.all 不代表并行网络请求，进行中的阻塞 HTTP 不能由 JS AbortController 即时取消。

## 业务导出

支持 `search`、`suggest`、`home`、`browse`、`album`、`artist`、`playlist`、`playlists`、`liked`、`like`、`savedAlbums`、`saveAlbum`、`recent`、`stream`、`lyrics`、`account`、`signIn`、`signOut`。

搜索使用 YPM 的 `tracks/albums/artists/playlists`，宿主自动转换为应用中的 Item 和 ResourceKey，ID 保持字符串。源 ID 由应用覆盖，不能冒充其他源。`sections` 展平成列表；分段链接转换为页面按钮。云盘的下一页通过 `browse({page:"cloud:50"})` 等链接访问，不能把它当作 `offset/more` 累加页。

YPM 的 `signIn()` 可以持续等待扫码。宿主将它适配成应用的 `loginStart` / `loginPoll` / `loginCancel`：保留同一 JS Promise 和定时器，每两秒推进一次，转发二维码和状态，最多八分钟。取消会丢弃上下文，不执行挂起回调，保留原账号；重新登录会新建上下文。退出只重置该插件会话。

## 权限与资源

API 请求必须为清单允许的 HTTPS 域名；支持精确域名和 `*.example.com` 子域名规则，拒绝重定向、URL 用户信息和非 443 端口。封面不携带账号 Cookie。可选 `permissions.media` 限定媒体域名；网易云包已声明白名单。普通 YPM 包没有媒体声明时，仅接受域名形式的 HTTPS 地址，拒绝 IP 字面量、localhost 和 `.local`。

每个上下文内存 128 MiB、栈 1 MiB。连续 JS 执行片段约五秒后中断；单次宿主调用总时限六十秒，HTTP 最长二十秒且不超过剩余时限。消息 16 MiB，KV 8 MiB，定时器最多 1024 个。触发执行中断或超时会丢弃上下文。这里使用时间/内存限制，不使用 WASM fuel。

限制不等于独立进程隔离。DNS 解析到私网地址、libmpv 内部重定向及恶意媒体解码不在完整隔离范围内。一般定时器只在插件调用或登录轮询期间推进，不提供常驻后台插件服务。

## 旧数据迁移

继续使用同一系统凭据项及 AES-GCM 文件 `plugin-data/{id}.enc`。网易云原 `cookies/account/quality` 可直接读取，`device` 自动补入 `deviceId`。安装新版应直接覆盖导入，不能先卸载。旧 `.wasm` 不再由应用加载，协议保存在 `legacy/wasm-protocol.md`。

## 联机搜索和下载

插件在 `plugin.json` 的 `capabilities` 数组中声明 `download`，并通过 `permissions.media` 指定音频域名。插件导出 `download({ id })`，成功时返回 `{ url, size?, attribution? }`，失败时抛出可显示的错误。宿主只向此接口请求下载地址，校验 HTTPS 地址、重定向域名、文件长度和音频格式，再保存并索引文件。`attribution` 会随歌曲保存为 UTF-8 文本。

`anonymous` 表示该源无需登录。`tracksOnly` 表示本地搜索只在歌曲类型回退到该源。插件可在曲目中返回 `license`，在歌曲行显示许可名称。`searchAliases` 可携带歌名或歌手的其他写法，下载入库后支持按这些写法搜索。

插件清单可声明 `searchPageSize` 为正整数，默认 50。宿主按每个源自己的页长推进搜索偏移。哔哩哔哩使用 20。只读源可声明 `readOnly` 隐藏收藏操作，同时保留账号登录。

`stream({ id })` 返回 `{ url, referrer? }`。可选的 `referrer` 为音乐源已声明的 HTTPS 来源页，路径支持字母、数字和 `/._-~`。播放器仅对本曲目设置 HTTP Referer。哔哩哔哩在 API 请求中使用本插件加密保存的会话，并用 BV 号对应的视频页作为播放来源页。

曲目可以返回可选的布尔字段 `downloadable`。`false` 会在歌曲行显示「未开放下载」，`true` 允许点击下载。省略字段会沿用原来的行为，由插件在点击时判断。插件仍应在导出 `download({ id })` 时核对最新权限，播放权限另由曲目的 `unavailable` 控制。免登录筛选同时要求插件声明 `anonymous`、`download` 和媒体域名，并由曲目明确返回 `downloadable: true`。

不同插件的搜索在独立线程返回，同一插件的调用串行复用一个 QuickJS 会话。宿主按来源分别翻页。下载任务轮到执行或重试时才调用 `download` 获取最新地址。

`host.mediaAllowed(url)` 按当前插件的媒体域名声明检查 HTTPS 地址，插件可以用它从源提供的镜像中选择可用地址。

## 在线目录与发布

在「设置与插件 → 在线插件」可以读取 HTTPS JSON 索引，或直接输入单个插件 ZIP 地址。官方目录使用 [GitHub Release 的稳定下载地址](https://github.com/zacharyzj/music-app/releases/latest/download/plugins.json)。从索引安装时，程序校验 ZIP 的 SHA-256，以及包内清单的 ID、版本、API 和媒体域名。导入成功后，与本地 ZIP 使用同一套启停和更新流程。

`cargo xtask plugin` 在 `dist/` 生成五个独立插件包和 `plugins.json`。索引根对象含 `schema: 1` 和 `plugins` 数组。每条记录含 `id`、`name`、`version`、`description`、`url`、`sha256`、`network` 和 `media`。发布包使用带版本的绝对地址。自托管索引也可使用相对于索引的地址。

维护者推送 `plugins-v*` 标签会触发 `.github/workflows/publish-plugins.yml`，自动构建包并在同名 GitHub Release 上传索引与插件。播放器使用公开 HTTPS 下载地址，无需登录 GitHub。
