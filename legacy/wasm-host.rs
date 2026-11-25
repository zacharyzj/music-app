use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, OsRng, rand_core::RngCore},
};
use anyhow::{Result, anyhow, bail};
use music_core::*;
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wasmi::{Caller, Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder};

const INITIAL_FUEL: u64 = 100_000_000;
const MAX_CALL_FUEL: u64 = 500_000_000;
const HTTP_RESPONSE_FUEL_PER_BYTE: u64 = 1_000;

struct FuelBudget {
    granted: u64,
}
impl FuelBudget {
    fn new() -> Self {
        Self {
            granted: INITIAL_FUEL,
        }
    }
    fn http_response_credit(&mut self, bytes: usize) -> u64 {
        // Account for JSON decoding, mapping and dropping large HTTP responses.
        // Cap total granted fuel, not remaining fuel: repeated host calls cannot
        // keep an otherwise infinite guest alive by continually refilling it.
        let credit = (bytes as u64)
            .saturating_mul(HTTP_RESPONSE_FUEL_PER_BYTE)
            .min(MAX_CALL_FUEL - self.granted);
        self.granted += credit;
        credit
    }
}

pub fn manifest(bytes: &[u8]) -> Result<Manifest> {
    if bytes.len() > 32 * 1024 * 1024 {
        bail!("插件超过 32 MiB");
    }
    let mut found = None;
    for p in wasmparser::Parser::new(0).parse_all(bytes) {
        if let wasmparser::Payload::CustomSection(s) = p?
            && s.name() == "music_manifest"
        {
            if found.is_some() {
                bail!("重复清单");
            }
            found = Some(serde_json::from_slice::<Manifest>(s.data())?);
        }
    }
    let m = found.ok_or_else(|| anyhow!("缺少 music_manifest 清单"))?;
    if m.abi != ABI_VERSION
        || m.id == "local"
        || m.id.is_empty()
        || m.id.len() > 64
        || !m
            .id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
    {
        bail!("插件 ID 或 ABI 不兼容");
    }
    Ok(m)
}
pub fn allowed(url: &str, patterns: &[String]) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some_and(|p| p != 443)
    {
        return false;
    }
    let Some(host) = u.host_str() else {
        return false;
    };
    patterns.iter().any(|p| {
        p == host
            || p.strip_prefix("*.")
                .is_some_and(|s| host.ends_with(&format!(".{s}")))
    })
}
pub fn install(file: &Path, dir: &Path) -> Result<Manifest> {
    let bytes = std::fs::read(file)?;
    let m = manifest(&bytes)?;
    let rt = Runtime::new();
    rt.validate(&bytes)?;
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.wasm", m.id));
    let tmp = dir.join(format!("{}.pending", m.id));
    std::fs::write(&tmp, bytes)?;
    // Windows rename cannot replace an existing file. Preserve a recoverable backup.
    let backup = path.with_extension("previous");
    if path.exists() {
        if backup.exists() {
            std::fs::remove_file(&backup)?;
        }
        std::fs::rename(&path, &backup)?;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        if backup.exists() {
            let _ = std::fs::rename(&backup, &path);
        }
        return Err(e.into());
    }
    Ok(m)
}

