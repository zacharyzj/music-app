use super::{AppWindow, CloudUploadRow, Job, UiState, download, has_capability};
use anyhow::{Context, Result, anyhow, ensure};
use music_core::{Item, Request};
use music_library::Library;
use reqwest::{
    Url,
    blocking::{Body, Client},
};
use serde::Deserialize;
use serde_json::{Value, json};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::RefCell,
    collections::HashSet,
    fs::File,
    io::{self, Read, Seek},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

const BUCKET: &str = "jd-musicrep-privatecloud-audio-public";
const DIRECTORY: &str = "https://wanproxy.127.net/lbs";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ticket {
    song_id: String,
    resource_id: String,
    need_upload: bool,
    token: String,
    object_key: String,
}

// The plugin supplies an object key and token. Only NetEase's own directory selects the destination.
fn upload_url(server: &str, key: &str) -> Result<Url> {
    let mut url = Url::parse(server)?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some_and(|host| host
                .strip_suffix(".127.net")
                .is_some_and(|node| node.starts_with("nosup") && !node.contains('.')))
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "网易云未返回可用的 NOS 上传节点"
    );
    ensure!(
        !matches!(key, "" | "." | ".."),
        "网易云未返回有效的上传对象"
    );
    url.set_scheme("https")
        .map_err(|_| anyhow!("上传节点不支持 HTTPS"))?;
    url.path_segments_mut()
        .map_err(|_| anyhow!("上传节点路径无效"))?
        .push(BUCKET)
        .push(key);
    url.set_query(Some("offset=0&complete=true&version=1.0"));
    Ok(url)
}

fn post_file(
    client: &Client,
    url: Url,
    ticket: &Ticket,
    file: File,
    size: u64,
    md5: &str,
    ext: &str,
) -> Result<()> {
    let mime = match ext {
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "opus" | "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        _ => "application/octet-stream",
    };
    let response = client
        .post(url)
        .header("x-nos-token", &ticket.token)
        .header("Content-MD5", md5)
        .header("Content-Type", mime)
        .body(Body::sized(file, size))
        .send()
        .map_err(reqwest::Error::without_url)?;
    ensure!(
        response.status().is_success(),
        "云盘文件上传失败（HTTP {}）",
        response.status().as_u16()
    );
    let mut body = String::new();
    response.take(65536).read_to_string(&mut body)?;
    if !body.trim().is_empty() {
        let result: Value = serde_json::from_str(&body).context("云盘未确认文件上传")?;
        ensure!(
            result.get("errCode").is_none_or(|v| v == 0)
                && result
                    .get("offset")
                    .is_none_or(|v| v.as_u64() == Some(size)),
            "云盘未确认文件已完整上传"
        );
    }
    Ok(())
}

