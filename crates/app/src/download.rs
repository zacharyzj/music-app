use super::{AppWindow, DownloadRow, Job, Reply, UiState, send};
use anyhow::{Context, Result, ensure};
use music_core::{Item, ResourceKey};
use music_library::Library;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use slint::{Model, ModelRc, VecModel};
use std::sync::atomic::AtomicU64;
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;

#[derive(Clone, Serialize, Deserialize)]
pub(super) enum Phase {
    Queued,
    Active { bytes: u64, total: Option<u64> },
    Complete,
    Failed(String),
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Entry {
    pub item: Item,
    pub phase: Phase,
}

#[derive(Default)]
pub(super) struct Downloads(pub Vec<Entry>);

impl Downloads {
    pub fn restore(value: Value) -> Self {
        let entries: Vec<Entry> = serde_json::from_value(value).unwrap_or_default();
        let mut queue = Self::default();
        for mut entry in entries {
            if matches!(entry.phase, Phase::Active { .. }) {
                entry.phase = Phase::Queued;
            }
            if queue.index(&entry.item.key).is_none() {
                queue.0.push(entry);
            }
        }
        queue
    }

    pub fn snapshot(&self) -> Value {
        json!(
            self.0
                .iter()
                .filter(|entry| !matches!(entry.phase, Phase::Complete))
                .collect::<Vec<_>>()
        )
    }

    pub fn index(&self, key: &ResourceKey) -> Option<usize> {
        self.0.iter().position(|entry| entry.item.key == *key)
    }

    pub fn enqueue(&mut self, mut item: Item) -> bool {
        if let Some(i) = self.index(&item.key) {
            if !matches!(self.0[i].phase, Phase::Failed(_)) {
                return false;
            }
            self.0.remove(i);
        }
        item.cover.clear();
        self.0.push(Entry {
            item,
            phase: Phase::Queued,
        });
        true
    }

    pub fn next(&mut self) -> Option<Item> {
        if self
            .0
            .iter()
            .any(|e| matches!(e.phase, Phase::Active { .. }))
        {
            return None;
        }
        let entry = self
            .0
            .iter_mut()
            .find(|e| matches!(e.phase, Phase::Queued))?;
        entry.phase = Phase::Active {
            bytes: 0,
            total: None,
        };
        Some(entry.item.clone())
    }

    pub fn finish(&mut self, key: &ResourceKey, error: Option<String>) -> bool {
        let Some(i) = self.index(key) else {
            return false;
        };
        if !matches!(self.0[i].phase, Phase::Active { .. }) {
            return false;
        }
        self.0[i].phase = error.map_or(Phase::Complete, Phase::Failed);
        true
    }
}

pub(super) struct Progress {
    pub key: ResourceKey,
    pub bytes: u64,
    pub total: Option<u64>,
}

impl Entry {
    fn row(&self) -> DownloadRow {
        fn size(bytes: u64) -> String {
            if bytes < 1_048_576 {
                format!("{:.0} KiB", bytes as f64 / 1024.)
            } else {
                format!("{:.1} MiB", bytes as f64 / 1_048_576.)
            }
        }
        let mut row = DownloadRow {
            title: self.item.title.as_str().into(),
            artist: self.item.artist.as_str().into(),
            ..Default::default()
        };
        let status = match &self.phase {
            Phase::Queued => "等待下载".to_owned(),
            Phase::Active { bytes, total } => {
                row.active = true;
                if let Some(total) = total.filter(|n| *n > 0) {
                    row.fraction = (*bytes as f64 / total as f64).clamp(0., 1.) as f32;
                    format!(
                        "{} / {} · {:.0}%",
                        size(*bytes),
                        size(total),
                        row.fraction * 100.
                    )
                } else if *bytes > 0 {
                    format!("已下载 {}", size(*bytes))
                } else {
                    "正在获取音频…".to_owned()
                }
            }
            Phase::Complete => "已加入本地音乐库".to_owned(),
            Phase::Failed(error) => {
                row.failed = true;
                error.clone()
            }
        };
        row.status = status.into();
        row
    }
}

impl UiState {
    pub(super) fn schedule_downloads(
        &mut self,
        ui: &AppWindow,
        jobs: &mpsc::Sender<Job>,
        generation: &AtomicU64,
    ) {
        send(
            jobs,
            generation,
            "local",
            "persist-downloads",
            self.downloads.snapshot(),
        );
        if let Some(item) = self.downloads.next() {
            send(
                jobs,
                generation,
                &item.key.source,
                "download",
                json!({"track":item}),
            );
        }
        ui.set_download_pending(
            self.downloads
                .0
                .iter()
                .filter(|e| matches!(e.phase, Phase::Queued | Phase::Active { .. }))
                .count() as i32,
        );
        ui.set_downloads(ModelRc::new(VecModel::from(
            self.downloads.0.iter().map(Entry::row).collect::<Vec<_>>(),
        )));
        self.update_rows(ui);
    }

