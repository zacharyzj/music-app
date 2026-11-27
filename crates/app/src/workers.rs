use super::{Job, Reply, download, plugin_list};
use anyhow::{Result, anyhow, ensure};
use music_core::{Item, MusicSource, Request};
use music_library::Library;
use music_plugin_host::{Runtime, manifest};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

pub(super) struct Workers {
    pub root: PathBuf,
    pub library: Arc<Mutex<Library>>,
    pub generation: Arc<AtomicU64>,
    pub out: mpsc::Sender<Reply>,
    pub download_tx: mpsc::Sender<download::Transfer>,
}

struct PluginWorker {
    tx: mpsc::Sender<Job>,
    retired: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

impl Workers {
    pub fn start(self, jobs: mpsc::Receiver<Job>, cancel: Arc<AtomicBool>) {
        let ctx = Arc::new(self);
        thread::spawn(move || {
            let mut plugins = HashMap::new();
            for job in jobs {
                if ctx.obsolete(&job) {
                    continue;
                }
                if job.source == "local" {
                    let result = ctx.local(&job.request, &cancel, &mut plugins);
                    ctx.reply(job, result);
                    continue;
                }
                let worker = plugins
                    .entry(job.source.clone())
                    .or_insert_with(|| ctx.spawn_plugin());
                if let Err(error) = worker.tx.send(job) {
                    ctx.reply(error.0, Err(anyhow!("音乐源线程已停止")));
                }
            }
        });
    }

    fn obsolete(&self, job: &Job) -> bool {
        job.generation != self.generation.load(Ordering::Relaxed)
            && match job.request.op.as_str() {
                "search" | "home" | "browse" | "album" | "artist" | "playlist" | "loginPoll" => {
                    true
                }
                "isLiked" => job.generation != super::favorites::TRACK_SCOPE,
                _ => false,
            }
    }

    fn spawn_plugin(self: &Arc<Self>) -> PluginWorker {
        let ctx = self.clone();
        let (tx, jobs) = mpsc::channel::<Job>();
        let retired = Arc::new(AtomicBool::new(false));
        let stop = retired.clone();
        let thread = thread::spawn(move || {
            let runtime = Runtime::new();
            for job in jobs {
                if ctx.obsolete(&job) {
                    continue;
                }
                let result = if stop.load(Ordering::Acquire) {
                    Err(anyhow!("音乐源已更改"))
                } else {
                    ctx.plugin(&runtime, &job.source, &job.request)
                };
                ctx.reply(job, result);
            }
        });
        PluginWorker {
            tx,
            retired,
            thread,
        }
    }

    fn reply(&self, job: Job, result: Result<Value>) {
        let _ = self.out.send(Reply {
            source: job.source,
            op: job.request.op,
            args: job.request.args,
            generation: job.generation,
            value: result.map_err(|error| error.to_string()),
        });
    }

    fn local(
        &self,
        request: &Request,
        cancel: &AtomicBool,
        plugins: &mut HashMap<String, PluginWorker>,
    ) -> Result<Value> {
        let op = request.op.as_str();
        let args = &request.args;
        let root = &self.root;
        let data = root.join("plugin-data");
        if matches!(op, "remove" | "toggle") {
            retire(plugins, args["id"].as_str().unwrap_or(""));
        }
        let mut db = self.library.lock().map_err(|_| anyhow!("音乐库不可用"))?;
        if op == "import-targets" {
            return Ok(json!(db.playlist_choices()?));
        }
        if op == "import-replace" {
            let item: Item = serde_json::from_value(args["track"].clone())?;
            ensure!(
                plugin_list(&root.join("plugins"), &db)
                    .iter()
                    .any(|(m, on)| *on && m.id == item.key.source),
                "替选歌曲的来源未安装或未启用"
            );
            db.replace_unavailable(
                args["id"].as_str().unwrap_or_default(),
                &serde_json::from_value(args["original"].clone())?,
                item,
            )?;
            return Ok(Value::Null);
        }
        if op == "persist-downloads" {
            db.set("download-queue", args.clone())?;
            return Ok(Value::Null);
        }
        if op == "persist" {
            db.set("queue", args["queue"].clone())?;
            db.set("dark", args["dark"].clone())?;
            return Ok(Value::Null);
        }
        if op == "scan" {
            cancel.store(false, Ordering::Relaxed);
            if let Some(p) = args["path"].as_str() {
                db.add_folder(Path::new(p))?;
            }
            let warnings = db.scan(cancel)?;
            return Ok(json!(warnings));
        }
        if op == "plugins" {
            return Ok(json!(plugin_list(&root.join("plugins"), &db)));
        }
        if op == "install" {
            let installed = plugin_list(&root.join("plugins"), &db);
            drop(db);
            let m = music_plugin_host::install_with(
                Path::new(args["path"].as_str().unwrap_or("")),
                &root.join("plugins"),
                |m| retire(plugins, &m.id),
            )?;
            if !installed.iter().any(|(old, _)| {
                old.id == m.id
                    && m.network.iter().all(|d| old.network.contains(d))
                    && (old.media.is_empty()
                        || (!m.media.is_empty() && m.media.iter().all(|d| old.media.contains(d))))
            }) {
                self.library
                    .lock()
                    .map_err(|_| anyhow!("音乐库不可用"))?
                    .set(&format!("disabled:{}", m.id), json!(true))?;
            }
            return Ok(json!(m));
        }
        if op == "toggle" {
            let key = format!("disabled:{}", args["id"].as_str().unwrap_or(""));
            db.set(&key, json!(db.get(&key) != true))?;
            return Ok(Value::Null);
        }
        if op == "remove" {
            let id = args["id"].as_str().unwrap_or("");
            if plugin_list(&root.join("plugins"), &db)
                .iter()
                .any(|(m, _)| m.id == id)
            {
                std::fs::remove_file(root.join("plugins").join(format!("{id}.ypm.zip")))?;
                for p in [
                    data.join(format!("{id}.enc")),
                    root.join("plugins").join(format!("{id}.previous")),
                    root.join("plugins").join(format!("{id}.pending")),
                ] {
                    if p.exists() {
                        std::fs::remove_file(p)?;
                    }
                }
            }
            return Ok(Value::Null);
        }

        db.call(Request::new(
            match op {
                "local-playlists" => "playlists",
                "resolve" => "stream",
                other => other,
            },
            args.clone(),
        ))
        .map_err(Into::into)
    }

