mod covers;
mod download;
mod search;
#[cfg(windows)]
mod window_behavior;
mod workers;
use anyhow::Result;
use music_core::*;
use music_library::Library;
use music_player::{Command, Player, PlayerEvent};
use music_plugin_host::{catalog, manifest};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
slint::include_modules!();
struct Job {
    source: String,
    request: Request,
    generation: u64,
}
struct Reply {
    source: String,
    op: String,
    args: Value,
    value: std::result::Result<Value, String>,
    generation: u64,
}
struct UiState {
    source: String,
    page_source: String,
    online: Option<search::Search>,
    downloads: download::Downloads,
    sources: Vec<String>,
    plugins: Vec<(Manifest, bool)>,
    catalog: Vec<catalog::Entry>,
    nav: Vec<Navigation>,
    page_links: Vec<Navigation>,
    page_tabs: Vec<Navigation>,
    items: Vec<Item>,
    queue: Queue,
    last: Request,
    playlists: Vec<Item>,
    lines: Vec<(u64, String)>,
    translations: Vec<(u64, String)>,
    login_key: String,
    login_started: Option<Instant>,
    last_poll: Instant,
    login_inflight: bool,
    suggest_query: String,
    playing: bool,
    loaded: bool,
    play_generation: u64,
}
fn has_capability(plugins: &[(Manifest, bool)], id: &str, cap: &str) -> bool {
    plugins
        .iter()
        .any(|(m, enabled)| *enabled && m.id == id && m.capabilities.iter().any(|c| c == cap))
}

impl UiState {
    fn clear_page(&mut self, ui: &AppWindow) {
        self.items.clear();
        self.page_source = self.source.clone();
        self.online = None;
        ui.set_can_filter(false);
        self.page_links.clear();
        ui.set_online_sources(ModelRc::default());
        ui.set_page_links(ModelRc::default());
        ui.set_page_tabs(ModelRc::default());
        ui.set_rows(ModelRc::default());
        ui.set_more(false);
        ui.set_next_label("加载更多".into());
        ui.set_search_page(false);
        ui.set_can_download(false);
        ui.set_home_page(false);
        ui.set_album_detail(false);
        ui.set_local_playlist(false);
        ui.set_has_error(false);
    }

    fn update_rows(&self, ui: &AppWindow) {
        let list: Vec<_> = self
            .items
            .iter()
            .map(|item| {
                let mut result = row(item);
                let plugin = self
                    .plugins
                    .iter()
                    .find(|(m, enabled)| *enabled && m.id == item.key.source)
                    .map(|(m, _)| m);
                let anonymous =
                    plugin.is_some_and(|m| m.capabilities.iter().any(|c| c == "anonymous"));
                let can_download = plugin.is_some_and(|m| {
                    !m.media.is_empty() && m.capabilities.iter().any(|c| c == "download")
                });
                result.show_download = item.kind == "track" && can_download;
                result.account_actions = plugin.is_none_or(Manifest::supports_likes);
                if self.online.is_some()
                    && let Some(m) = plugin
                {
                    let mut details = vec![m.name.as_str()];
                    if !result.subtitle.is_empty() {
                        details.push(result.subtitle.as_str());
                    }
                    if result.show_download && item.downloadable != Some(false) {
                        details.push(if anonymous && item.downloadable == Some(true) {
                            "免登录可下载"
                        } else if !anonymous {
                            "按账号确认"
                        } else {
                            "下载权限待确认"
                        });
                    }
                    result.subtitle = details.join(" · ").into();
                }
                if let Some(index) = self.downloads.index(&item.key) {
                    use download::Phase::*;
                    let phase = &self.downloads.0[index].phase;
                    result.download_blocked |= !matches!(phase, Failed(_));
                    result.download_label = match phase {
                        Queued => "排队中",
                        Active { .. } => "下载中",
                        Complete => "已入库",
                        Failed(_) => "重试",
                    }
                    .into();
                }
                result
            })
            .collect();
        ui.set_rows(ModelRc::new(VecModel::from(list)));
    }

    fn begin_online(&mut self, ui: &AppWindow, jobs: &mpsc::Sender<Job>, genr: &AtomicU64) -> bool {
        let kind = self.last.args["kind"].as_str().unwrap_or("track");
        let Some(search) = search::Search::new(&self.plugins, &self.source, kind) else {
            return false;
        };
        let mut choices = vec![slint::SharedString::from("全部音乐源")];
        choices.extend(search.sources.iter().map(|s| s.name.as_str().into()));
        ui.set_online_sources(ModelRc::new(VecModel::from(choices)));
        ui.set_online_index(0);
        ui.set_search_page(true);
        ui.set_can_filter(kind == "track");
        ui.set_can_download(search.sources.iter().any(|s| s.downloads));
        ui.set_page_account_actions(search.sources.iter().any(|s| s.likes));
        ui.set_heading(if self.source == "local" {
            "在线搜索".into()
        } else {
            format!("搜索结果 · {}", search.sources[0].name).into()
        });
        self.online = Some(search);
        self.request_pages(jobs, genr, false);
        self.show_search(ui);
        true
    }

    fn request_pages(&mut self, jobs: &mpsc::Sender<Job>, genr: &AtomicU64, guest_only: bool) {
        if let Some(search) = self.online.as_mut() {
            for (source, offset) in search.requests(guest_only) {
                let mut args = self.last.args.clone();
                args["offset"] = json!(offset);
                send(jobs, genr, &source, "search", args);
            }
        }
    }

    fn show_search(&mut self, ui: &AppWindow) {
        let Some(search) = &self.online else {
            return;
        };
        let guest_only = ui.get_can_filter() && ui.get_guest_only();
        self.items = search.visible(guest_only);
        let pending = search.pending(guest_only);
        let errors = search.errors(guest_only);
        let mut status = format!("{} 项结果", self.items.len());
        if pending > 0 {
            status.push_str(&format!(" · {pending} 个源搜索中"));
        }
        if !errors.is_empty() {
            status.push_str(&format!(" · {}", errors.join("；")));
        }
        ui.set_status(status.into());
        ui.set_has_error(!errors.is_empty() && self.items.is_empty() && pending == 0);
        ui.set_busy(self.items.is_empty() && pending > 0);
        ui.set_more(search.more(guest_only));
        ui.set_next_label(search.next_label(guest_only).into());
        ui.set_cards(!self.items.is_empty() && self.items.iter().all(|i| i.kind != "track"));
        self.update_rows(ui);
    }
}

fn image_path(path: &str) -> slint::Image {
    if Path::new(path).is_file() {
        slint::Image::load_from_path(Path::new(path)).unwrap_or_default()
    } else {
        slint::Image::default()
    }
}
fn row(t: &Item) -> Row {
    Row {
        title: t.title.clone().into(),
        subtitle: if t.license.is_empty() {
            t.subtitle.clone()
        } else {
            format!("{} · {}", t.subtitle, t.license)
        }
        .into(),
        resource: format!("{}:{}", t.key.source, t.key.id).into(),
        kind: t.kind.clone().into(),
        cover: if !t.cover.starts_with("http") {
            image_path(&t.cover)
        } else {
            slint::Image::default()
        },
        disabled: t.unavailable,
        show_download: false,
        account_actions: true,
        download_blocked: t.downloadable == Some(false),
        download_label: if t.downloadable == Some(false) {
            "未开放下载"
        } else {
            "下载"
        }
        .into(),
        album: t.album.clone().into(),
        duration: format!(
            "{:02}:{:02}",
            t.duration_ms / 60000,
            t.duration_ms / 1000 % 60
        )
        .into(),
    }
}
fn rows(items: &[Item]) -> ModelRc<Row> {
    ModelRc::new(VecModel::from(items.iter().map(row).collect::<Vec<_>>()))
}
fn prepare_items(
    items: &mut [Item],
    plugins: &[(Manifest, bool)],
    source: &str,
    covers: &mpsc::SyncSender<covers::Cover>,
    generation: u64,
) {
    let domains = plugins
        .iter()
        .find(|(m, _)| m.id == source)
        .map(|(m, _)| m.media.as_slice())
        .unwrap_or_default();
    for item in items {
        item.key.source = source.to_owned();
        if source == "local" {
            continue;
        }
        if !music_plugin_host::media_allowed(&item.cover, domains) {
            item.cover.clear();
        } else {
            let _ = covers.try_send(covers::Cover {
                key: item.key.clone(),
                url: item.cover.clone(),
                domains: domains.to_vec(),
                generation,
            });
        }
    }
}