    pub(super) fn show_progress(&mut self, ui: &AppWindow, progress: Progress) {
        let Some(i) = self.downloads.index(&progress.key) else {
            return;
        };
        let entry = &mut self.downloads.0[i];
        if matches!(entry.phase, Phase::Active { .. }) {
            entry.phase = Phase::Active {
                bytes: progress.bytes,
                total: progress.total,
            };
            ui.get_downloads().set_row_data(i, entry.row());
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Transfer {
    pub item: Item,
    pub url: String,
    pub domains: Vec<String>,
    pub size: Option<u64>,
    pub attribution: String,
}

pub(super) fn worker(
    directory: PathBuf,
    library: Arc<Mutex<Library>>,
    out: mpsc::Sender<Reply>,
    progress: mpsc::Sender<Progress>,
) -> mpsc::Sender<Transfer> {
    let (tx, rx) = mpsc::channel::<Transfer>();
    std::thread::spawn(move || {
        for mut job in rx {
            let result = fetch(&directory, &job, |bytes, total| {
                let _ = progress.send(Progress {
                    key: job.item.key.clone(),
                    bytes,
                    total,
                });
            })
            .and_then(|(path, duration)| {
                job.item.duration_ms = duration;
                let indexed = library
                    .lock()
                    .map_err(|_| anyhow::anyhow!("音乐库不可用"))
                    .and_then(|mut db| db.index_download(&path, job.item.clone()));
                if let Err(e) = indexed {
                    fs::remove_file(&path)?;
                    if !job.attribution.is_empty() {
                        fs::remove_file(path.with_extension("attribution.txt"))?;
                    }
                    return Err(e);
                }
                Ok(json!(path))
            });
            if out
                .send(Reply {
                    source: job.item.key.source.clone(),
                    op: "download".into(),
                    args: json!({"track":{"key":job.item.key, "title":job.item.title}}),
                    generation: 0,
                    value: result.map_err(|e| format!("{e:#}")),
                })
                .is_err()
            {
                break;
            }
        }
    });
    tx
}

pub(super) fn fetch(
    directory: &Path,
    job: &Transfer,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<(PathBuf, u64)> {
    let domains = job.domains.clone();
    ensure!(
        !domains.is_empty() && music_plugin_host::media_allowed(&job.url, &domains),
        "下载地址不在插件的媒体域名内"
    );
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(12))
        .timeout(Duration::from_secs(600))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() < 5
                && music_plugin_host::media_allowed(attempt.url().as_str(), &domains)
            {
                attempt.follow()
            } else {
                attempt.error("下载重定向超出插件媒体域名或次数限制")
            }
        }))
        .build()?;
    let response = client
        .get(&job.url)
        .send()
        .map_err(reqwest::Error::without_url)?;
    ensure!(
        response.status() == reqwest::StatusCode::OK,
        "下载服务器返回 HTTP {}",
        response.status()
    );
    let length = response.content_length();
    if let (Some(size), Some(length)) = (job.size, length) {
        ensure!(size == length, "音频文件长度与音乐源记录不符");
    }
    save(directory, job, job.size.or(length), response, progress)
}

fn save(
    directory: &Path,
    job: &Transfer,
    expected: Option<u64>,
    reader: impl Read,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<(PathBuf, u64)> {
    const MAX: u64 = 512 * 1024 * 1024;
    ensure!(
        expected.is_none_or(|n| n > 0 && n <= MAX),
        "单曲下载大小超过 512 MiB 或文件为空"
    );
    fs::create_dir_all(directory)?;
    let mut audio = NamedTempFile::new_in(directory)?;
    let limit = expected.unwrap_or(MAX);
    let mut reader = reader.take(limit + 1);
    let mut count = 0;
    let mut block = [0u8; 64 * 1024];
    let mut reported = Instant::now();
    progress(0, expected);
    loop {
        let n = match reader.read(&mut block) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if n == 0 {
            break;
        }
        count += n as u64;
        ensure!(count <= limit, "音频文件不完整或超过大小限制");
        audio.write_all(&block[..n])?;
        if reported.elapsed() >= Duration::from_millis(200) {
            progress(count, expected);
            reported = Instant::now();
        }
    }
    ensure!(
        count > 0 && count <= MAX && expected.is_none_or(|n| n == count),
        "音频文件不完整或超过大小限制"
    );
    progress(count, expected);
    audio.as_file().sync_all()?;
    let (ext, duration) =
        music_library::downloaded_audio(audio.path()).context("下载内容不是可识别的音频")?;
    let name = if job.item.artist.is_empty() {
        job.item.title.clone()
    } else {
        format!("{} - {}", job.item.artist, job.item.title)
    };
    let name: String = name
        .chars()
        .take(64)
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let stem = format!(
        "{} - {}",
        job.item.key.source,
        name.trim_end_matches(['.', ' '])
    );
    let mut credit = if job.attribution.is_empty() {
        None
    } else {
        let mut file = NamedTempFile::new_in(directory)?;
        writeln!(file, "{}", job.attribution)?;
        file.as_file().sync_all()?;
        Some(file)
    };
    for suffix in 0.. {
        let base = if suffix == 0 {
            stem.clone()
        } else {
            format!("{stem} ({suffix})")
        };
        let path = directory.join(format!("{base}.{ext}"));
        let note = directory.join(format!("{base}.attribution.txt"));
        if path.exists() || note.exists() {
            continue;
        }
        if let Some(file) = credit.take() {
            file.persist_noclobber(&note)?;
            if let Err(err) = audio.persist_noclobber(&path) {
                let _ = fs::remove_file(note);
                return Err(err.into());
            }
        } else {
            audio.persist_noclobber(&path)?;
        }
        return Ok((path, duration));
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use music_core::{MusicSource, Request, ResourceKey};
    use music_library::Library;
    use serde_json::json;
    use std::sync::atomic::AtomicBool;

    fn recording() -> Transfer {
        Transfer {
            item: Item {
                key: ResourceKey {
                    source: "incompetech".into(),
                    id: "sample".into(),
                },
                kind: "track".into(),
                title: "Piano / 鋼琴".into(),
                search_aliases: vec!["钢琴".into()],
                artist: "A".into(),
                subtitle: "A".into(),
                license: "CC BY 4.0".into(),
                ..Item::default()
            },
            domains: vec!["incompetech.com".into()],
            url: "https://example.invalid/sample".into(),
            size: None,
            attribution: "Sample by A".into(),
        }
    }

    #[test]
    fn queue_retries_in_order_and_restores_without_signed_media() {
        let mut q = Downloads::default();
        let mut a = recording().item;
        a.cover = "https://audio.example/signed".into();
        let mut b = a.clone();
        b.key.source = "other".into();
        assert!(q.enqueue(a.clone()));
        assert!(!q.enqueue(a.clone()));
        assert!(q.enqueue(b.clone()));
        assert_eq!(q.next().unwrap().key, a.key);
        assert!(q.next().is_none());
        assert!(!q.finish(&b.key, None));
        let snapshot = q.snapshot();
        assert!(!snapshot.to_string().contains("signed"));
        let mut q = Downloads::restore(snapshot);
        assert_eq!(q.next().unwrap().key, a.key);
        assert!(q.finish(&a.key, Some("offline".into())));
        assert_eq!(q.next().unwrap().key, b.key);
        assert!(q.enqueue(a.clone()));
        assert!(!q.enqueue(a.clone()));
        assert!(q.finish(&b.key, None));
        assert_eq!(q.next().unwrap().key, a.key);
        assert!(q.finish(&a.key, None));
        assert!(q.next().is_none());
        assert_eq!(q.snapshot(), json!([]));
    }

    #[test]
    fn rejects_bad_downloads_before_creating_library_files() {
        let dir = tempfile::tempdir().unwrap();
        let job = recording();
        assert!(fetch(dir.path(), &job, |_, _| {}).is_err());
        assert!(save(dir.path(), &job, Some(100), &b"partial"[..], |_, _| {}).is_err());
        assert!(
            save(
                dir.path(),
                &job,
                None,
                &b"<html>login</html>"[..],
                |_, _| {}
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn saved_audio_is_searchable_offline_after_restart_and_scan() {
        let root = tempfile::tempdir().unwrap();
        let job = recording();
        let mut wave = b"RIFF".to_vec();
        wave.extend_from_slice(&8036u32.to_le_bytes());
        wave.extend_from_slice(
            b"WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x40\x1f\0\0\x01\0\x08\0data",
        );
        wave.extend_from_slice(&8000u32.to_le_bytes());
        wave.resize(8044, 128);
        let mut reports = Vec::new();
        let (path, duration) = save(
            &root.path().join("downloads"),
            &job,
            Some(wave.len() as u64),
            &wave[..],
            |n, total| reports.push((n, total)),
        )
        .unwrap();
        assert_eq!(reports.first(), Some(&(0, Some(wave.len() as u64))));
        assert_eq!(
            reports.last(),
            Some(&(wave.len() as u64, Some(wave.len() as u64)))
        );
        assert_eq!(path.extension().unwrap(), "wav");
        assert_eq!(duration, 1000);
        assert_eq!(
            fs::read_to_string(path.with_extension("attribution.txt"))
                .unwrap()
                .trim(),
            job.attribution
        );
        let mut db = Library::open(root.path()).unwrap();
        db.index_download(&path, job.item.clone()).unwrap();
        drop(db);
        let mut db = Library::open(root.path()).unwrap();
        db.scan(&AtomicBool::new(false)).unwrap();
        let page = db
            .call(Request::new(
                "search",
                json!({"query":"piano","kind":"track"}),
            ))
            .unwrap();
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        let item = &page["items"][0];
        assert_eq!(item["title"], job.item.title);
        assert_eq!(item["key"]["source"], "local");
        let alias = db
            .call(Request::new(
                "search",
                json!({"query":"钢琴","kind":"track"}),
            ))
            .unwrap();
        assert_eq!(alias["items"][0]["key"], item["key"]);
        assert_eq!(
            db.downloaded_path(&job.item.key).unwrap(),
            path.canonicalize().unwrap()
        );
        let url = db
            .call(Request::new("stream", json!({"id":item["key"]["id"]})))
            .unwrap();
        assert!(url["url"].as_str().unwrap().starts_with("file:"));
    }
}
