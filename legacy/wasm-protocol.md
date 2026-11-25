# 单文件插件协议 v1

插件构建为 Rust `cdylib`，目标 `wasm32-unknown-unknown`。没有 WASI、浏览器 JS 或原生系统导入。

清单嵌入名为 `music_manifest` 的 custom section，格式参见 `plugins/netease/manifest.json`。`id=local` 保留。安装时读取清单并验证 ABI=1、ID 和 Wasm 模块；不执行启动函数。

## 内存与调用约定

| 方向 | 名称 | 类型 |
|---|---|---|
| 导出 | memory | 32 位线性内存 |
| 导出 | music_alloc | `(len:i32) -> ptr:i32` |
| 导出 | music_free | `(ptr:i32,len:i32) -> ()` |
| 导出 | music_call | `(ptr:i32,len:i32) -> packed:i64` |
| 导入 music_host_v1 | request | `(ptr:i32,len:i32) -> response_len:i32` |
| 导入 music_host_v1 | response | `(ptr:i32,len:i32) -> copied_len:i32` |

宿主先分配输入，写入 UTF-8 JSON，调用 music_call，然后释放输入。结果低 32 位是指针，高 32 位是长度；宿主读取并释放结果。所有释放必须使用原长度。SDK 使用 boxed slice 保证对应的内存布局。

输入为 `{"op":"search","args":{"query":"晴天","kind":"track","offset":0}}`。输出为 serde Result：`{"Ok":...}` 或 `{"Err":{"code":"signIn","message":"请先登录","retryable":false}}`。宿主导入返回同样的 Result 封装。一个 request 对应一次 response，避免重复执行有副作用的宿主请求。

公共数据使用 `music-core` 的 Item、ResourceKey、Page。Item.kind 为 track/album/artist/playlist，所有 ID 使用字符串。页面分页返回 offset/more；宿主覆盖资源 source 字段，不能冒充另一个源。

## 宿主服务

`time` 返回 Unix 毫秒；`random` 返回宿主随机整数；`get/set` 操作插件独立 KV。`http` 接受 HTTPS URL、headers 和字符串 body，返回 status、全部 Set-Cookie 和 body。仅允许声明的 API 主机，拒绝重定向、用户信息和非 HTTPS URL。封面请求不携带插件 Cookie。媒体 URL另受 media 域名限制。

每次调用使用新实例，持久状态必须放入 KV。限制为：模块 32 MiB、实例内存 64 MiB、消息 16 MiB、KV 8 MiB、宿主调用整体 60 秒、单次 HTTP 最长 20 秒。无文件系统和进程 API。

执行预算初始为 100,000,000 fuel。每次成功取得 HTTP 响应，按 body 的 UTF-8 字节数额外分配每字节 1,000 fuel，用于 JSON 解析、映射和释放；整个插件调用累计最多获配 500,000,000 fuel。上限计算累计发放量，而非当前剩余量，重复 HTTP 调用不会无限续充。KV、时间和随机数调用不补充预算。这样较大的正常列表不与无限循环共用一个过小的固定预算。

标准操作包括 search、home、browse、album、artist、playlist、playlists、stream、lyrics、liked、like、savedAlbums、saveAlbum、recent。只在 capabilities 中声明真正实现的操作。

登录拆为 loginStart、loginPoll、loginCancel、account、signOut。loginStart 返回 key/qr；轮询传 key，返回 status：waiting/scanned/expired/success/failed。账号与二维码状态由插件维护；退出只删除本插件凭据。

网易云例子中的 AES/MD5 实现位于 guest 内；宿主不包含网易云加密或端点。每次 release 需要递增清单版本，重建 `.wasm` 后手动导入更新。更新保留 KV，卸载删除当前模块与 KV。