fn send_file(
    client: &Client,
    ticket: &Ticket,
    file: File,
    size: u64,
    md5: &str,
    ext: &str,
) -> Result<()> {
    let response = client
        .get(DIRECTORY)
        .query(&[("version", "1.0"), ("bucketname", BUCKET)])
        .timeout(Duration::from_secs(15))
        .send()?
        .error_for_status()?;
    let directory: Value = serde_json::from_reader(response.take(65536))?;
    let url = directory["upload"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find_map(|s| upload_url(s, &ticket.object_key).ok())
        .context("网易云未返回可用的 NOS 上传节点")?;
    post_file(client, url, ticket, file, size, md5, ext)
}

pub(super) enum Event {
    Status(String),
    Row(usize, String, bool),
    Finished(String),
}

struct Worker {
    db: Arc<Mutex<Library>>,
    jobs: mpsc::Sender<Job>,
    replies: mpsc::Receiver<std::result::Result<Value, String>>,
    events: mpsc::Sender<Event>,
}

impl Worker {
    fn call(&self, source: &str, op: &str, args: Value) -> Result<Value> {
        self.jobs
            .send(Job {
                source: source.into(),
                request: Request::new(op, args),
                generation: 0,
            })
            .map_err(|_| anyhow!("音乐源服务已停止"))?;
        self.replies
            .recv()
            .context("音乐源服务已停止")?
            .map_err(anyhow::Error::msg)
    }

    fn file(&self, item: &Item, index: usize) -> Result<(PathBuf, Option<tempfile::TempDir>)> {
        {
            let db = self.db.lock().map_err(|_| anyhow!("音乐库不可用"))?;
            if item.key.source == "local" {
                let path = db.path(&item.key.id).context("本地音乐库找不到此文件")?;
                ensure!(path.is_file(), "本地音频文件已移除");
                return Ok((path, None));
            }
            if let Some(path) = db.downloaded_path(&item.key) {
                return Ok((path, None));
            }
        }
        ensure!(item.downloadable != Some(false), "来源未开放此曲的下载权限");
        self.row(index, "正在向原音乐源申请下载".into(), false);
        let grant = self.call(&item.key.source, "cloud-download", json!({"track": item}))?;
        if let Some(cached) = grant.as_str() {
            return Ok((PathBuf::from(cached), None));
        }
        let transfer: download::Transfer = serde_json::from_value(grant)?;
        let temp = tempfile::tempdir()?;
        let (path, _) = download::fetch(temp.path(), &transfer, |bytes, total| {
            let label = match total.filter(|&t| t > 0) {
                Some(t) => format!("正在下载 · {}%", bytes.saturating_mul(100) / t),
                None => format!("正在下载 · {:.1} MiB", bytes as f64 / 1048576.),
            };
            self.row(index, label, false);
        })?;
        Ok((path, Some(temp)))
    }

    fn row(&self, index: usize, status: String, failed: bool) {
        let _ = self.events.send(Event::Row(index, status, failed));
    }

    fn upload(
        &self,
        item: &Item,
        index: usize,
        account: &str,
        seen: &mut HashSet<String>,
        client: &Client,
    ) -> Result<&'static str> {
        let (path, _download) = self.file(item, index)?;
        self.row(index, "正在核对音频文件".into(), false);
        let (ext, duration) = music_library::downloaded_audio(&path)?;
        let mut file = File::open(&path)?;
        let mut hasher = md5::Context::new();
        let size = io::copy(&mut file, &mut hasher)?;
        ensure!(size > 0, "音频文件为空");
        let md5 = format!("{:x}", hasher.compute());
        if seen.contains(&md5) {
            return Ok("相同文件已上传");
        }
        let filename = path.file_name().context("缺少文件名")?.to_string_lossy();
        let title = if item.title.is_empty() {
            path.file_stem().unwrap_or_default().to_string_lossy()
        } else {
            item.title.as_str().into()
        };
        let mut args = json!({"step":"prepare", "accountId":account, "filename":filename,
            "song":title, "artist":item.artist, "album":item.album, "ext":ext, "size":size,
            "md5":md5, "bitrate":size.saturating_mul(8000) / duration.max(1)});
        let ticket: Ticket =
            serde_json::from_value(self.call("netease", "cloud-upload", args.clone())?)?;
        if ticket.need_upload {
            self.row(index, "正在上传音频到云盘".into(), false);
            file.rewind()?;
            send_file(client, &ticket, file, size, &md5, ext)?;
        }
        self.row(index, "正在写入网易云云盘".into(), false);
        args["step"] = json!("finish");
        args["songId"] = json!(ticket.song_id);
        args["resourceId"] = json!(ticket.resource_id);
        self.call("netease", "cloud-upload", args)?;
        seen.insert(md5);
        Ok(if ticket.need_upload {
            "已上传"
        } else {
            "已秒传"
        })
    }

    fn batch(&self, tracks: &[Item], stop: &AtomicBool) -> Result<String> {
        let account = self.call("netease", "cloud-upload", json!({"step":"account"}))?;
        let id = account["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .context("请先在网易云音乐插件中登录，再上传到云盘")?;
        let name = account["name"].as_str().unwrap_or("网易云音乐");
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(12))
            .timeout(Duration::from_secs(600))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let mut seen = HashSet::new();
        let (mut done, mut failed) = (0, 0);
        let mut stopped = None;
        for (index, item) in tracks.iter().enumerate() {
            if stop.load(Ordering::Acquire) {
                stopped = Some("已停止后续上传".to_owned());
                break;
            }
            if let Err(error) = self.call(
                "netease",
                "cloud-upload",
                json!({"step":"session","accountId":id}),
            ) {
                stopped = Some(error.to_string());
                break;
            }
            let _ = self.events.send(Event::Status(format!(
                "{name} · 第 {} / {} 首",
                index + 1,
                tracks.len()
            )));
            match self.upload(item, index, id, &mut seen, &client) {
                Ok(status) => {
                    self.row(index, status.into(), false);
                    done += 1;
                }
                Err(error) => {
                    self.row(index, format!("{error:#}"), true);
                    failed += 1;
                }
            }
        }
        let mut summary = format!("{name} · 完成 {done} 首，失败 {failed} 首");
        if let Some(reason) = stopped {
            summary.push_str(&format!(
                "，未处理 {} 首 · {reason}",
                tracks.len() - done - failed
            ));
        }
        Ok(summary)
    }
}