fn local_nav() -> Vec<Navigation> {
    vec![
        ("歌曲", "library", json!({"kind":"track"})),
        ("专辑", "library", json!({"kind":"album"})),
        ("歌手", "library", json!({"kind":"artist"})),
        ("喜欢的歌曲", "liked", json!({})),
        ("本地歌单", "playlists", json!({})),
    ]
    .into_iter()
    .map(|(title, op, args)| Navigation {
        title: title.into(),
        op: op.into(),
        args,
    })
    .collect()
}
fn send(tx: &mpsc::Sender<Job>, generation: &AtomicU64, source: &str, op: &str, args: Value) {
    let _ = tx.send(Job {
        source: source.into(),
        request: Request::new(op, args),
        generation: generation.load(Ordering::Relaxed),
    });
}
fn show_nav(ui: &AppWindow, state: &UiState) {
    ui.set_nav(ModelRc::new(VecModel::from(
        state
            .nav
            .iter()
            .map(|n| Nav {
                title: n.title.clone().into(),
            })
            .collect::<Vec<_>>(),
    )));
}
fn show_page_navigation(ui: &AppWindow, state: &mut UiState, links: Vec<Navigation>) {
    if state.last.op == "home" {
        let title = state
            .nav
            .iter()
            .find(|n| n.op == "home")
            .map(|n| n.title.clone())
            .unwrap_or_else(|| "首页".into());
        state.page_tabs = vec![Navigation {
            title,
            op: "home".into(),
            args: json!({}),
        }];
        state.page_tabs.extend(links.iter().cloned());
    }
    let active = state.page_tabs.iter().position(|tab|tab.op == state.last.op && tab.args == state.last.args)
        .or_else(||state.page_tabs.iter().position(|tab| {
            // Keep the same view selected when a plugin's :0 page advances.
            if tab.op != "browse" || state.last.op != "browse" {return false;}
            let base = tab.args["page"].as_str().and_then(|p|p.strip_suffix(":0"));
            let page = state.last.args["page"].as_str().and_then(|p|p.rsplit_once(':'));
            matches!((base,page),(Some(base),Some((current,offset))) if base==current && !offset.is_empty() && offset.bytes().all(|b|b.is_ascii_digit()))
        }));
    ui.set_page_tab_index(active.map_or(-1, |i| i as i32));
    ui.set_page_tabs(if active.is_some() {
        ModelRc::new(VecModel::from(
            state
                .page_tabs
                .iter()
                .map(|n| Nav {
                    title: n.title.clone().into(),
                })
                .collect::<Vec<_>>(),
        ))
    } else {
        ModelRc::default()
    });
    state.page_links = links
        .into_iter()
        .filter(|n| {
            active.is_none()
                || !state
                    .page_tabs
                    .iter()
                    .any(|t| t.op == n.op && t.args == n.args)
        })
        .collect();
    ui.set_page_links(ModelRc::new(VecModel::from(
        state
            .page_links
            .iter()
            .map(|n| Nav {
                title: n.title.clone().into(),
            })
            .collect::<Vec<_>>(),
    )));
}
fn refresh_queue(ui: &AppWindow, state: &UiState) {
    ui.set_queue(rows(&state.queue.items));
}
fn play(
    ui: &AppWindow,
    state: &mut UiState,
    tx: &mpsc::Sender<Job>,
    generation: &AtomicU64,
    player: &mpsc::Sender<Command>,
    item: Item,
) {
    state.play_generation += 1;
    let _ = generation;
    state.loaded = false;
    if player.send(Command::Stop).is_err() {
        ui.set_status("播放引擎不可用，请检查 libmpv 路径后重启".into());
        ui.set_playing(false);
        return;
    }
    state.playing = false;
    ui.set_playing(false);
    ui.set_now_title(item.title.clone().into());
    ui.set_now_artist(item.subtitle.clone().into());
    ui.set_now_cover(row(&item).cover);
    ui.set_lyric("".into());
    ui.set_translated_lyric("".into());
    ui.set_lyrics("".into());
    state.lines.clear();
    state.translations.clear();
    ui.set_busy(true);
    let _ = tx.send(Job {
        source: item.key.source,
        request: Request::new("resolve", json!({"id":item.key.id})),
        generation: state.play_generation,
    });
}
fn plugin_list(dir: &Path, db: &Library) -> Vec<(Manifest, bool)> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let p = e.ok()?.path();
            if p.extension()?.to_str()? != "zip" {
                return None;
            }
            let m = manifest(&std::fs::read(p).ok()?).ok()?;
            let enabled = db.get(&format!("disabled:{}", m.id)) != true;
            Some((m, enabled))
        })
        .collect()
}
pub fn run() -> Result<()> {
    let root = std::env::var_os("MUSIC_API_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            directories::ProjectDirs::from("org", "music-api", "MusicPlayer")
                .map(|p| p.data_dir().to_path_buf())
        })
        .ok_or_else(|| anyhow::anyhow!("无法确定数据目录"))?;
    run_at(root)
}
pub fn run_at(root: PathBuf) -> Result<()> {
    let ui = AppWindow::new()?;
    #[cfg(windows)]
    window_behavior::install(&ui);
    let library = Library::open(&root)?;
    let restored: Queue = serde_json::from_value(library.get("queue")).unwrap_or_default();
    ui.set_dark(library.get("dark").as_bool().unwrap_or(true));
    ui.set_repeat_mode(restored.repeat as i32);
    ui.set_shuffle_enabled(restored.shuffle);
    let plugin_dir = root.join("plugins");
    std::fs::create_dir_all(&plugin_dir)?;
    let plugins = plugin_list(&plugin_dir, &library);
    let state = Rc::new(RefCell::new(UiState {
        source: "local".into(),
        page_source: "local".into(),
        online: None,
        downloads: download::Downloads::restore(library.get("download-queue")),
        sources: vec!["local".into()],
        plugins,
        catalog: Vec::new(),
        nav: local_nav(),
        page_links: vec![],
        page_tabs: vec![],
        items: vec![],
        queue: restored,
        last: Request::new("library", json!({})),
        playlists: vec![],
        lines: vec![],
        translations: vec![],
        login_key: String::new(),
        login_started: None,
        last_poll: Instant::now(),
        login_inflight: false,
        suggest_query: String::new(),
        playing: false,
        loaded: false,
        play_generation: 0,
    }));
    let (tx, rx) = mpsc::channel::<Job>();
    let (out_tx, out_rx) = mpsc::channel::<Reply>();
    let generation = Arc::new(AtomicU64::new(1));
    let cancel = Arc::new(AtomicBool::new(false));
    let (cover_tx, cover_rx) = covers::worker(root.join("cover-cache"), generation.clone());
    let library = Arc::new(Mutex::new(library));
    let (progress_tx, progress_rx) = mpsc::channel();
    let download_tx = download::worker(
        root.join("downloads"),
        library.clone(),
        out_tx.clone(),
        progress_tx,
    );
    workers::Workers {
        root: root.clone(),
        library,
        generation: generation.clone(),
        out: out_tx,
        download_tx,
    }
    .start(rx, cancel.clone());
    let player = Rc::new(Player::start(
        std::env::var_os("MUSIC_API_MPV").as_deref().map(Path::new),
    ));
    let ptx = player.commands.clone();
    {
        let mut s = state.borrow_mut();
        update_plugins(&ui, &mut s);
        show_nav(&ui, &s);
        refresh_queue(&ui, &s);
        s.schedule_downloads(&ui, &tx, &generation);
    }
    let weak = ui.as_weak();
    let st = state.clone();
    let job = tx.clone();
    let genr = generation.clone();
    ui.on_select_source(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let mut s = st.borrow_mut();
        let Some(source) = s.sources.get(index as usize).cloned() else {
            return;
        };
        genr.fetch_add(1, Ordering::Relaxed);
        if s.login_started.is_some() {
            send(&job, &genr, &s.source, "loginCancel", json!({}));
        }
        s.login_started = None;
        s.login_key.clear();
        s.login_inflight = false;
        ui.set_show_login(false);
        s.source = source.clone();
        s.clear_page(&ui);
        let anonymous = has_capability(&s.plugins, &source, "anonymous");
        ui.set_anonymous_source(anonymous);
        ui.set_has_quality_setting(s.plugins.iter().any(|(m, on)| {
            *on && m.id == source && m.settings.iter().any(|setting| setting.key == "quality")
        }));
        ui.set_nav_index(0);
        s.page_tabs.clear();
        s.suggest_query.clear();
        ui.set_suggestions(ModelRc::default());
        ui.set_account_name("登录".into());
        s.nav = if source == "local" {
            local_nav()
        } else {
            s.plugins
                .iter()
                .find(|(m, _)| m.id == source)
                .map(|(m, _)| m.navigation.clone())
                .unwrap_or_default()
        };
        show_nav(&ui, &s);
        s.last = Request::new(
            if source == "local" { "library" } else { "home" },
            json!({}),
        );
        ui.set_busy(true);
        send(&job, &genr, &source, &s.last.op, s.last.args.clone());
        if source != "local" && !anonymous {
            send(&job, &genr, &source, "account", json!({}));
            if s.plugins
                .iter()
                .any(|(m, _)| m.id == source && m.capabilities.iter().any(|c| c == "settings"))
            {
                send(&job, &genr, &source, "settings", json!({}));
            }
        }
        ui.set_show_settings(false);
    });
    let weak = ui.as_weak();
    let st = state.clone();
    let job = tx.clone();
    let genr = generation.clone();
    ui.on_search(move |query, kind| {
        if let Some(ui) = weak.upgrade() {
            ui.set_suggestions(ModelRc::default());
            let mut s = st.borrow_mut();
            s.suggest_query.clear();
            genr.fetch_add(1, Ordering::Relaxed);
            s.clear_page(&ui);
            ui.set_search_page(true);
            s.last = Request::new("search", json!({
                "query": query.trim(),
                "kind": (["track", "album", "artist", "playlist"].get(kind as usize).unwrap_or(&"track"))
            }));
            ui.set_busy(true);
            ui.set_show_settings(false);
            ui.set_show_lyrics(false);
            if s.source == "local" || !s.begin_online(&ui, &job, &genr) {
                send(&job, &genr, &s.source, &s.last.op, s.last.args.clone());
            }
        }
    });
    let weak = ui.as_weak();
    let st = state.clone();
    ui.on_online_source(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let mut s = st.borrow_mut();
        if let Some(search) = &mut s.online {
            search.selected =
                (index > 0 && index as usize <= search.sources.len()).then(|| index as usize - 1);
            ui.set_online_index(index);
            s.show_search(&ui);
        }
    });
    let weak = ui.as_weak();
    let st = state.clone();
    ui.on_filter_changed(move || {
        if let Some(ui) = weak.upgrade() {
            st.borrow_mut().show_search(&ui);
        }
    });
    let weak = ui.as_weak();
    let st = state.clone();
    let job = tx.clone();
    let genr = generation.clone();
    ui.on_navigate(move |i| {
        if let Some(ui) = weak.upgrade() {
            let mut s = st.borrow_mut();
            if let Some(n) = s.nav.get(i as usize).cloned() {
                s.clear_page(&ui);
                ui.set_nav_index(i);
                genr.fetch_add(1, Ordering::Relaxed);
                s.last = Request::new(&n.op, n.args);
                ui.set_busy(true);
                ui.set_show_settings(false);
                ui.set_show_lyrics(false);
                send(&job, &genr, &s.source, &s.last.op, s.last.args.clone());
            }
        }
    });
    let weak = ui.as_weak();
    let st = state.clone();
    let job = tx.clone();
    let genr = generation.clone();
    let pc = ptx.clone();
    ui.on_row_action(move |i, action| {
        if let Some(ui) = weak.upgrade() {
            let mut s = st.borrow_mut();
            let Some(item) = s.items.get(i as usize).cloned() else {
                return;
            };
            match action.as_str() {
                "download"
                    if item.kind == "track"
                        && item.downloadable != Some(false)
                        && has_capability(&s.plugins, &item.key.source, "download") =>
                {
                    if s.downloads.enqueue(item) {
                        s.schedule_downloads(&ui, &job, &genr);
                    }
                }
                "open" if item.kind == "track" && item.unavailable => {}
                "open" if item.kind == "track" => {
                    s.queue.items = s
                        .items
                        .iter()
                        .filter(|t| t.kind == "track" && !t.unavailable)
                        .cloned()
                        .collect();
                    s.queue.index = s.queue.items.iter().position(|t| t.key == item.key);
                    refresh_queue(&ui, &s);
                    play(&ui, &mut s, &job, &genr, &pc, item);
                }
                "open" => {
                    genr.fetch_add(1, Ordering::Relaxed);
                    s.clear_page(&ui);
                    s.page_source = item.key.source.clone();
                    s.last = Request::new(&item.kind, json!({"id":item.key.id}));
                    ui.set_busy(true);
                    send(
                        &job,
                        &genr,
                        &item.key.source,
                        &s.last.op,
                        s.last.args.clone(),
                    );
                }
                "add" => {
                    if item.key.source == "local"
                        && let Some(p) = ui
                            .get_playlist_index()
                            .checked_sub(1)
                            .and_then(|i| s.playlists.get(i as usize))
                    {
                        send(
                            &job,
                            &genr,
                            "local",
                            "addToPlaylist",
                            json!({"id":p.key.id,"track":item.key.id}),
                        );
                    } else if item.kind == "track" && !item.unavailable {
                        s.queue.items.push(item);
                        refresh_queue(&ui, &s);
                    }
                }
                "remove" if s.page_source == "local" && s.last.op == "playlist" => {
                    send(
                        &job,
                        &genr,
                        "local",
                        "removeFromPlaylist",
                        json!({"id":s.last.args["id"],"track":item.key.id}),
                    );
                    send(&job, &genr, "local", "playlist", s.last.args.clone());
                }
                "like" | "unlike" => {
                    let op = if item.kind == "album" {
                        "saveAlbum"
                    } else {
                        "like"
                    };
                    send(
                        &job,
                        &genr,
                        &item.key.source,
                        op,
                        json!({"id":item.key.id,"on":action=="like"}),
                    );
                }
                _ => {}
            }
        }
    });
    let weak = ui.as_weak();
    let st = state.clone();
    let job = tx.clone();
    let genr = generation.clone();
    let pc = ptx.clone();
    ui.on_queue_action(move |i, a| {
        if let Some(ui) = weak.upgrade() {
            let mut s = st.borrow_mut();
            if a == "remove" {
                if s.queue.index == Some(i as usize) {
                    let _ = pc.send(Command::Stop);
                    s.play_generation += 1;
                    s.loaded = false;
                    s.playing = false;
                    ui.set_playing(false);
                }
                s.queue.remove(i as usize);
                refresh_queue(&ui, &s);
            } else if let Some(t) = s.queue.items.get(i as usize).cloned() {
                s.queue.index = Some(i as usize);
                play(&ui, &mut s, &job, &genr, &pc, t);
            }
        }
    });
    let weak = ui.as_weak();
    let st = state.clone();
    let jobs = tx.clone();
    let genr = generation.clone();
    ui.on_download_action(move |index, action| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut s = st.borrow_mut();
        let index = index as usize;
        let Some(entry) = s.downloads.0.get(index) else {
            return;
        };
        match action.as_str() {
            "retry" if matches!(entry.phase, download::Phase::Failed(_)) => {
                let item = entry.item.clone();
                s.downloads.enqueue(item);
            }
            "remove" if !matches!(entry.phase, download::Phase::Active { .. }) => {
                s.downloads.0.remove(index);
            }
            _ => return,
        }
        s.schedule_downloads(&ui, &jobs, &genr);
    });
    let weak = ui.as_weak();
    let st = state.clone();
    let job = tx.clone();
    let genr = generation.clone();
    let pc = ptx.clone();
    let c = cancel.clone();
    ui.on_action(move |a| {
        let Some(ui) = weak.upgrade() else { return };
        let mut s = st.borrow_mut();
        match a.as_str() {
            "pause" => {
                if !s.loaded {
                    if let Some(t) = s
                        .queue
                        .index
                        .and_then(|i| s.queue.items.get(i).cloned())
                        .or_else(|| s.queue.advance(false, rand::random()))
                    {
                        play(&ui, &mut s, &job, &genr, &pc, t);
                    }
                } else {
                    s.playing = !s.playing;
                    let _ = pc.send(Command::Pause(!s.playing));
                    ui.set_playing(s.playing);
                }
            }
            "next" | "previous" => {
                let item = if a == "previous" {
                    let i = s.queue.index.unwrap_or(0).saturating_sub(1);
                    s.queue.index = Some(i);
                    s.queue.items.get(i).cloned()
                } else {
                    s.queue.advance(false, rand::random())
                };
                if let Some(t) = item {
                    play(&ui, &mut s, &job, &genr, &pc, t);
                }
            }
            "repeat" => {
                s.queue.repeat = (s.queue.repeat + 1) % 3;
                ui.set_repeat_mode(s.queue.repeat as i32);
                ui.set_status(["顺序播放", "列表循环", "单曲循环"][s.queue.repeat as usize].into());
            }
            "shuffle" => {
                s.queue.shuffle = !s.queue.shuffle;
                ui.set_shuffle_enabled(s.queue.shuffle);
                ui.set_status(
                    if s.queue.shuffle {
                        "随机播放"
                    } else {
                        "顺序播放"
                    }
                    .into(),
                );
            }
            "play-all" => {
                s.queue.items = s
                    .items
                    .iter()
                    .filter(|t| t.kind == "track" && !t.unavailable)
                    .cloned()
                    .collect();
                s.queue.index = None;
                if let Some(t) = s.queue.advance(false, rand::random()) {
                    refresh_queue(&ui, &s);
                    play(&ui, &mut s, &job, &genr, &pc, t);
                }
            }
            "library" => {
                s.clear_page(&ui);
                genr.fetch_add(1, Ordering::Relaxed);
                s.last = Request::new(
                    if s.source == "local" {
                        "library"
                    } else {
                        "liked"
                    },
                    json!({}),
                );
                ui.set_busy(true);
                ui.set_show_settings(false);
                ui.set_show_lyrics(false);
                send(&job, &genr, &s.source, &s.last.op, s.last.args.clone());
            }
            "more" if s.online.is_some() => {
                s.request_pages(&job, &genr, ui.get_can_filter() && ui.get_guest_only());
                s.show_search(&ui);
            }
            "more" => {
                let mut req = s.last.clone();
                let step = if s.page_source == "local" { 100 } else { 50 };
                req.args["offset"] = json!(req.args["offset"].as_u64().unwrap_or(0) + step);
                ui.set_busy(true);
                send(&job, &genr, &s.page_source, &req.op, req.args);
            }
            "save-album" | "unsave-album" => {
                if s.last.op == "album" {
                    send(
                        &job,
                        &genr,
                        &s.page_source,
                        "saveAlbum",
                        json!({"id":s.last.args["id"],"on":a=="save-album"}),
                    );
                }
            }
            "scan" => {
                ui.set_busy(true);
                send(&job, &genr, "local", "scan", json!({}));
            }
            "cancel-scan" => {
                c.store(true, Ordering::Relaxed);
            }
            "add-folder" =>
            {
                #[cfg(not(target_os = "android"))]
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    ui.set_busy(true);
                    send(&job, &genr, "local", "scan", json!({"path":p}));
                }
            }
            "import" => {
                #[cfg(not(target_os = "android"))]
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("JavaScript 插件包", &["zip"])
                    .pick_file()
                {
                    send(&job, &genr, "local", "install", json!({"path":p}));
                }
            }
            "login" => {
                s.login_started = Some(Instant::now());
                s.login_key.clear();
                s.login_inflight = true;
                ui.set_qr(slint::Image::default());
                ui.set_show_login(true);
                ui.set_login_message("正在获取二维码…".into());
                send(&job, &genr, &s.source, "loginStart", json!({}));
            }
            "cancel-login" => {
                s.login_started = None;
                s.login_key.clear();
                ui.set_show_login(false);
                send(&job, &genr, &s.source, "loginCancel", json!({}));
            }
            "logout" => {
                s.login_started = None;
                s.login_key.clear();
                send(&job, &genr, &s.source, "signOut", json!({}));
                ui.set_account_name("登录".into());
            }
            _ => {}
        }
    });
    ui.set_catalog_url(catalog::DEFAULT_URL.into());
    let (catalog_tx, catalog_rx) = mpsc::channel();
    let weak = ui.as_weak();
    let st = state.clone();
    ui.on_catalog_action(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_catalog_busy() {
            return;
        }
        let address = ui.get_catalog_url().to_string();
        let entry = if index < 0 {
            None
        } else {
            let Some(entry) = st.borrow().catalog.get(index as usize).cloned() else {
                return;
            };
            Some(entry)
        };
        if index < 0 {
            st.borrow_mut().catalog.clear();
            ui.set_catalog_rows(ModelRc::default());
        }
        ui.set_catalog_busy(true);
        ui.set_catalog_status(
            if entry.is_some() {
                "正在下载并校验插件…"
            } else {
                "正在读取在线地址…"
            }
            .into(),
        );
        let sender = catalog_tx.clone();
        std::thread::spawn(move || {
            let _ =
                sender.send(catalog::load(&address, entry.as_ref()).map_err(|e| format!("{e:#}")));
        });
    });
    let weak = ui.as_weak();
    let st = state.clone();
    let job = tx.clone();
    let genr = generation.clone();
    let pc = ptx.clone();
    ui.on_plugin_action(move |i, a| {
        if let Some(ui) = weak.upgrade() {
            let mut s = st.borrow_mut();
            if let Some((m, _)) = s.plugins.get(i as usize).cloned() {
                genr.fetch_add(1, Ordering::Relaxed);
                if s.queue
                    .index
                    .and_then(|i| s.queue.items.get(i))
                    .is_some_and(|t| t.key.source == m.id)
                {
                    s.play_generation += 1;
                    s.loaded = false;
                    s.playing = false;
                    let _ = pc.send(Command::Stop);
                    ui.set_playing(false);
                }
                send(&job, &genr, "local", a.as_str(), json!({"id":m.id}));
            }
        }
    });
    let job = tx.clone();
    let genr = generation.clone();
    ui.on_playlist_create(move |name| {
        if !name.trim().is_empty() {
            send(
                &job,
                &genr,
                "local",
                "createPlaylist",
                json!({"name":name.as_str()}),
            );
        }
    });
    let job = tx.clone();
    let genr = generation.clone();
    let st = state.clone();
    let weak = ui.as_weak();
    ui.on_playlist_manage(move |name, op| {
        if let Some(ui) = weak.upgrade() {
            let s = st.borrow();
            if let Some(p) = ui
                .get_playlist_index()
                .checked_sub(1)
                .and_then(|i| s.playlists.get(i as usize))
            {
                if op == "renamePlaylist" && name.trim().is_empty() {
                    return;
                }
                send(
                    &job,
                    &genr,
                    "local",
                    op.as_str(),
                    json!({"id":p.key.id,"name":name.as_str()}),
                );
                send(&job, &genr, "local", "local-playlists", json!({}));
                send(&job, &genr, "local", "playlists", json!({}));
            }
        }
    });
    let job = tx.clone();
    let genr = generation.clone();
    let st = state.clone();
    ui.on_quality(move |q| {
        let s = st.borrow();
        if s.source != "local" {
            send(
                &job,
                &genr,
                &s.source,
                "settings",
                json!({"quality":q.as_str()}),
            );
        }
    });
    let pc = ptx.clone();
    ui.on_seek(move |v| {
        let _ = pc.send(Command::Seek(v as f64));
    });
    let pc = ptx.clone();
    ui.on_volume(move |v| {
        let _ = pc.send(Command::Volume(v as f64));
    });
    let debounce = Rc::new(slint::Timer::default());
    let weak = ui.as_weak();
    let st = state.clone();
    let job = tx.clone();
    let genr = generation.clone();
    ui.on_suggest(move |query| {
        let Some(ui) = weak.upgrade() else { return };
        let mut s = st.borrow_mut();
        s.suggest_query = query.to_string();
        if query.chars().count() < 2 || s.source == "local" || ui.get_anonymous_source() {
            ui.set_suggestions(ModelRc::default());
            debounce.stop();
            return;
        }
        let source = s.source.clone();
        let job = job.clone();
        let genr = genr.clone();
        debounce.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(350),
            move || {
                send(
                    &job,
                    &genr,
                    &source,
                    "suggest",
                    json!({"query":query.as_str()}),
                );
            },
        );
    });
    let weak = ui.as_weak();
    let st = state.clone();
    let job = tx.clone();
    let genr = generation.clone();
    ui.on_page_link(move |i| {
        if let Some(ui) = weak.upgrade() {
            let mut s = st.borrow_mut();
            if let Some(n) = s.page_links.get(i as usize).cloned() {
                genr.fetch_add(1, Ordering::Relaxed);
                s.last = Request::new(&n.op, n.args);
                ui.set_busy(true);
                ui.set_show_lyrics(false);
                send(&job, &genr, &s.page_source, &s.last.op, s.last.args.clone());
            }
        }
    });
    let weak = ui.as_weak();
    let st = state.clone();
    let job = tx.clone();
    let genr = generation.clone();
    ui.on_page_tab(move |i| {
        if let Some(ui) = weak.upgrade() {
            let mut s = st.borrow_mut();
            if ui.get_page_tab_index() == i && !ui.get_busy() {
                return;
            }
            if let Some(n) = s.page_tabs.get(i as usize).cloned() {
                genr.fetch_add(1, Ordering::Relaxed);
                if let Some(index) = s.nav.iter().position(|v| v.op == n.op && v.args == n.args) {
                    ui.set_nav_index(index as i32);
                }
                s.last = Request::new(&n.op, n.args);
                ui.set_busy(true);
                send(&job, &genr, &s.page_source, &s.last.op, s.last.args.clone());
            }
        }
    });
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    let st = state.clone();
    let job = tx.clone();
    let genr = generation.clone();
    let pc = ptx.clone();
    #[cfg(feature = "ui-smoke")]
    let mut capture_download = std::env::var_os("MUSIC_API_CAPTURE_DOWNLOAD").is_some();
    let mut saved = Instant::now();
    let mut catalog_file: Option<tempfile::NamedTempFile> = None;
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            while let Ok(result) = catalog_rx.try_recv() {
                match result {
                    Ok(catalog::Fetched::List(entries)) => {
                        ui.set_catalog_status(format!("{} 个可用插件", entries.len()).into());
                        s.catalog = entries;
                        #[cfg(feature = "ui-smoke")]
                        if let Ok(id) = std::env::var("MUSIC_API_CAPTURE_CATALOG_ID")
                            && let Some(i) = s.catalog.iter().position(|e| e.id == id)
                        {
                            let window = ui.as_weak();
                            slint::Timer::single_shot(Duration::ZERO, move || {
                                if let Some(ui) = window.upgrade() {
                                    ui.invoke_catalog_action(i as i32);
                                }
                            });
                        }
                        update_catalog_rows(&ui, &s);
                        ui.set_catalog_busy(false);
                    }
                    Ok(catalog::Fetched::Package(file)) => {
                        send(
                            &job,
                            &genr,
                            "local",
                            "install",
                            json!({"path":file.path(),"online":true}),
                        );
                        catalog_file = Some(file);
                    }
                    Err(error) => {
                        ui.set_catalog_status(format!("读取失败 · {error}").into());
                        ui.set_catalog_busy(false);
                    }
                }
            }
            while let Ok(reply) = out_rx.try_recv() {
                if reply.op == "install" && reply.args["online"] == true {
                    drop(catalog_file.take());
                    ui.set_catalog_busy(false);
                    ui.set_catalog_status(
                        match &reply.value {
                            Ok(m) => format!(
                                "{} 已导入，可在插件列表查看启用状态",
                                m["name"].as_str().unwrap_or("插件")
                            ),
                            Err(e) => format!("导入失败 · {e}"),
                        }
                        .into(),
                    );
                }
                if reply.op == "download" {
                    let key =
                        serde_json::from_value::<ResourceKey>(reply.args["track"]["key"].clone());
                    if let Ok(key) = key {
                        let title = reply.args["track"]["title"].as_str().unwrap_or_default();
                        let error = match reply.value {
                            Ok(Value::String(_)) => None,
                            Err(e) => Some(e),
                            _ => continue,
                        };
                        let message = match &error {
                            Some(e) => format!("《{title}》下载失败：{e}"),
                            None => format!("《{title}》已在本地音乐库"),
                        };
                        if s.downloads.finish(&key, error) {
                            ui.set_download_notice(message.into());
                            s.schedule_downloads(&ui, &job, &genr);
                            if s.online.is_none()
                                && s.page_source == "local"
                                && matches!(s.last.op.as_str(), "library" | "search")
                            {
                                send(&job, &genr, "local", &s.last.op, s.last.args.clone());
                            }
                        }
                    }
                    continue;
                }
                if (["resolve", "lyrics"].contains(&reply.op.as_str())
                    && reply.generation != s.play_generation)
                    || (!["resolve", "lyrics"].contains(&reply.op.as_str())
                        && reply.generation != genr.load(Ordering::Relaxed))
                        && ![
                            "plugins",
                            "install",
                            "toggle",
                            "remove",
                            "scan",
                            "createPlaylist",
                            "local-playlists",
                        ]
                        .contains(&reply.op.as_str())
                {
                    continue;
                }
                if reply.op == "search" && reply.source != "local" && s.online.is_some() {
                    let result = reply
                        .value
                        .and_then(|v| serde_json::from_value::<Page>(v).map_err(|e| e.to_string()))
                        .map(|mut page| {
                            prepare_items(
                                &mut page.items,
                                &s.plugins,
                                &reply.source,
                                &cover_tx,
                                reply.generation,
                            );
                            page
                        });
                    s.online.as_mut().unwrap().accept(
                        &reply.source,
                        reply.args["offset"].as_u64().unwrap_or(0),
                        result,
                    );
                    s.show_search(&ui);
                    #[cfg(feature = "ui-smoke")]
                    eprintln!(
                        "Search result: source={}, visible={}, pending={}",
                        reply.source,
                        s.items.len(),
                        s.online.as_ref().unwrap().pending(false)
                    );
                    continue;
                }
                if reply.op == "persist-downloads" {
                    if let Err(error) = reply.value {
                        ui.set_download_notice(format!("下载队列保存失败：{error}").into());
                    }
                    continue;
                }
                if reply.op == "persist" {
                    continue;
                }
                if s.online
                    .as_ref()
                    .is_none_or(|search| search.pending(false) == 0 || !s.items.is_empty())
                {
                    ui.set_busy(false);
                }
                if ["loginStart", "loginPoll"].contains(&reply.op.as_str()) {
                    s.login_inflight = false;
                }
                match reply.value {
                    Err(e) => {
                        ui.set_has_error(true);
                        ui.set_status(e.clone().into());
                        if ["loginStart", "loginPoll"].contains(&reply.op.as_str()) {
                            s.login_started = None;
                            s.login_key.clear();
                        }
                        if reply.op == "resolve" {
                            s.playing = false;
                            ui.set_playing(false);
                        }
                        if ui.get_show_login() {
                            ui.set_login_message(e.into());
                        }
                    }
                    Ok(v) => match reply.op.as_str() {
                        "resolve" => {
                            ui.set_has_error(false);
                            if let Some(url) = v["url"].as_str() {
                                let _ = pc.send(Command::Open {
                                    url: url.into(),
                                    referrer: v["referrer"].as_str().map(str::to_owned),
                                });
                                let _ = pc.send(Command::Pause(false));
                                s.loaded = true;
                                s.playing = true;
                                ui.set_playing(true);
                                let _ = job.send(Job {
                                    source: reply.source,
                                    request: Request::new("lyrics", reply.args),
                                    generation: reply.generation,
                                });
                            }
                        }
                        "lyrics" => {
                            let text = v["lrc"].as_str().unwrap_or("");
                            s.lines = parse_lrc(text);
                            s.translations = parse_lrc(v["translation"].as_str().unwrap_or(""));
                            ui.set_lyrics(if s.lines.is_empty() {
                                if text.is_empty() {
                                    "暂无歌词".into()
                                } else {
                                    text.into()
                                }
                            } else {
                                s.lines
                                    .iter()
                                    .map(|(_, line)| line.as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n\n")
                                    .into()
                            });
                        }
                        "suggest" => {
                            if reply.source == s.source
                                && reply.args["query"].as_str() == Some(s.suggest_query.as_str())
                            {
                                ui.set_suggestions(ModelRc::new(VecModel::from(
                                    v.as_array()
                                        .into_iter()
                                        .flatten()
                                        .take(5)
                                        .filter_map(|v| v.as_str().map(Into::into))
                                        .collect::<Vec<_>>(),
                                )));
                            }
                        }
                        "account" => {
                            ui.set_account_name(v["name"].as_str().unwrap_or("登录").into());
                        }
                        "settings" => {
                            if reply.source == s.source {
                                ui.set_quality_index(
                                    ["follow", "standard", "high", "lossless", "hires"]
                                        .iter()
                                        .position(|q| Some(*q) == v.as_str())
                                        .unwrap_or(0) as i32,
                                );
                            }
                        }
                        "loginStart" => {
                            if s.login_started.is_some() {
                                s.login_key = v["key"].as_str().unwrap_or("").into();
                                set_login_qr(&ui, v["qr"].as_str().unwrap_or(""));
                                ui.set_login_message(
                                    v["message"]
                                        .as_str()
                                        .unwrap_or("请使用音乐源官方 App 扫码")
                                        .into(),
                                );
                            }
                        }
                        "loginPoll" => {
                            if s.login_started.is_some() {
                                if let Some(qr) = v["qr"].as_str() {
                                    set_login_qr(&ui, qr);
                                }
                                if let Some(message) = v["message"].as_str() {
                                    ui.set_login_message(message.into());
                                }
                                match v["status"].as_str() {
                                    Some("expired") => {
                                        send(&job, &genr, &s.source, "loginStart", json!({}))
                                    }
                                    Some("scanned") => {
                                        ui.set_login_message("已扫码，请在手机确认".into())
                                    }
                                    Some("success") => {
                                        s.login_started = None;
                                        s.login_key.clear();
                                        ui.set_show_login(false);
                                        send(&job, &genr, &s.source, "account", json!({}));
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "plugins" => {
                            let old_source = s.source.clone();
                            s.plugins = serde_json::from_value(v).unwrap_or_default();
                            update_plugins(&ui, &mut s);
                            if s.online.is_some() && s.last.op == "search" && s.source == old_source
                            {
                                genr.fetch_add(1, Ordering::Relaxed);
                                s.clear_page(&ui);
                                s.last.args["offset"] = json!(0);
                                if s.source == "local" || !s.begin_online(&ui, &job, &genr) {
                                    send(&job, &genr, &s.source, "search", s.last.args.clone());
                                }
                            }
                            if s.source != old_source {
                                genr.fetch_add(1, Ordering::Relaxed);
                                s.clear_page(&ui);
                                ui.set_anonymous_source(false);
                                s.last = Request::new("library", json!({}));
                                s.login_started = None;
                                s.login_key.clear();
                                ui.set_show_login(false);
                                ui.set_account_name("登录".into());
                                send(&job, &genr, "local", "library", json!({}));
                            }
                        }
                        "install" | "toggle" | "remove" => {
                            send(&job, &genr, "local", "plugins", json!({}));
                            ui.set_status("插件已更新".into());
                        }
                        "scan" => {
                            ui.set_status(if v.as_array().is_some_and(|v| v.is_empty()) {
                                "扫描完成".into()
                            } else {
                                v.to_string().into()
                            });
                            if s.online.is_none()
                                && s.page_source == "local"
                                && matches!(s.last.op.as_str(), "library" | "search")
                            {
                                send(&job, &genr, "local", &s.last.op, s.last.args.clone());
                            }
                        }
                        "local-playlists" => {
                            if let Ok(p) = serde_json::from_value::<Page>(v) {
                                s.playlists = p.items;
                                ui.set_playlist_names(ModelRc::new(VecModel::from(
                                    std::iter::once("加入播放队列".into())
                                        .chain(s.playlists.iter().map(|p| p.title.clone().into()))
                                        .collect::<Vec<_>>(),
                                )));
                            }
                        }
                        "createPlaylist" => {
                            send(&job, &genr, "local", "local-playlists", json!({}));
                            send(&job, &genr, "local", "playlists", json!({}));
                        }
                        _ => {
                            let mut links: Vec<Navigation> =
                                serde_json::from_value(v["links"].clone()).unwrap_or_default();
                            links.retain(|n| n.op == "browse");
                            if let Ok(mut page) = serde_json::from_value::<Page>(v) {
                                if reply.source != s.page_source
                                    || (page.offset > 0
                                        && s.last.op == reply.op
                                        && s.last.args["offset"].as_u64() == Some(page.offset))
                                {
                                    continue;
                                }
                                s.last = Request::new(&reply.op, reply.args.clone());
                                let local_miss = reply.source == "local"
                                    && reply.op == "search"
                                    && page.offset == 0
                                    && page.items.is_empty()
                                    && reply.args["query"]
                                        .as_str()
                                        .is_some_and(|q| !q.trim().is_empty());
                                if local_miss && s.begin_online(&ui, &job, &genr) {
                                    continue;
                                }
                                ui.set_search_page(reply.op == "search");
                                ui.set_can_download(has_capability(
                                    &s.plugins,
                                    &reply.source,
                                    "download",
                                ));
                                ui.set_page_account_actions(
                                    s.plugins
                                        .iter()
                                        .find(|(m, _)| m.id == reply.source)
                                        .is_none_or(|(m, _)| m.supports_likes()),
                                );
                                show_page_navigation(&ui, &mut s, links);
                                ui.set_album_detail(reply.op == "album" && reply.source != "local");
                                ui.set_home_page(reply.op == "home");
                                ui.set_local_playlist(
                                    reply.op == "playlist" && reply.source == "local",
                                );
                                ui.set_cards(
                                    !page.items.is_empty()
                                        && page.items.iter().all(|i| i.kind != "track"),
                                );
                                prepare_items(
                                    &mut page.items,
                                    &s.plugins,
                                    &reply.source,
                                    &cover_tx,
                                    reply.generation,
                                );
                                ui.set_heading(page.title.into());
                                ui.set_has_error(false);
                                ui.set_more(page.more);
                                if page.offset > 0 {
                                    s.items.extend(page.items);
                                } else {
                                    s.items = page.items;
                                }
                                s.update_rows(&ui);
                                let status = if local_miss {
                                    "本地未找到匹配内容，可在设置中安装并启用在线音乐源".to_owned()
                                } else {
                                    format!("{} 项", s.items.len())
                                };
                                ui.set_status(status.into());
                            } else {
                                ui.set_status("操作完成".into());
                            }
                        }
                    },
                }
            }
            while let Ok(progress) = progress_rx.try_recv() {
                #[cfg(feature = "ui-smoke")]
                if progress.bytes > 0
                    && s.downloads.index(&progress.key).is_some_and(|i| {
                        matches!(
                            s.downloads.0[i].phase,
                            download::Phase::Active { bytes: 0, .. }
                        )
                    })
                {
                    eprintln!(
                        "Download progress: source={}, bytes={}, total={:?}",
                        progress.key.source, progress.bytes, progress.total
                    );
                }
                s.show_progress(&ui, progress);
            }
            #[cfg(feature = "ui-smoke")]
            if capture_download && s.online.as_ref().is_some_and(|q| q.pending(false) == 0) {
                capture_download = false;
                let count = std::env::var("MUSIC_API_CAPTURE_COUNT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                let indices: Vec<_> = s
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| {
                        t.kind == "track"
                            && t.downloadable != Some(false)
                            && has_capability(&s.plugins, &t.key.source, "download")
                    })
                    .take(count)
                    .map(|(i, _)| i as i32)
                    .collect();
                let window = ui.as_weak();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if let Some(ui) = window.upgrade() {
                        for i in indices {
                            ui.invoke_row_action(i, "download".into());
                        }
                    }
                });
            }
            let mut queue_covers_changed = false;
            while let Ok((key, path, g)) = cover_rx.try_recv() {
                for item in &mut s.queue.items {
                    if item.key == key {
                        item.cover = path.clone();
                        queue_covers_changed = true;
                    }
                }
                if s.queue
                    .index
                    .and_then(|i| s.queue.items.get(i))
                    .is_some_and(|t| t.key == key)
                {
                    ui.set_now_cover(image_path(&path));
                }
                if g == genr.load(Ordering::Relaxed) {
                    if let Some(search) = &mut s.online {
                        for item in &mut search.items {
                            if item.key == key {
                                item.cover = path.clone();
                            }
                        }
                    }
                    for (i, item) in s.items.iter_mut().enumerate() {
                        if item.key == key {
                            item.cover = path.clone();
                            let model = ui.get_rows();
                            if let Some(mut row) = model.row_data(i) {
                                row.cover = image_path(&path);
                                model.set_row_data(i, row);
                            }
                        }
                    }
                }
            }
            if queue_covers_changed {
                refresh_queue(&ui, &s);
            }
            while let Ok(e) = player.events.try_recv() {
                match e {
                    PlayerEvent::Ready => {}
                    PlayerEvent::Error(e) => {
                        ui.set_has_error(true);
                        ui.set_status(e.into());
                        s.loaded = false;
                        s.playing = false;
                        ui.set_playing(false);
                    }
                    PlayerEvent::Position(p, d) => {
                        ui.set_position(p as f32);
                        ui.set_duration(d as f32);
                        ui.set_time_label(
                            format!(
                                "{:02}:{:02} / {:02}:{:02}",
                                p as u64 / 60,
                                p as u64 % 60,
                                d as u64 / 60,
                                d as u64 % 60
                            )
                            .into(),
                        );
                        let current = s
                            .lines
                            .iter()
                            .rev()
                            .find(|(t, _)| *t <= (p * 1000.0) as u64)
                            .map(|(_, v)| v.as_str())
                            .unwrap_or("");
                        ui.set_lyric(current.into());
                        let translated = s
                            .translations
                            .iter()
                            .rev()
                            .find(|(t, _)| *t <= (p * 1000.0) as u64)
                            .map(|(_, v)| v.as_str())
                            .unwrap_or("");
                        ui.set_translated_lyric(translated.into());
                    }
                    PlayerEvent::Ended => {
                        s.loaded = false;
                        s.playing = false;
                        ui.set_playing(false);
                        if let Some(t) = s.queue.advance(true, rand::random()) {
                            play(&ui, &mut s, &job, &genr, &pc, t);
                        }
                    }
                }
            }
            if s.login_started
                .is_some_and(|t| t.elapsed() > Duration::from_secs(480))
            {
                s.login_started = None;
                s.login_key.clear();
                ui.set_login_message("登录已超时，请关闭后重试".into());
                send(&job, &genr, &s.source, "loginCancel", json!({}));
            }
            if s.login_started.is_some()
                && !s.login_inflight
                && !s.login_key.is_empty()
                && s.last_poll.elapsed() > Duration::from_secs(2)
            {
                s.login_inflight = true;
                s.last_poll = Instant::now();
                send(
                    &job,
                    &genr,
                    &s.source,
                    "loginPoll",
                    json!({"key":s.login_key}),
                );
            }
            if saved.elapsed() > Duration::from_secs(3) {
                saved = Instant::now();
                send(
                    &job,
                    &genr,
                    "local",
                    "persist",
                    json!({"queue":s.queue,"dark":ui.get_dark()}),
                );
            }
        },
    );
    send(&tx, &generation, "local", "library", json!({}));
    send(&tx, &generation, "local", "local-playlists", json!({}));
    send(&tx, &generation, "local", "scan", json!({}));
    #[cfg(feature = "ui-smoke")]
    let capture_timer = slint::Timer::default();
    #[cfg(feature = "ui-smoke")]
    {
        if let Ok(source) = std::env::var("MUSIC_API_CAPTURE_SOURCE") {
            let index = state.borrow().sources.iter().position(|id| id == &source);
            if let Some(index) = index {
                ui.set_source_index(index as i32);
                ui.invoke_select_source(index as i32);
            }
        }
        if let Some(path) = std::env::var_os("MUSIC_API_CAPTURE") {
            ui.set_guest_only(std::env::var_os("MUSIC_API_CAPTURE_GUEST").is_some());
            ui.set_show_downloads(
                std::env::var("MUSIC_API_CAPTURE_SCREEN").as_deref() == Ok("downloads"),
            );
            match std::env::var("MUSIC_API_CAPTURE_THEME").as_deref() {
                Ok("light") => ui.set_dark(false),
                Ok("dark") => ui.set_dark(true),
                _ => {}
            }
            if std::env::var("MUSIC_API_CAPTURE_SCREEN").as_deref() == Ok("catalog") {
                ui.set_show_settings(true);
                ui.set_show_catalog(true);
                ui.invoke_catalog_action(-1);
            }
            if std::env::var("MUSIC_API_CAPTURE_SCREEN").as_deref() == Ok("settings") {
                ui.set_show_settings(true);
                if let Ok(source) = std::env::var("MUSIC_API_CAPTURE_TOGGLE") {
                    let index = state
                        .borrow()
                        .plugins
                        .iter()
                        .position(|(m, _)| m.id == source);
                    if let Some(index) = index {
                        ui.invoke_plugin_action(index as i32, "toggle".into());
                    }
                }
            }
            let window = ui.as_weak();
            if let Some(index) = std::env::var("MUSIC_API_CAPTURE_LINK")
                .ok()
                .and_then(|s| s.parse::<i32>().ok())
            {
                let link_window = ui.as_weak();
                slint::Timer::single_shot(Duration::from_secs(8), move || {
                    if let Some(ui) = link_window.upgrade() {
                        ui.invoke_page_link(index);
                    }
                });
            }
            if let Ok(title) = std::env::var("MUSIC_API_CAPTURE_TAB") {
                let tab_state = state.clone();
                let tab_window = ui.as_weak();
                slint::Timer::single_shot(Duration::from_secs(4), move || {
                    let index = tab_state
                        .borrow()
                        .page_tabs
                        .iter()
                        .position(|n| n.title == title);
                    if let (Some(ui), Some(index)) = (tab_window.upgrade(), index) {
                        ui.invoke_page_tab(index as i32);
                    }
                });
            }
            if let Ok(title) = std::env::var("MUSIC_API_CAPTURE_NAV") {
                let index = state.borrow().nav.iter().position(|n| n.title == title);
                let nav_window = ui.as_weak();
                slint::Timer::single_shot(Duration::from_secs(1), move || {
                    if let (Some(ui), Some(i)) = (nav_window.upgrade(), index) {
                        ui.invoke_navigate(i as i32);
                    }
                });
            }
            if let Ok(screen) = std::env::var("MUSIC_API_CAPTURE_SCREEN") {
                let screen_window = ui.as_weak();
                slint::Timer::single_shot(Duration::from_secs(9), move || {
                    if let Some(ui) = screen_window.upgrade() {
                        match screen.as_str() {
                            "lyrics" => ui.set_show_lyrics(true),
                            "queue" => ui.set_show_queue(true),
                            "login" => ui.invoke_action("login".into()),
                            _ => {}
                        }
                    }
                });
            }
            if let Ok(query) = std::env::var("MUSIC_API_CAPTURE_QUERY") {
                let search_window = ui.as_weak();
                slint::Timer::single_shot(Duration::from_secs(1), move || {
                    if let Some(ui) = search_window.upgrade() {
                        ui.invoke_search(query.into(), 0);
                    }
                });
            }
            if std::env::var_os("MUSIC_API_CAPTURE_PLAY").is_some() {
                let play_window = ui.as_weak();
                slint::Timer::single_shot(Duration::from_secs(6), move || {
                    if let Some(ui) = play_window.upgrade() {
                        ui.invoke_volume(5.0);
                        ui.invoke_row_action(0, "open".into());
                    }
                });
            }
            let retry_download = std::env::var_os("MUSIC_API_CAPTURE_RETRY").is_some();
            if retry_download {
                let queue_window = ui.as_weak();
                slint::Timer::single_shot(Duration::from_secs(1), move || {
                    if let Some(ui) = queue_window.upgrade() {
                        let queue = ui.get_downloads();
                        if let Some(i) = (0..queue.row_count())
                            .find(|&i| queue.row_data(i).is_some_and(|r| r.failed))
                        {
                            ui.invoke_download_action(i as i32, "retry".into());
                        }
                    }
                });
            }
            let wait_for_download =
                retry_download || std::env::var_os("MUSIC_API_CAPTURE_DOWNLOAD").is_some();
            let started = Instant::now();
            let interval = if wait_for_download {
                Duration::from_millis(500)
            } else {
                Duration::from_secs(15)
            };
            capture_timer.start(slint::TimerMode::Repeated, interval, move || {
                if let Some(ui) = window.upgrade() {
                    if ui.get_show_catalog() && ui.get_catalog_busy() && started.elapsed() < Duration::from_secs(120) { return; }
                    if ui.get_show_catalog() { eprintln!("Online catalog: rows={}, status={}",ui.get_catalog_rows().row_count(),ui.get_catalog_status()); }
                    if wait_for_download && !ui.get_has_error() && started.elapsed() < Duration::from_secs(120) &&
                        (ui.get_download_notice().is_empty() || ui.get_download_pending() > 0) { return; }
                    eprintln!(
                        "UI state: heading={}, tab={}, title={}, position={}, duration={}, playing={}, rows={}, status={}, download={}",
                        ui.get_heading(),
                        ui.get_page_tab_index(),
                        ui.get_now_title(),
                        ui.get_position(),
                        ui.get_duration(),
                        ui.get_playing(),
                        ui.get_rows().row_count(),
                        ui.get_status(),
                        ui.get_download_notice()
                    );
                    for i in 0..ui.get_downloads().row_count() {
                        if let Some(entry)=ui.get_downloads().row_data(i) { eprintln!("Download task: {} | {}",entry.title,entry.status); }
                    }
                    match ui.window().take_snapshot() {
                        Ok(pixels) => {
                            let result = image::save_buffer(
                                Path::new(&path),
                                pixels.as_bytes(),
                                pixels.width(),
                                pixels.height(),
                                image::ColorType::Rgba8,
                            );
                            eprintln!("Renderer snapshot: {result:?}");
                        }
                        Err(e) => eprintln!("Renderer snapshot failed: {e}"),
                    }
                }
                let _ = slint::quit_event_loop();
            });
        }
    }
    ui.run()?;
    cancel.store(true, Ordering::Relaxed);
    let s = state.borrow();
    let shutdown_db = Library::open(&root)?;
    shutdown_db.set("queue", json!(s.queue))?;
    shutdown_db.set("download-queue", s.downloads.snapshot())?;
    send(
        &tx,
        &generation,
        "local",
        "persist-downloads",
        s.downloads.snapshot(),
    );
    shutdown_db.set("dark", json!(ui.get_dark()))?;
    send(
        &tx,
        &generation,
        "local",
        "persist",
        json!({"queue":s.queue,"dark":ui.get_dark()}),
    );
    let _ = ptx.send(Command::Quit);
    Ok(())
}
fn update_catalog_rows(ui: &AppWindow, s: &UiState) {
    ui.set_catalog_rows(ModelRc::new(VecModel::from(
        s.catalog
            .iter()
            .map(|entry| {
                let installed = s.plugins.iter().find(|(m, _)| m.id == entry.id);
                let same = installed.is_some_and(|(m, _)| m.version == entry.version);
                CatalogRow {
                    title: format!("{}  {}", entry.name, entry.version).into(),
                    description: entry.description.as_str().into(),
                    permissions: format!(
                        "API · {}   媒体 · {}",
                        entry.network.join(", "),
                        entry.media.join(", ")
                    )
                    .into(),
                    action_label: if same {
                        "已安装"
                    } else if installed.is_some() {
                        "更新"
                    } else {
                        "安装"
                    }
                    .into(),
                    can_install: !same,
                }
            })
            .collect::<Vec<_>>(),
    )));
}
fn update_plugins(ui: &AppWindow, s: &mut UiState) {
    update_catalog_rows(ui, s);
    let mut names = vec!["本地音乐".into()];
    s.sources = vec!["local".into()];
    for (m, on) in &s.plugins {
        if *on {
            s.sources.push(m.id.clone());
            names.push(m.name.clone().into());
        }
    }
    ui.set_sources(ModelRc::new(VecModel::from(names)));
    ui.set_plugin_rows(ModelRc::new(VecModel::from(
        s.plugins
            .iter()
            .map(|(m, enabled)| PluginRow {
                name: format!("{}  {}", m.name, m.version).into(),
                permissions: format!(
                    "API · {}\n媒体 · {}",
                    m.network.join(", "),
                    if m.media.is_empty() {
                        "HTTPS 媒体地址".into()
                    } else {
                        m.media.join(", ")
                    }
                )
                .into(),
                enabled: *enabled,
            })
            .collect::<Vec<_>>(),
    )));
    if let Some(index) = s.sources.iter().position(|id| id == &s.source) {
        ui.set_source_index(index as i32);
    } else {
        s.source = "local".into();
        s.nav = local_nav();
        ui.set_source_index(0);
        ui.set_has_quality_setting(false);
        show_nav(ui, s);
    }
}
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: slint::android::AndroidApp) {
    slint::android::init(app.clone()).unwrap();
    if let Some(path) = app.internal_data_path() {
        let _ = run_at(path);
    }
}

fn set_login_qr(ui: &AppWindow, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Ok(qr) = qrcode::QrCode::new(text) {
        let width = qr.width() + 8;
        let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(width as u32, width as u32);
        for y in 0..width {
            for x in 0..width {
                let black = x >= 4
                    && y >= 4
                    && x < width - 4
                    && y < width - 4
                    && qr[(x - 4, y - 4)] == qrcode::Color::Dark;
                let c = if black { 0 } else { 255 };
                buf.make_mut_slice()[y * width + x] = slint::Rgb8Pixel::new(c, c, c);
            }
        }
        ui.set_qr(slint::Image::from_rgb8(buf));
    }
}