    fn plugin(&self, rt: &Runtime, source: &str, request: &Request) -> Result<Value> {
        let op = request.op.as_str();
        let args = &request.args;
        let op = match op {
            "import-search" => "search",
            "import-choices" => "playlists",
            op => op,
        };
        ensure!(
            matches!(
                op,
                "search"
                    | "suggest"
                    | "home"
                    | "browse"
                    | "album"
                    | "artist"
                    | "playlist"
                    | "import-playlist"
                    | "playlists"
                    | "liked"
                    | "heartbeat"
                    | "like"
                    | "savedAlbums"
                    | "saveAlbum"
                    | "recent"
                    | "resolve"
                    | "download"
                    | "lyrics"
                    | "account"
                    | "loginStart"
                    | "loginPoll"
                    | "loginCancel"
                    | "signOut"
                    | "settings"
                    | "isLiked"
            ),
            "音乐源不能调用应用管理操作"
        );
        let db = self.library.lock().map_err(|_| anyhow!("音乐库不可用"))?;
        if db.get(&format!("disabled:{source}")) == true {
            anyhow::bail!("插件已停用");
        }
        let package = self.root.join("plugins").join(format!("{source}.ypm.zip"));
        if op == "download" {
            let item: Item = serde_json::from_value(args["track"].clone())?;
            anyhow::ensure!(
                item.key.source == source
                    && item.kind == "track"
                    && item.downloadable != Some(false),
                "无效的下载曲目"
            );
            if let Some(path) = db.downloaded_path(&item.key) {
                return Ok(json!(path));
            }
            drop(db);
            let bytes = std::fs::read(package)?;
            let m = manifest(&bytes)?;
            anyhow::ensure!(
                m.capabilities.iter().any(|c| c == "download") && !m.media.is_empty(),
                "请更新音乐源插件以启用下载"
            );
            let file = rt.call(
                &bytes,
                &self.root.join("plugin-data"),
                Request::new("download", json!({"id":item.key.id})),
            )?;
            self.download_tx
                .send(download::Transfer {
                    item,
                    domains: m.media,
                    url: file["url"].as_str().unwrap_or_default().into(),
                    size: file["size"].as_u64().filter(|n| *n > 0),
                    attribution: file["attribution"].as_str().unwrap_or_default().to_owned(),
                })
                .map_err(|_| anyhow::anyhow!("下载线程已经停止"))?;
            return Ok(Value::Null);
        }
        drop(db);
        let bytes = std::fs::read(package)?;
        let data = self.root.join("plugin-data");
        if op == "import-playlist" {
            let m = manifest(&bytes)?;
            ensure!(
                m.id == source && !m.capabilities.iter().any(|c| c == "tracksOnly"),
                "该插件不支持导入歌单"
            );
            let page = super::playlist_import::read_playlist(source, |offset| {
                Ok(serde_json::from_value(rt.call(
                    &bytes,
                    &data,
                    Request::new("playlist", json!({"id":args["id"],"offset":offset})),
                )?)?)
            })?;
            let name = args["name"]
                .as_str()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .unwrap_or(&page.title);
            let mut db = self.library.lock().map_err(|_| anyhow!("音乐库不可用"))?;
            ensure!(
                plugin_list(&self.root.join("plugins"), &db)
                    .iter()
                    .any(|(m, on)| *on && m.id == source),
                "来源插件已移除或停用"
            );
            let (id, added) = db.import_playlist(args["target"].as_str(), name, &page.items)?;
            let missing: Vec<_> = page.items.into_iter().filter(|t| t.unavailable).collect();
            return Ok(json!({"id":id,"added":added,"missing":missing}));
        }
        if op == "heartbeat" {
            ensure!(
                source == "netease"
                    && manifest(&bytes)?
                        .capabilities
                        .iter()
                        .any(|c| c == "heartbeat"),
                "当前音乐源不支持心动模式"
            );
        }
        if op == "resolve" {
            let m = manifest(&bytes)?;
            let mut stream = json!({"id":args["id"]});
            let video = args["video"] == true;
            if video {
                ensure!(
                    m.capabilities.iter().any(|c| c == "video"),
                    "音乐源不支持视频播放"
                );
                stream["video"] = json!(true);
            }
            let value = rt.call(&bytes, &data, Request::new("stream", stream))?;
            ensure!(
                music_plugin_host::media_allowed(value["url"].as_str().unwrap_or(""), &m.media),
                "播放地址不在插件声明的媒体域名内"
            );
            if video && let Some(audio) = value.get("audioUrl") {
                ensure!(
                    music_plugin_host::media_allowed(audio.as_str().unwrap_or(""), &m.media),
                    "视频音轨不在插件声明的媒体域名内"
                );
            }
            if video && let Some(qualities) = value.get("qualities") {
                let choices: Vec<music_player::VideoQuality> =
                    serde_json::from_value(qualities.clone())?;
                ensure!(choices.iter().all(|q| q.height > 0 && music_plugin_host::media_allowed(&q.url, &m.media)), "视频清晰度地址不在插件声明的媒体域名内");
            }
            if let Some(referrer) = value.get("referrer") {
                let page = referrer.as_str().unwrap_or("");
                ensure!(
                    music_plugin_host::allowed(page, &m.network)
                        && page
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b":/._-~".contains(&b)),
                    "播放来源页必须是插件声明的 HTTPS 域名及普通路径"
                );
            }
            return Ok(value);
        }
        rt.call(&bytes, &data, Request::new(op, args.clone()))
    }
}

