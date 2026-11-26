pub mod catalog;
mod package;
mod storage;
use aes::cipher::{BlockEncryptMut, KeyInit, block_padding::Pkcs7};
use anyhow::{Result, anyhow, bail, ensure};
use music_core::{MAX_MESSAGE, Request};
pub use package::{Package, allowed, bundle, install, install_with, manifest, media_allowed};
use rquickjs::{Context, Ctx, Function, Module, Runtime as JsRuntime};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};
pub use storage::Storage;

struct ModuleFiles(Rc<BTreeMap<String, Vec<u8>>>);
impl rquickjs::loader::Resolver for ModuleFiles {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<rquickjs::loader::ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        let resolved = if name.starts_with("./") || name.starts_with("../") {
            let mut parts: Vec<_> = base
                .rsplit_once('/')
                .map(|(d, _)| d.split('/').collect())
                .unwrap_or_default();
            for p in name.split('/') {
                match p {
                    "." => {}
                    ".." => {
                        if parts.pop().is_none() {
                            return Err(rquickjs::Error::new_resolving(base, name));
                        }
                    }
                    x => parts.push(x),
                }
            }
            parts.join("/")
        } else {
            name.to_owned()
        };
        if !package::safe_name(&resolved)
            || !resolved.ends_with(".js")
            || !self.0.contains_key(&resolved)
        {
            return Err(rquickjs::Error::new_resolving(base, name));
        }
        Ok(resolved)
    }
}
impl rquickjs::loader::Loader for ModuleFiles {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<rquickjs::loader::ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js>> {
        let code = self
            .0
            .get(name)
            .ok_or_else(|| rquickjs::Error::new_loading(name))?;
        Module::declare(ctx.clone(), name, code.as_slice())
    }
}
struct Host {
    storage: Storage,
    package: Package,
    client: reqwest::blocking::Client,
    prompt: Value,
    #[cfg(test)]
    fixture: Option<Value>,
}
impl Host {
    fn invoke(&mut self, v: Value, remaining: Duration) -> Result<Value> {
        let key = v["key"].as_str().unwrap_or("");
        match v["op"].as_str().unwrap_or("") {
            "get" => Ok(self.storage.get(key)),
            "set" => {
                self.storage.set(key, v["value"].clone())?;
                Ok(Value::Null)
            }
            "remove" => {
                self.storage.remove(key)?;
                Ok(Value::Null)
            }
            "mediaAllowed" => Ok(json!(media_allowed(
                v["url"].as_str().unwrap_or_default(),
                &self.package.manifest.media
            ))),
            "setting" => Ok(self.storage.get(key)),
            "prompt" => {
                self.prompt = v["prompt"].clone();
                Ok(Value::Null)
            }
            "md5" => Ok(json!(format!(
                "{:x}",
                md5::compute(v["text"].as_str().unwrap_or(""))
            ))),
            "aes" => {
                let key = v["key"].as_str().unwrap_or("").as_bytes();
                ensure!(key.len() == 16, "AES 密钥必须为 16 字节");
                let encrypted = ecb::Encryptor::<aes::Aes128>::new_from_slice(key)
                    .map_err(|_| anyhow!("AES 密钥无效"))?
                    .encrypt_padded_vec_mut::<Pkcs7>(v["text"].as_str().unwrap_or("").as_bytes());
                Ok(json!(hex::encode_upper(encrypted)))
            }
            "fetch" => {
                let url = v["url"].as_str().unwrap_or("");
                ensure!(
                    allowed(url, &self.package.manifest.network),
                    "插件无权访问此地址"
                );
                #[cfg(test)]
                if let Some(fixture) = self.fixture.as_mut() {
                    let path = url.split("/eapi/").nth(1).unwrap_or(url);
                    let value = fixture
                        .get_mut(path)
                        .ok_or_else(|| anyhow!("Unexpected fixture URL: {path}"))?;
                    let body = if let Some(values) =
                        value.get_mut("responses").and_then(Value::as_array_mut)
                    {
                        ensure!(!values.is_empty(), "Fixture exhausted");
                        values.remove(0)
                    } else {
                        value.clone()
                    };
                    if let Some(expected) = body["_expectCookie"].as_str() {
                        ensure!(
                            v["headers"]["Cookie"].as_str().unwrap_or("") == expected,
                            "Unexpected fixture cookie"
                        );
                    }
                    if let Some(expected) = body.get("_expectBody") {
                        ensure!(
                            v["method"] == "POST"
                                && serde_json::from_str::<Value>(v["body"].as_str().unwrap_or(""))?
                                    == *expected,
                            "Unexpected fixture POST body"
                        );
                    }
                    let content = body["_text"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| body.to_string());
                    return Ok(
                        json!({"status":200,"headers":{},"cookies":body.get("_cookies").cloned().unwrap_or(json!([])),"body":content}),
                    );
                }
                let method =
                    reqwest::Method::from_bytes(v["method"].as_str().unwrap_or("GET").as_bytes())?;
                ensure!(
                    [reqwest::Method::GET, reqwest::Method::POST].contains(&method),
                    "不支持此 HTTP 方法"
                );
                let mut req = self.client.request(method, url).timeout(
                    Duration::from_millis(v["timeout"].as_u64().unwrap_or(20000).clamp(1, 20000))
                        .min(remaining),
                );
                if let Some(headers) = v["headers"].as_object() {
                    for (k, val) in headers {
                        ensure!(
                            !["host", "content-length", "connection"]
                                .contains(&k.to_ascii_lowercase().as_str()),
                            "不允许覆盖此请求头"
                        );
                        req = req.header(k, val.as_str().unwrap_or(""));
                    }
                }
                if let Some(body) = v["body"].as_str() {
                    ensure!(body.len() <= MAX_MESSAGE, "请求过大");
                    req = req.body(body.to_owned());
                }
                let response = req.send().map_err(|e| {
                    let detail = anyhow::Error::new(e.without_url());
                    anyhow!("HTTPS 请求失败：{detail:#}")
                })?;
                let status = response.status().as_u16();
                ensure!(
                    !response.status().is_redirection(),
                    "插件请求不允许跨地址重定向"
                );
                let cookies: Vec<_> = response
                    .headers()
                    .get_all("set-cookie")
                    .iter()
                    .filter_map(|h| h.to_str().ok().map(str::to_owned))
                    .collect();
                let headers: BTreeMap<_, _> = response
                    .headers()
                    .iter()
                    .filter_map(|(k, v)| {
                        v.to_str()
                            .ok()
                            .map(|v| (k.as_str().to_owned(), v.to_owned()))
                    })
                    .collect();
                let mut body = String::new();
                response
                    .take(MAX_MESSAGE as u64 + 1)
                    .read_to_string(&mut body)?;
                ensure!(body.len() <= MAX_MESSAGE, "响应过大");
                Ok(json!({"status":status,"headers":headers,"cookies":cookies,"body":body}))
            }
            _ => bail!("宿主能力未开放"),
        }
    }
}
struct Session {
    context: Context,
    engine: JsRuntime,
    host: Rc<RefCell<Host>>,
    slice: Rc<Cell<Instant>>,
    deadline: Rc<Cell<Instant>>,
    serial: u64,
    login: Option<u64>,
    signature: String,
    root: PathBuf,
}
impl Session {
    fn new(
        package: Package,
        mut storage: Storage,
        signature: String,
        root: PathBuf,
    ) -> Result<Self> {
        // Preserve the previous WASM plugin's device identity and encrypted account.
        if storage.get("deviceId").is_null() && storage.get("device").is_string() {
            storage.set("deviceId", storage.get("device"))?;
        }
        let engine = JsRuntime::new()?;
        engine.set_memory_limit(128 * 1024 * 1024);
        engine.set_max_stack_size(1024 * 1024);
        let slice = Rc::new(Cell::new(Instant::now() + Duration::from_secs(5)));
        let deadline = Rc::new(Cell::new(Instant::now() + Duration::from_secs(60)));
        let check_slice = slice.clone();
        let check_deadline = deadline.clone();
        engine.set_interrupt_handler(Some(Box::new(move || {
            Instant::now() > check_slice.get() || Instant::now() > check_deadline.get()
        })));
        let files = Rc::new(package.files.clone());
        engine.set_loader(ModuleFiles(files.clone()), ModuleFiles(files));
        let main = package.main.clone();
        let host = Rc::new(RefCell::new(Host {
            storage,
            package,
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            prompt: Value::Null,
            #[cfg(test)]
            fixture: None,
        }));
        let context = Context::full(&engine)?;
        context.with(|ctx| -> Result<()> {
            let host = host.clone();
            let slice = slice.clone();
            let deadline = deadline.clone();
            ctx.globals().set(
                "__native",
                Function::new(ctx.clone(), move |input: String| -> String {
                    let result = (|| -> Result<Value> {
                        ensure!(Instant::now() < deadline.get(), "插件调用超时");
                        ensure!(input.len() <= MAX_MESSAGE, "请求过大");
                        host.borrow_mut().invoke(
                            serde_json::from_str(&input)?,
                            deadline.get().saturating_duration_since(Instant::now()),
                        )
                    })();
                    slice.set(Instant::now() + Duration::from_secs(5));
                    match result {
                        Ok(v) => json!({"value":v}).to_string(),
                        Err(e) => json!({"error":e.to_string(),"code":"host"}).to_string(),
                    }
                })?,
            )?;
            ctx.eval::<(), _>(include_str!("prelude.js"))
                .map_err(|e| js_error(&ctx, e))?;
            Module::evaluate(
                ctx.clone(),
                "@entry",
                format!(
                    "import * as p from {}; globalThis.__plugin=p;",
                    serde_json::to_string(&main)?
                ),
            )
            .map_err(|e| js_error(&ctx, e))?
            .finish::<()>()
            .map_err(|e| js_error(&ctx, e))?;
            Ok(())
        })?;
        Ok(Self {
            context,
            engine,
            host,
            slice,
            deadline,
            serial: 0,
            login: None,
            signature,
            root,
        })
    }
    fn begin(&self) {
        self.slice.set(Instant::now() + Duration::from_secs(5));
        self.deadline.set(Instant::now() + Duration::from_secs(60));
    }
    fn eval<T: for<'js> rquickjs::FromJs<'js>>(&self, code: &str) -> Result<T> {
        self.slice.set(Instant::now() + Duration::from_secs(5));
        self.context
            .with(|ctx| ctx.eval(code).map_err(|e| js_error(&ctx, e)))
    }
    fn start(&mut self, op: &str, args: Value) -> Result<u64> {
        self.serial += 1;
        let id = self.serial;
        let source = self.host.borrow().package.manifest.id.clone();
        self.eval::<()>(&format!(
            "__start({id},{},{},{})",
            serde_json::to_string(op)?,
            args,
            serde_json::to_string(&source)?
        ))?;
        Ok(id)
    }
    fn pump(&self) -> Result<()> {
        self.eval::<()>("__tick()")?;
        let until = Instant::now() + Duration::from_millis(100);
        while Instant::now() < until {
            if !self
                .engine
                .execute_pending_job()
                .map_err(|_| anyhow!("JavaScript 执行中断或任务失败"))?
            {
                break;
            }
        }
        Ok(())
    }
    fn result(&self, id: u64) -> Result<Value> {
        let text: String = self.eval(&format!(
            "JSON.stringify(__results[{id}]||{{pending:true}})"
        ))?;
        ensure!(text.len() <= MAX_MESSAGE, "插件返回消息过大");
        Ok(serde_json::from_str(&text)?)
    }
    fn take(&self, id: u64, r: Value) -> Result<Value> {
        self.eval::<()>(&format!("delete __results[{id}];"))?;
        if let Some(e) = r["error"].as_str() {
            bail!("{e}")
        }
        Ok(r["value"].clone())
    }
    fn call(&mut self, request: Request) -> Result<Value> {
        self.begin();
        match request.op.as_str() {
            "settings" => {
                let mut host = self.host.borrow_mut();
                if let Some(q) = request.args["quality"].as_str() {
                    host.storage.set("quality", json!(q))?;
                }
                return Ok(host.storage.get("quality"));
            }
            "loginStart" => {
                self.host.borrow_mut().prompt = Value::Null;
                self.login = Some(self.start("signIn", json!({}))?);
                self.pump()?;
                let prompt = self.host.borrow().prompt.clone();
                return Ok(
                    json!({"key":self.login.unwrap().to_string(),"qr":prompt["qr"],"message":prompt["text"]}),
                );
            }
            "loginPoll" => {
                let Some(id) = self.login else {
                    return Ok(json!({"status":"expired"}));
                };
                if request.args["key"].as_str() != Some(&id.to_string()) {
                    return Ok(json!({"status":"expired"}));
                }
                self.pump()?;
                let result = self.result(id)?;
                if result["pending"] != true {
                    self.login = None;
                    return self
                        .take(id, result)
                        .map(|a| json!({"status":"success","account":a}));
                }
                let prompt = self.host.borrow().prompt.clone();
                return Ok(json!({"status":"waiting","qr":prompt["qr"],"message":prompt["text"]}));
            }
            _ => {}
        }
        let id = self.start(&request.op, request.args)?;
        loop {
            self.pump()?;
            let result = self.result(id)?;
            if result["pending"] != true {
                return self.take(id, result);
            }
            ensure!(Instant::now() < self.deadline.get(), "插件调用超时");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
fn js_error(ctx: &Ctx<'_>, e: rquickjs::Error) -> anyhow::Error {
    anyhow!("JavaScript: {}", rquickjs::CaughtError::from_error(ctx, e))
}

pub struct Runtime {
    sessions: RefCell<HashMap<String, Session>>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
impl Runtime {
    pub fn new() -> Self {
        Self {
            sessions: RefCell::new(HashMap::new()),
        }
    }
    pub fn validate(&self, bytes: &[u8]) -> Result<()> {
        Package::read(bytes)?;
        Ok(())
    }
    pub fn unload(&self, id: &str) {
        self.sessions.borrow_mut().remove(id);
    }
    pub fn call(&self, bytes: &[u8], root: &Path, request: Request) -> Result<Value> {
        let package = Package::read(bytes)?;
        let id = package.manifest.id.clone();
        if request.op == "loginCancel" {
            self.unload(&id);
            return Ok(Value::Null);
        }
        let signature = format!("{:x}", md5::compute(bytes));
        let mut sessions = self.sessions.borrow_mut();
        if request.op == "loginStart"
            || sessions
                .get(&id)
                .is_some_and(|s| s.signature != signature || s.root != root)
        {
            sessions.remove(&id);
        }
        if !sessions.contains_key(&id) {
            let storage = Storage::open(root, &id)?;
            sessions.insert(
                id.clone(),
                Session::new(package, storage, signature, root.into())?,
            );
        }
        let result = sessions.get_mut(&id).unwrap().call(request);
        if result.as_ref().is_err_and(|e| {
            e.to_string().contains("interrupted")
                || e.to_string().contains("中断")
                || e.to_string().contains("超时")
        }) {
            sessions.remove(&id);
        }
        result
    }
}

#[cfg(test)]
mod tests;