pub(super) fn install(
    ui: &AppWindow,
    state: Rc<RefCell<UiState>>,
    db: Arc<Mutex<Library>>,
    jobs: mpsc::Sender<Job>,
    replies: mpsc::Receiver<std::result::Result<Value, String>>,
    stop: Arc<AtomicBool>,
) -> mpsc::Receiver<Event> {
    let (start, batches) = mpsc::channel::<Vec<Item>>();
    let (events, updates) = mpsc::channel();
    let canceled = stop.clone();
    std::thread::spawn(move || {
        let worker = Worker {
            db,
            jobs,
            replies,
            events,
        };
        for tracks in batches {
            let result = worker
                .batch(&tracks, &canceled)
                .unwrap_or_else(|e| format!("云盘上传未开始 · {e:#}"));
            if worker.events.send(Event::Finished(result)).is_err() {
                break;
            }
        }
    });
    let starting = stop.clone();
    let weak = ui.as_weak();
    ui.on_cloud_upload(move |queue| {
        let Some(ui) = weak.upgrade() else { return };
        ui.set_show_cloud_upload(true);
        if ui.get_cloud_busy() {
            return;
        }
        let s = state.borrow();
        ui.set_cloud_rows(ModelRc::default());
        if !has_capability(&s.plugins, "netease", "cloudUpload") {
            ui.set_cloud_status(
                "请在「设置与插件」安装或更新网易云音乐插件，并启用后再上传。".into(),
            );
            return;
        }
        let tracks = if queue {
            &s.queue.items
        } else {
            &s.local_playlist.tracks
        };
        let mut seen = HashSet::new();
        let tracks: Vec<_> = tracks
            .iter()
            .filter(|t| t.kind == "track" && seen.insert(&t.key))
            .cloned()
            .collect();
        if tracks.is_empty() {
            ui.set_cloud_status("列表中没有可上传的歌曲".into());
            return;
        }
        ui.set_cloud_rows(ModelRc::new(VecModel::from(
            tracks
                .iter()
                .map(|t| CloudUploadRow {
                    title: t.title.as_str().into(),
                    status: "未上传".into(),
                    failed: false,
                })
                .collect::<Vec<_>>(),
        )));
        ui.set_cloud_status(format!("正在确认网易云账号 · {} 首歌曲", tracks.len()).into());
        starting.store(false, Ordering::Release);
        ui.set_cloud_stopping(false);
        ui.set_cloud_busy(start.send(tracks).is_ok());
        if !ui.get_cloud_busy() {
            ui.set_cloud_status("上传服务已停止".into());
        }
    });
    let weak = ui.as_weak();
    ui.on_stop_cloud_upload(move || {
        stop.store(true, Ordering::Release);
        if let Some(ui) = weak.upgrade() {
            ui.set_cloud_stopping(true);
        }
    });
    updates
}

pub(super) fn present(ui: &AppWindow, event: Event) {
    match event {
        Event::Status(message) => ui.set_cloud_status(message.into()),
        Event::Row(index, status, failed) => {
            let rows = ui.get_cloud_rows();
            if let Some(mut row) = rows.row_data(index) {
                row.status = status.into();
                row.failed = failed;
                rows.set_row_data(index, row);
            }
        }
        Event::Finished(message) => {
            ui.set_cloud_busy(false);
            ui.set_cloud_status(message.as_str().into());
            if !ui.get_show_cloud_upload() {
                ui.invoke_notify_row(message.into());
            }
        }
    }
}

#[cfg(test)]
mod tests;