fn retire(plugins: &mut HashMap<String, PluginWorker>, id: &str) {
    if let Some(worker) = plugins.remove(id) {
        worker.retired.store(true, Ordering::Release);
        drop(worker.tx);
        let _ = worker.thread.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use music_core::ResourceKey;

    #[test]
    fn video_requires_capability_and_checks_the_separate_audio_url() -> Result<()> {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("source");
        let plugins = dir.path().join("plugins");
        std::fs::create_dir(&source)?;
        std::fs::create_dir(&plugins)?;
        std::fs::write(
            source.join("index.js"),
            r#"export function stream({id}) {
            if (id === "bad-quality") return {url:"https://media.example/video",
                qualities:[{height:720,url:"https://other.example/video"}]};
            return {url:"https://media.example/video",audioUrl:"https://other.example/audio"};
        }"#,
        )?;
        let workers = Workers {
            root: dir.path().into(),
            library: Arc::new(Mutex::new(Library::open(dir.path())?)),
            out: mpsc::channel().0,
            download_tx: mpsc::channel().0,
            generation: Arc::new(AtomicU64::new(0)),
        };
        let rt = Runtime::new();
        let watch = Request::new("resolve", json!({"id":"1","video":true}));
        for (caps, message) in [(json!([]), "不支持视频"), (json!(["video"]), "视频音轨")]
        {
            std::fs::write(source.join("plugin.json"), json!({
                "id":"fixture","name":"fixture","version":"1","main":"index.js","capabilities":caps,
                "permissions":{"media":["media.example"]}
            }).to_string())?;
            std::fs::write(
                plugins.join("fixture.ypm.zip"),
                music_plugin_host::bundle(&source)?,
            )?;
            let error = workers.plugin(&rt, "fixture", &watch).unwrap_err();
            assert!(error.to_string().contains(message), "{error:#}");
        }
        let bad = Request::new("resolve", json!({"id":"bad-quality","video":true}));
        assert!(
            workers
                .plugin(&rt, "fixture", &bad)
                .unwrap_err()
                .to_string()
                .contains("清晰度地址")
        );
        Ok(())
    }

    #[test]
    fn queued_downloads_resolve_for_local_playback() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Library::open(dir.path()).unwrap();
        let folder = dir.path().join("downloads");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("track.wav");
        std::fs::write(&path, b"fixture").unwrap();
        db.index_download(
            &path,
            Item {
                key: ResourceKey {
                    source: "guest".into(),
                    id: "1".into(),
                },
                kind: "track".into(),
                ..Item::default()
            },
        )
        .unwrap();
        let id = db.tracks().unwrap()[0].key.id.clone();
        let (out, replies) = mpsc::channel();
        let (tx, jobs) = mpsc::channel();
        let (download_tx, _) = mpsc::channel();
        Workers {
            root: dir.path().into(),
            library: Arc::new(Mutex::new(db)),
            out,
            download_tx,
            generation: Arc::new(AtomicU64::new(0)),
        }
        .start(jobs, Arc::new(AtomicBool::new(false)));
        tx.send(Job {
            source: "local".into(),
            request: Request::new("resolve", json!({"id":id})),
            generation: 1,
        })
        .unwrap();
        let file = replies
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .value
            .unwrap();
        assert!(file["url"].as_str().unwrap().starts_with("file:"));
        drop(tx);
        assert!(matches!(
            replies.recv_timeout(std::time::Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }
}
