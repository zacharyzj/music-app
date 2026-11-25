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
