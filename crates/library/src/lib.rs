use anyhow::Result;
use lofty::{
    file::{AudioFile, TaggedFileExt},
    tag::Accessor,
};
use music_core::*;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
pub struct Library {
    pub db: Connection,
    root: PathBuf,
}
impl Library {
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        let db = Connection::open(root.join("library.sqlite"))?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS tracks(id TEXT PRIMARY KEY,path TEXT UNIQUE NOT NULL,root TEXT NOT NULL,stamp TEXT NOT NULL,data TEXT NOT NULL); CREATE TABLE IF NOT EXISTS liked(id TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS playlists(id TEXT PRIMARY KEY,name TEXT NOT NULL,tracks TEXT NOT NULL);")?;
        Ok(Self {
            db,
            root: root.into(),
        })
    }
    pub fn get(&self, key: &str) -> Value {
        self.db
            .query_row("SELECT value FROM kv WHERE key=?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or(Value::Null)
    }
    pub fn set(&self, key: &str, v: Value) -> Result<()> {
        self.db.execute(
            "INSERT INTO kv VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, v.to_string()],
        )?;
        Ok(())
    }
    pub fn folders(&self) -> Vec<String> {
        serde_json::from_value(self.get("folders")).unwrap_or_default()
    }
    pub fn add_folder(&self, path: &Path) -> Result<()> {
        let path = std::fs::canonicalize(path)?.to_string_lossy().to_string();
        let mut folders = self.folders();
        if !folders.contains(&path) {
            folders.push(path);
        }
        self.set("folders", json!(folders))
    }
    pub fn scan(&mut self, cancel: &AtomicBool) -> Result<Vec<String>> {
        let mut warnings = Vec::new();
        for folder in self.folders() {
            let mut found = HashSet::new();
            let mut complete = true;
            let mut updates = Vec::new();
            for entry in walkdir::WalkDir::new(&folder).follow_links(false) {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(vec!["扫描已取消，原索引保留".into()]);
                }
                let entry = match entry {
                    Ok(x) => x,
                    Err(e) => {
                        complete = false;
                        warnings.push(e.to_string());
                        continue;
                    }
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let p = entry.path();
                let ext = p
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_ascii_lowercase();
                if ![
                    "mp3", "flac", "m4a", "wav", "ogg", "opus", "ape", "aif", "aiff", "aac",
                ]
                .contains(&ext.as_str())
                {
                    continue;
                }
                let path = p.to_string_lossy().to_string();
                found.insert(path.clone());
                let meta = match p.metadata() {
                    Ok(v) => v,
                    Err(e) => {
                        complete = false;
                        warnings.push(e.to_string());
                        continue;
                    }
                };
                let stamp = format!("{}:{:?}", meta.len(), meta.modified());
                let old = self
                    .db
                    .query_row("SELECT id,stamp FROM tracks WHERE path=?1", [&path], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })
                    .ok();
                if old.as_ref().is_some_and(|v| v.1 == stamp) {
                    continue;
                }
                let id = old
                    .map(|v| v.0)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let mut item = Item {
                    key: ResourceKey {
                        source: "local".into(),
                        id: id.clone(),
                    },
                    kind: "track".into(),
                    title: p
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    ..Default::default()
                };
                match lofty::read_from_path(p) {
                    Ok(tagged) => {
                        item.duration_ms = tagged.properties().duration().as_millis() as u64;
                        if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
                            item.title = tag.title().map(|s| s.to_string()).unwrap_or(item.title);
                            item.artist = tag.artist().map(|s| s.to_string()).unwrap_or_default();
                            item.album = tag.album().map(|s| s.to_string()).unwrap_or_default();
                            if let Some(pic) = tag.pictures().first() {
                                let covers = self.root.join("covers");
                                std::fs::create_dir_all(&covers)?;
                                let cover = covers.join(format!("{id}.img"));
                                std::fs::write(&cover, pic.data())?;
                                item.cover = cover.to_string_lossy().to_string();
                            }
                        }
                    }
                    Err(e) => warnings.push(format!("{}：标签不可读 ({e})", p.display())),
                }
                if item.cover.is_empty() {
                    for name in ["cover.jpg", "folder.jpg", "cover.png", "folder.png"] {
                        let cover = p.parent().unwrap().join(name);
                        if cover.exists() {
                            item.cover = cover.to_string_lossy().to_string();
                            break;
                        }
                    }
                }
                item.subtitle = format!("{} · {}", item.artist, item.album);
                updates.push((id, path, stamp, serde_json::to_string(&item)?));
            }
            if cancel.load(Ordering::Relaxed) {
                return Ok(vec!["扫描已取消".into()]);
            }
            let tx = self.db.transaction()?;
            for (id, path, stamp, data) in updates {
                tx.execute("INSERT INTO tracks VALUES(?1,?2,?3,?4,?5) ON CONFLICT(path) DO UPDATE SET stamp=excluded.stamp,data=excluded.data",params![id,path,folder,stamp,data])?;
            }
            if complete {
                let paths: Vec<String> = tx
                    .prepare("SELECT path FROM tracks WHERE root=?1")?
                    .query_map([&folder], |r| r.get(0))?
                    .collect::<std::result::Result<_, _>>()?;
                for p in paths {
                    if !found.contains(&p) {
                        tx.execute("DELETE FROM tracks WHERE path=?1", [p])?;
                    }
                }
            }
            tx.commit()?;
        }
        Ok(warnings)
    }
    pub fn tracks(&self) -> Result<Vec<Item>> {
        let strings: Vec<String> = self
            .db
            .prepare("SELECT data FROM tracks ORDER BY path")?
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(strings
            .iter()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect())
    }
    pub fn path(&self, id: &str) -> Result<PathBuf> {
        Ok(PathBuf::from(self.db.query_row(
            "SELECT path FROM tracks WHERE id=?1",
            [id],
            |r| r.get::<_, String>(0),
        )?))
    }
    fn playlist_page(&self, query: &str, offset: u64) -> Result<Value> {
        let query = query.to_lowercase();
        let items = self
            .db
            .prepare("SELECT id,name FROM playlists ORDER BY name")?
            .query_map([], |r| {
                Ok(Item {
                    key: ResourceKey {
                        source: "local".into(),
                        id: r.get(0)?,
                    },
                    title: r.get(1)?,
                    kind: "playlist".into(),
                    ..Default::default()
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let filtered: Vec<_> = items
            .into_iter()
            .filter(|v| v.title.to_lowercase().contains(&query))
            .collect();
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let more = filtered.len() > start.saturating_add(100);
        Ok(json!(Page {
            title: "本地歌单".into(),
            items: filtered.into_iter().skip(start).take(100).collect(),
            offset,
            more
        }))
    }
    fn run(&mut self, r: Request) -> Result<Value> {
        let a = r.args;
        let id = a["id"].as_str().unwrap_or("");
        match r.op.as_str() {
            "stream" => Ok(
                json!({"url":url::Url::from_file_path(self.path(id)?).map_err(|_|anyhow::anyhow!("无效本地路径"))?.as_str()}),
            ),
            "lyrics" => {
                let p = self.path(id)?.with_extension("lrc");
                Ok(json!({"lrc":std::fs::read_to_string(p).unwrap_or_default()}))
            }
            "like" => {
                if a["on"] == true {
                    self.db
                        .execute("INSERT OR IGNORE INTO liked VALUES(?1)", [id])?;
                } else {
                    self.db.execute("DELETE FROM liked WHERE id=?1", [id])?;
                }
                Ok(Value::Null)
            }
            "createPlaylist" => {
                let id = uuid::Uuid::new_v4().to_string();
                self.db.execute(
                    "INSERT INTO playlists VALUES(?1,?2,'[]')",
                    params![id, a["name"].as_str().unwrap_or("我的歌单")],
                )?;
                Ok(json!(id))
            }
            "addToPlaylist" | "removeFromPlaylist" => {
                let s: String =
                    self.db
                        .query_row("SELECT tracks FROM playlists WHERE id=?1", [id], |r| {
                            r.get(0)
                        })?;
                let mut ids: Vec<String> = serde_json::from_str(&s)?;
                let track = a["track"].as_str().unwrap_or("").to_owned();
                if r.op == "addToPlaylist" {
                    if !ids.contains(&track) {
                        ids.push(track);
                    }
                } else {
                    ids.retain(|x| x != &track);
                }
                self.db.execute(
                    "UPDATE playlists SET tracks=?1 WHERE id=?2",
                    params![serde_json::to_string(&ids)?, id],
                )?;
                Ok(Value::Null)
            }
            "deletePlaylist" => {
                self.db.execute("DELETE FROM playlists WHERE id=?1", [id])?;
                Ok(Value::Null)
            }
            "renamePlaylist" => {
                self.db.execute(
                    "UPDATE playlists SET name=?1 WHERE id=?2",
                    params![a["name"].as_str().unwrap_or("我的歌单"), id],
                )?;
                Ok(Value::Null)
            }
            "playlists" => self.playlist_page("", a["offset"].as_u64().unwrap_or(0)),
            "search" if a["kind"] == "playlist" => self.playlist_page(
                a["query"].as_str().unwrap_or(""),
                a["offset"].as_u64().unwrap_or(0),
            ),
            _ => {
                let mut tracks = self.tracks()?;
                let mut title = "本地音乐".to_string();
                match r.op.as_str() {
                    "liked" => {
                        let ids: HashSet<String> = self
                            .db
                            .prepare("SELECT id FROM liked")?
                            .query_map([], |r| r.get(0))?
                            .collect::<std::result::Result<_, _>>()?;
                        tracks.retain(|v| ids.contains(&v.key.id));
                        title = "喜欢的歌曲".into();
                    }
                    "playlist" => {
                        let s: String = self.db.query_row(
                            "SELECT tracks FROM playlists WHERE id=?1",
                            [id],
                            |r| r.get(0),
                        )?;
                        let ids: Vec<String> = serde_json::from_str(&s)?;
                        let all: HashMap<_, _> =
                            tracks.into_iter().map(|v| (v.key.id.clone(), v)).collect();
                        tracks = ids.iter().filter_map(|id| all.get(id).cloned()).collect();
                    }
                    "album" => {
                        tracks.retain(|v| format!("{}\u{1f}{}", v.artist, v.album) == id);
                        title = tracks.first().map(|v| v.album.clone()).unwrap_or_default();
                    }
                    "artist" => {
                        tracks.retain(|v| v.artist == id);
                        title = id.into();
                    }
                    "home" | "search" | "library" => {}
                    _ => anyhow::bail!("不支持此本地操作"),
                }
                let query = a["query"].as_str().unwrap_or("").to_lowercase();
                tracks.retain(|v| {
                    format!("{} {} {}", v.title, v.artist, v.album)
                        .to_lowercase()
                        .contains(&query)
                });
                let kind = a["kind"].as_str().unwrap_or("track");
                if ["album", "artist"].contains(&kind) {
                    let mut seen = HashSet::new();
                    tracks = tracks
                        .into_iter()
                        .filter_map(|v| {
                            let id = if kind == "album" {
                                format!("{}\u{1f}{}", v.artist, v.album)
                            } else {
                                v.artist.clone()
                            };
                            if !seen.insert(id.clone()) {
                                return None;
                            }
                            Some(Item {
                                key: ResourceKey {
                                    source: "local".into(),
                                    id,
                                },
                                kind: kind.into(),
                                title: if kind == "album" { v.album } else { v.artist },
                                subtitle: v.subtitle,
                                cover: v.cover,
                                ..Default::default()
                            })
                        })
                        .collect();
                }
                let offset = a["offset"].as_u64().unwrap_or(0);
                let more = tracks.len() > offset as usize + 100;
                Ok(json!(Page {
                    title,
                    items: tracks.into_iter().skip(offset as usize).take(100).collect(),
                    offset,
                    more
                }))
            }
        }
    }
}
impl MusicSource for Library {
    fn call(&mut self, r: Request) -> SourceResult {
        self.run(r)
            .map_err(|e| SourceError::new("local", e.to_string()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persist_and_playlist() {
        let dir = tempfile::tempdir().unwrap();
        let mut lib = Library::open(dir.path()).unwrap();
        lib.set("x", json!(42)).unwrap();
        assert_eq!(lib.get("x"), 42);
        let id = lib
            .call(Request::new("createPlaylist", json!({"name":"测试"})))
            .unwrap();
        assert!(id.is_string());
        let p = lib.call(Request::new("playlists", json!({}))).unwrap();
        assert_eq!(p["items"][0]["title"], "测试");
        let p = lib
            .call(Request::new(
                "search",
                json!({"kind":"playlist","query":"测试"}),
            ))
            .unwrap();
        assert_eq!(p["items"][0]["key"]["id"], id);
        let p = lib
            .call(Request::new(
                "search",
                json!({"kind":"playlist","query":"不匹配"}),
            ))
            .unwrap();
        assert!(p["items"].as_array().unwrap().is_empty());
    }
    #[test]
    fn scan_incremental_cancel_and_offline_root() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir(&music).unwrap();
        std::fs::write(music.join("测试.mp3"), b"invalid tag but usable filename").unwrap();
        let mut lib = Library::open(&dir.path().join("data")).unwrap();
        lib.add_folder(&music).unwrap();
        lib.scan(&AtomicBool::new(false)).unwrap();
        let original = lib.tracks().unwrap();
        assert_eq!(original.len(), 1);
        lib.scan(&AtomicBool::new(false)).unwrap();
        assert_eq!(lib.tracks().unwrap()[0].key, original[0].key);
        std::fs::remove_file(music.join("测试.mp3")).unwrap();
        lib.scan(&AtomicBool::new(true)).unwrap();
        assert_eq!(lib.tracks().unwrap().len(), 1);
        std::fs::rename(&music, dir.path().join("offline")).unwrap();
        assert!(!lib.scan(&AtomicBool::new(false)).unwrap().is_empty());
        assert_eq!(lib.tracks().unwrap().len(), 1);
        std::fs::create_dir(&music).unwrap();
        lib.scan(&AtomicBool::new(false)).unwrap();
        assert!(lib.tracks().unwrap().is_empty());
    }
}
