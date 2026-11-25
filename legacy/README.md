# WASM 版本留档

`wasm-host.rs` 与 `wasm-protocol.md` 保存 0.1.1 的 Wasmi 宿主及协议。旧插件源码在 `plugins/netease`，SDK 在 `crates/plugin-sdk`，仍可独立查阅。0.2 的 `music-app` 只使用 `crates/plugin-host` 的 QuickJS 实现和 `.ypm.zip`；不再加载 `.wasm`，也不需要安装 wasm32 构建目标。