struct State {
    memory: Option<Memory>,
    response: Vec<u8>,
    limits: StoreLimits,
    manifest: Manifest,
    kv: Value,
    path: PathBuf,
    key: [u8; 32],
    deadline: Instant,
    fuel_budget: FuelBudget,
    #[cfg(test)]
    fixture: Option<Value>,
}
fn crypt_key() -> Result<[u8; 32]> {
    let entry = keyring::Entry::new("music-api", "plugin-storage-key")?;
    let key = match entry.get_password() {
        Ok(s) => hex::decode(s)?,
        Err(keyring::Error::NoEntry) => {
            let mut key = [0; 32];
            OsRng.fill_bytes(&mut key);
            entry.set_password(&hex::encode(key))?;
            key.to_vec()
        }
        Err(e) => return Err(e.into()),
    };
    key.try_into().map_err(|_| anyhow!("凭据密钥无效"))
}
fn load_kv(path: &Path, key: &[u8; 32]) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let data = std::fs::read(path)?;
    if data.len() < 12 {
        bail!("插件存储损坏");
    }
    let cipher = Aes256Gcm::new_from_slice(key).unwrap();
    let plain = cipher
        .decrypt((&data[..12]).into(), &data[12..])
        .map_err(|_| anyhow!("无法解密插件存储"))?;
    Ok(serde_json::from_slice(&plain)?)
}
fn save_kv(s: &State) -> Result<()> {
    let plain = serde_json::to_vec(&s.kv)?;
    if plain.len() > 8 * 1024 * 1024 {
        bail!("插件存储超过限制");
    }
    let mut nonce = [0; 12];
    OsRng.fill_bytes(&mut nonce);
    let mut data = nonce.to_vec();
    data.extend(
        Aes256Gcm::new_from_slice(&s.key)
            .unwrap()
            .encrypt((&nonce).into(), plain.as_slice())
            .map_err(|_| anyhow!("加密失败"))?,
    );
    use std::io::Write;
    let mut pending = tempfile::NamedTempFile::new_in(
        s.path
            .parent()
            .ok_or_else(|| anyhow!("Invalid storage directory"))?,
    )?;
    pending.write_all(&data)?;
    pending.as_file().sync_all()?;
    pending.persist(&s.path)?;
    Ok(())
}
fn handle(s: &mut State, v: Value) -> Result<Value> {
    if Instant::now() > s.deadline {
        bail!("插件请求超时");
    }
    match v["op"].as_str().unwrap_or("") {
        "time" => Ok(json!(
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64
        )),
        "random" => Ok(json!(rand::random::<u64>())),
        "get" => Ok(s.kv[v["key"].as_str().unwrap_or("")].clone()),
        "set" => {
            let k = v["key"].as_str().ok_or_else(|| anyhow!("缺少 key"))?;
            s.kv[k] = v["value"].clone();
            save_kv(s)?;
            Ok(Value::Null)
        }
        "http" => {
            let url = v["url"].as_str().ok_or_else(|| anyhow!("缺少 URL"))?;
            if !allowed(url, &s.manifest.network) {
                bail!("插件无权访问此地址");
            }
            #[cfg(test)]
            if let Some(fixture) = s.fixture.as_mut() {
                let path = url.split("/eapi/").nth(1).unwrap_or("");
                let value = fixture
                    .get_mut(path)
                    .ok_or_else(|| anyhow!("unexpected fixture URL: {path}"))?;
                let response = if let Some(responses) =
                    value.get_mut("responses").and_then(Value::as_array_mut)
                {
                    if responses.is_empty() {
                        bail!("fixture exhausted");
                    }
                    responses.remove(0)
                } else {
                    value.clone()
                };
                return Ok(
                    json!({"status":200,"cookies":response.get("_cookies").cloned().unwrap_or(json!([])),"body":response.to_string()}),
                );
            }
            let client = reqwest::blocking::Client::builder()
                .timeout(
                    Duration::from_secs(20)
                        .min(s.deadline.saturating_duration_since(Instant::now())),
                )
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let mut req = client.post(url);
            if let Some(headers) = v["headers"].as_object() {
                for (k, value) in headers {
                    req = req.header(k, value.as_str().unwrap_or(""));
                }
            }
            let response = req
                .body(v["body"].as_str().unwrap_or("").to_owned())
                .send()?;
            let status = response.status().as_u16();
            let cookies: Vec<_> = response
                .headers()
                .get_all("set-cookie")
                .iter()
                .filter_map(|x| x.to_str().ok().map(str::to_string))
                .collect();
            let mut body = String::new();
            response
                .take(MAX_MESSAGE as u64 + 1)
                .read_to_string(&mut body)?;
            if body.len() > MAX_MESSAGE {
                bail!("响应过大");
            }
            Ok(json!({"status":status,"cookies":cookies,"body":body}))
        }
        _ => bail!("未知宿主操作"),
    }
}
pub struct Runtime {
    engine: Engine,
    #[cfg(test)]
    fixture: Option<Value>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
impl Runtime {
    pub fn new() -> Self {
        let mut cfg = wasmi::Config::default();
        cfg.consume_fuel(true);
        cfg.allow_start_fn(false);
        Self {
            engine: Engine::new(&cfg),
            #[cfg(test)]
            fixture: None,
        }
    }
    pub fn validate(&self, bytes: &[u8]) -> Result<()> {
        use wasmi::{ExternType, ValType};
        let module = Module::new(&self.engine, bytes)?;
        let host_type = wasmi::FuncType::new([ValType::I32, ValType::I32], [ValType::I32]);
        for import in module.imports() {
            if import.module() != "music_host_v1"
                || !["request", "response"].contains(&import.name())
                || !matches!(import.ty(),ExternType::Func(t) if t==&host_type)
            {
                bail!("不支持的插件导入：{}::{}", import.module(), import.name());
            }
        }
        for (name, params, results) in [
            ("music_alloc", vec![ValType::I32], vec![ValType::I32]),
            ("music_free", vec![ValType::I32, ValType::I32], vec![]),
            (
                "music_call",
                vec![ValType::I32, ValType::I32],
                vec![ValType::I64],
            ),
        ] {
            let export = module
                .exports()
                .find(|e| e.name() == name)
                .ok_or_else(|| anyhow!("缺少 {name}"))?;
            if !matches!(export.ty(),ExternType::Func(t) if t==&wasmi::FuncType::new(params,results))
            {
                bail!("插件 ABI 签名不兼容：{name}");
            }
        }
        if !module
            .exports()
            .any(|e| e.name() == "memory" && matches!(e.ty(), ExternType::Memory(_)))
        {
            bail!("缺少 memory");
        }
        Ok(())
    }
    pub fn call(&self, bytes: &[u8], root: &Path, request: Request) -> Result<Value> {
        self.validate(bytes)?;
        let m = manifest(bytes)?;
        if !m.capabilities.contains(&request.op) {
            bail!("插件不支持 {}", request.op);
        }
        std::fs::create_dir_all(root)?;
        let path = root.join(format!("{}.enc", m.id));
        let key = crypt_key()?;
        let kv = load_kv(&path, &key)?;
        self.execute(bytes, m, kv, path, key, request)
    }
    fn execute(
        &self,
        bytes: &[u8],
        m: Manifest,
        kv: Value,
        path: PathBuf,
        key: [u8; 32],
        request: Request,
    ) -> Result<Value> {
        let module = Module::new(&self.engine, bytes)?;
        let mut store = Store::new(
            &self.engine,
            State {
                memory: None,
                response: Vec::new(),
                limits: StoreLimitsBuilder::new()
                    .memory_size(64 * 1024 * 1024)
                    .instances(1)
                    .memories(1)
                    .build(),
                manifest: m,
                kv,
                path,
                key,
                deadline: Instant::now() + Duration::from_secs(60),
                fuel_budget: FuelBudget::new(),
                #[cfg(test)]
                fixture: self.fixture.clone(),
            },
        );
        store.limiter(|s| &mut s.limits);
        store.set_fuel(INITIAL_FUEL)?;
        let mut linker = Linker::new(&self.engine);
        linker.func_wrap(
            "music_host_v1",
            "request",
            |mut c: Caller<'_, State>, ptr: u32, len: u32| -> i32 {
                if len as usize > MAX_MESSAGE {
                    return -1;
                }
                let Some(mem) = c.data().memory else {
                    return -1;
                };
                let mut bytes = vec![0; len as usize];
                if mem.read(&c, ptr as usize, &mut bytes).is_err() {
                    return -1;
                }
                let mut http_response = false;
                let result: SourceResult = serde_json::from_slice::<Value>(&bytes)
                    .map_err(anyhow::Error::from)
                    .and_then(|v| {
                        http_response = v["op"] == "http";
                        handle(c.data_mut(), v)
                    })
                    .map_err(|e| SourceError::new("host", e.to_string()));
                let response_bytes = if http_response {
                    result
                        .as_ref()
                        .ok()
                        .and_then(|v| v["body"].as_str())
                        .map_or(0, str::len)
                } else {
                    0
                };
                if response_bytes > 0 {
                    let credit = c
                        .data_mut()
                        .fuel_budget
                        .http_response_credit(response_bytes);
                    let remaining = c.get_fuel().unwrap_or(0);
                    if c.set_fuel(remaining.saturating_add(credit)).is_err() {
                        return -1;
                    }
                }
                c.data_mut().response = serde_json::to_vec(&result).unwrap();
                if std::env::var_os("MUSIC_API_TRACE").is_some() {
                    eprintln!(
                        "WASM host: response_bytes={}, remaining_fuel={}",
                        c.data().response.len(),
                        c.get_fuel().unwrap_or(0)
                    );
                }
                c.data().response.len() as i32
            },
        )?;
        linker.func_wrap(
            "music_host_v1",
            "response",
            |mut c: Caller<'_, State>, ptr: u32, len: u32| -> i32 {
                let data = std::mem::take(&mut c.data_mut().response);
                if data.len() != len as usize {
                    return -1;
                }
                let Some(mem) = c.data().memory else {
                    return -1;
                };
                if mem.write(&mut c, ptr as usize, &data).is_err() {
                    return -1;
                }
                len as i32
            },
        )?;
        let instance = linker.instantiate_and_start(&mut store, &module)?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| anyhow!("缺少 memory"))?;
        store.data_mut().memory = Some(memory);
        let alloc = instance.get_typed_func::<u32, u32>(&store, "music_alloc")?;
        let free = instance.get_typed_func::<(u32, u32), ()>(&store, "music_free")?;
        let call = instance.get_typed_func::<(u32, u32), u64>(&store, "music_call")?;
        let input = serde_json::to_vec(&request)?;
        if input.len() > MAX_MESSAGE {
            bail!("请求过大");
        }
        let ptr = alloc.call(&mut store, input.len() as u32)?;
        memory.write(&mut store, ptr as usize, &input)?;
        let result = call.call(&mut store, (ptr, input.len() as u32))?;
        free.call(&mut store, (ptr, input.len() as u32))?;
        let (ptr, len) = (result as u32, (result >> 32) as u32);
        if len as usize > MAX_MESSAGE {
            bail!("响应过大");
        }
        let mut output = vec![0; len as usize];
        memory.read(&store, ptr as usize, &mut output)?;
        free.call(&mut store, (ptr, len))?;
        let result: SourceResult = serde_json::from_slice(&output)?;
        if std::env::var_os("MUSIC_API_TRACE").is_some() {
            eprintln!(
                "WASM finished: granted_fuel={}, remaining_fuel={}",
                store.data().fuel_budget.granted,
                store.get_fuel()?
            );
        }
        Ok(result?)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_fuel_is_capped_across_all_host_calls() {
        let mut budget = FuelBudget::new();
        let credit = budget.http_response_credit(180_000);
        assert_eq!(credit, 180_000_000);
        let mut total = INITIAL_FUEL + credit;
        for _ in 0..100 {
            total += budget.http_response_credit(180_000);
        }
        assert_eq!(total, MAX_CALL_FUEL);
        assert_eq!(budget.http_response_credit(usize::MAX), 0);
        let mut budget = FuelBudget::new();
        assert_eq!(
            budget.http_response_credit(usize::MAX),
            MAX_CALL_FUEL - INITIAL_FUEL
        );
    }
    #[test]
    fn hosts() {
        let p = vec!["*.music.126.net".into()];
        assert!(allowed("https://m7.music.126.net/a", &p));
        for s in [
            "http://m7.music.126.net/a",
            "https://music.126.net/a",
            "https://m7.music.126.net.evil/a",
            "file:///etc/passwd",
            "https://x@m7.music.126.net/a",
        ] {
            assert!(!allowed(s, &p));
        }
    }
    #[test]
    fn invalid_module() {
        assert!(manifest(b"broken").is_err());
        assert!(Runtime::new().validate(b"broken").is_err());
    }
    fn test_manifest() -> Manifest {
        serde_json::from_str(include_str!("../../../plugins/netease/manifest.json")).unwrap()
    }
    #[test]
    fn rejects_start_and_exhausts_fuel() {
        let rt = Runtime::new();
        assert!(
            rt.validate(&wat::parse_str("(module (func $start) (start $start))").unwrap())
                .is_err()
        );
        let wasm=wat::parse_str(r#"(module (memory (export "memory") 1)
          (func (export "music_alloc") (param i32) (result i32) i32.const 0)
          (func (export "music_free") (param i32 i32))
          (func (export "music_call") (param i32 i32) (result i64) (loop $loop br $loop) i64.const 0))"#).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let result = rt.execute(
            &wasm,
            test_manifest(),
            json!({}),
            dir.path().join("kv"),
            [0; 32],
            Request::new("search", json!({})),
        );
        assert!(result.unwrap_err().to_string().contains("fuel"));
    }
    #[test]
    fn rejects_guest_out_of_bounds_response() {
        let wasm = wat::parse_str(
            r#"(module (memory (export "memory") 1)
          (func (export "music_alloc") (param i32) (result i32) i32.const 0)
          (func (export "music_free") (param i32 i32))
          (func (export "music_call") (param i32 i32) (result i64) i64.const 8589934591))"#,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Runtime::new()
                .execute(
                    &wasm,
                    test_manifest(),
                    json!({}),
                    dir.path().join("kv"),
                    [0; 32],
                    Request::new("search", json!({}))
                )
                .is_err()
        );
    }
    #[test]
    #[ignore = "Build dist/netease.wasm with cargo xtask plugin first"]
    fn netease_large_account_pages() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../dist/netease.wasm"
        ))
        .unwrap();
        // Nested metadata approximates full song/privilege responses, not just the few
        // fields our UI consumes. No real account data is stored in this fixture.
        let songs: Vec<_> = (0..50).map(|i| json!({
            "id": i + 1, "name": format!("测试歌曲 {i}"), "dt": 240000,
            "ar": [{"id": 1, "name": "测试歌手"}],
            "al": {"id": 2, "name": "测试专辑", "picUrl": "https://p1.music.126.net/test.jpg"},
            "privilege": {"st": 0, "fee": 8, "pl": 320000, "fl": 320000},
            "metadata": (0..48).map(|n| json!({"id": n, "description": "完整歌曲元数据".repeat(6), "flag": true, "extra": null})).collect::<Vec<_>>()
        })).collect();
        let cloud: Vec<_> = songs.iter().enumerate().map(|(i, song)| json!({
            "songId": 1000 + i, "songName": song["name"], "artist": "云盘歌手", "simpleSong": song
        })).collect();
        for (page, path, response) in [
            (
                "daily",
                "v3/discovery/recommend/songs",
                json!({"code":200,"data":{"dailySongs":songs}}),
            ),
            (
                "cloud",
                "v1/cloud/get",
                json!({"code":200,"data":cloud,"hasMore":true,"count":120}),
            ),
        ] {
            eprintln!("{page} fixture: {} bytes", response.to_string().len());
            let dir = tempfile::tempdir().unwrap();
            let mut rt = Runtime::new();
            rt.fixture = Some(json!({path: response}));
            let result = rt
                .execute(
                    &bytes,
                    manifest(&bytes).unwrap(),
                    json!({"account":{"id":1},"device":"test"}),
                    dir.path().join("kv"),
                    [0; 32],
                    Request::new("browse", json!({"page":page,"offset":0})),
                )
                .unwrap_or_else(|e| panic!("{page}: {e}"));
            assert_eq!(result["items"].as_array().unwrap().len(), 50);
            assert_eq!(
                result["items"][0]["key"]["id"],
                if page == "cloud" { "1000" } else { "1" }
            );
            assert_eq!(result["more"], page == "cloud");
        }
    }
    #[test]
    #[ignore = "Build dist/netease.wasm with cargo xtask plugin first"]
    fn netease_wasm_contract() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../dist/netease.wasm"
        ))
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut rt = Runtime::new();
        rt.fixture = Some(
            json!({"cloudsearch/pc":{"code":200,"result":{"songs":[{"id":7,"name":"测试曲","ar":[{"name":"作者"}],"dt":1234}],"songCount":51}}}),
        );
        let p = rt
            .execute(
                &bytes,
                manifest(&bytes).unwrap(),
                json!({}),
                dir.path().join("kv"),
                [0; 32],
                Request::new("search", json!({"query":"test"})),
            )
            .unwrap();
        assert_eq!(p["items"][0]["key"]["id"], "7");
        assert_eq!(p["items"][0]["duration_ms"], 1234);
        assert_eq!(p["more"], true);
        rt.fixture = Some(
            json!({"song/enhance/player/url/v1":{"responses":[{"code":200,"data":[{"url":"https://m7.music.126.net/trial","freeTrialInfo":{"start":0}}]},{"code":200,"data":[{"url":"https://m7.music.126.net/full","freeTrialInfo":null}]}]}}),
        );
        let p = rt
            .execute(
                &bytes,
                manifest(&bytes).unwrap(),
                json!({}),
                dir.path().join("kv"),
                [0; 32],
                Request::new("stream", json!({"id":"7","quality":"high"})),
            )
            .unwrap();
        assert_eq!(p["quality"], "standard");
        assert_eq!(p["url"], "https://m7.music.126.net/full");
        let path = dir.path().join("login");
        let key = [0; 32];
        rt.fixture = Some(json!({"login/qrcode/unikey":{"code":200,"unikey":"test-key"}}));
        let started = rt
            .execute(
                &bytes,
                manifest(&bytes).unwrap(),
                json!({"account":{"id":1}}),
                path.clone(),
                key,
                Request::new("loginStart", json!({})),
            )
            .unwrap();
        assert_eq!(started["key"], "test-key");
        rt.fixture = Some(json!({"login/qrcode/client/login":{"code":801}}));
        let waiting = rt
            .execute(
                &bytes,
                manifest(&bytes).unwrap(),
                load_kv(&path, &key).unwrap(),
                path.clone(),
                key,
                Request::new("loginPoll", json!({"key":"test-key"})),
            )
            .unwrap();
        assert_eq!(waiting["status"], "waiting");
        rt.execute(
            &bytes,
            manifest(&bytes).unwrap(),
            load_kv(&path, &key).unwrap(),
            path.clone(),
            key,
            Request::new("loginCancel", json!({})),
        )
        .unwrap();
        assert_eq!(load_kv(&path, &key).unwrap()["account"]["id"], 1);
        let expired = rt
            .execute(
                &bytes,
                manifest(&bytes).unwrap(),
                load_kv(&path, &key).unwrap(),
                path.clone(),
                key,
                Request::new("loginPoll", json!({"key":"old-key"})),
            )
            .unwrap();
        assert_eq!(expired["status"], "expired");
    }
}
