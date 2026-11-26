use super::*;
use serde::{Deserialize, Serialize};

pub(super) const KEY: &str = "browse-location";
const KINDS: [&str; 4] = ["track", "album", "artist", "playlist"];

#[derive(Serialize, Deserialize)]
pub(super) struct Location {
    pub source: String,
    page_source: String,
    page: Request,
    #[serde(default)]
    sidebar: Option<Request>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tabs: Vec<Navigation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    heartbeat: Option<String>,
}

// A location describes a first-page read. Never replay account or editing operations on startup.
fn page_request(op: &str, args: &Value) -> Option<Request> {
    let kind = || {
        args["kind"]
            .as_str()
            .filter(|v| KINDS.contains(v))
            .unwrap_or("track")
    };
    let required = |key: &str| args[key].as_str().filter(|v| !v.is_empty());
    let args = match op {
        "home" | "liked" | "playlists" | "savedAlbums" | "recent" => json!({}),
        "library" => json!({"kind":kind()}),
        "playlist" | "album" | "artist" => json!({"id":required("id")?}),
        "browse" => json!({"page":required("page")?}),
        "search" => json!({"query":args["query"].as_str()?, "kind":kind()}),
        _ => return None,
    };
    Some(Request::new(op, args))
}

impl Location {
    pub fn read(db: &Library) -> Option<Self> {
        let mut location: Self = serde_json::from_value(db.get(KEY)).ok()?;
        location.page = page_request(&location.page.op, &location.page.args)?;
        if location.page_source == "local"
            && location.page.op == "playlist"
            && !db.has_playlist(location.page.args["id"].as_str()?).ok()?
        {
            location.page = Request::new("playlists", json!({}));
            location.sidebar = Some(location.page.clone());
        }
        location.tabs.retain(|n| {
            matches!(n.op.as_str(), "home" | "browse") && page_request(&n.op, &n.args).is_some()
        });
        Some(location)
    }

    pub fn capture(ui: &AppWindow, state: &UiState) -> Self {
        let sidebar = state
            .nav
            .get(ui.get_nav_index() as usize)
            .and_then(|n| page_request(&n.op, &n.args));
        let page = page_request(&state.last.op, &state.last.args)
            .or_else(|| sidebar.clone())
            .unwrap_or_else(|| Request::new("home", json!({})));
        let tabs = if page.op == "browse" {
            state.page_tabs.clone()
        } else {
            Vec::new()
        };
        Self {
            source: state.source.clone(),
            page_source: state.page_source.clone(),
            page,
            sidebar,
            tabs,
            heartbeat: state.resume_heartbeat.clone().or_else(|| {
                state
                    .on_heartbeat_page(ui)
                    .then(|| ui.get_heartbeat_playlist_id().to_string())
            }),
        }
    }
}

impl UiState {
    pub(super) fn open_source(
        &mut self,
        ui: &AppWindow,
        jobs: &mpsc::Sender<Job>,
        generation: &AtomicU64,
        source: &str,
        location: Option<Location>,
    ) {
        generation.fetch_add(1, Ordering::Relaxed);
        if self.login_started.is_some() {
            send(jobs, generation, &self.source, "loginCancel", json!({}));
        }
        self.login_started = None;
        self.login_key.clear();
        self.login_inflight = false;
        ui.set_show_login(false);

        let index = self.sources.iter().position(|id| id == source).unwrap_or(0);
        self.source = self.sources[index].clone();
        ui.set_source_index(index as i32);
        self.clear_page(ui);
        let plugin = self
            .plugins
            .iter()
            .find(|(m, on)| *on && m.id == self.source)
            .map(|(m, _)| m);
        let anonymous = has_capability(&self.plugins, &self.source, "anonymous");
        let settings = has_capability(&self.plugins, &self.source, "settings");
        ui.set_anonymous_source(anonymous);
        ui.set_has_quality_setting(
            plugin.is_some_and(|m| m.settings.iter().any(|s| s.key == "quality")),
        );
        ui.set_account_name("登录".into());
        self.nav = plugin.map_or_else(local_nav, |m| m.navigation.clone());
        show_nav(ui, self);
        ui.set_nav_index(0);
        ui.set_show_settings(false);
        ui.set_show_lyrics(false);
        self.page_tabs.clear();
        self.suggest_query.clear();
        ui.set_suggestions(ModelRc::default());

        self.last = Request::new(
            if self.source == "local" {
                "library"
            } else {
                "home"
            },
            json!({}),
        );
        if let Some(location) =
            location.filter(|l| l.source == self.source && self.sources.contains(&l.page_source))
        {
            let selected = location.sidebar.as_ref().unwrap_or(&location.page);
            if let Some(index) = self
                .nav
                .iter()
                .position(|n| n.op == selected.op && n.args == selected.args)
            {
                ui.set_nav_index(index as i32);
            }
            self.page_source = location.page_source;
            self.last = location.page;
            self.page_tabs = location.tabs;
            self.resume_heartbeat = location.heartbeat;
        }
        ui.set_busy(true);
        let search = self.last.op == "search";
        if search {
            ui.set_search_text(self.last.args["query"].as_str().unwrap_or_default().into());
            ui.set_search_kind(
                KINDS
                    .iter()
                    .position(|k| self.last.args["kind"] == *k)
                    .unwrap_or(0) as i32,
            );
            ui.set_search_page(true);
        }
        if !search || self.source == "local" || !self.begin_online(ui, jobs, generation) {
            send(
                jobs,
                generation,
                &self.page_source,
                &self.last.op,
                self.last.args.clone(),
            );
        }
        if self.source != "local" && !anonymous {
            send(jobs, generation, &self.source, "account", json!({}));
            if settings {
                send(jobs, generation, &self.source, "settings", json!({}));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_existing_playlist_by_id_and_falls_back_if_deleted() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut db = Library::open(dir.path())?;
        assert!(Location::read(&db).is_none());
        let id = db.call(Request::new("createPlaylist", json!({"name":"晚间收藏"})))?;
        db.set(KEY, json!({"source":"local","page_source":"local","page":{"op":"playlist","args":{"id":id,"offset":100}}}))?;
        let request = Location::read(&db).unwrap().page;
        assert_eq!(request.args, json!({"id":id}));
        assert_eq!(db.call(request)?["title"], "晚间收藏");
        db.call(Request::new("deletePlaylist", json!({"id":id})))?;
        let location = Location::read(&db).unwrap();
        assert_eq!(location.page.op, "playlists");
        assert_eq!(location.sidebar.unwrap().op, "playlists");
        db.set(KEY, json!({"source":"local","page_source":"local","page":{"op":"deletePlaylist","args":{"id":id}}}))?;
        assert!(Location::read(&db).is_none());
        Ok(())
    }

    #[cfg(feature = "ui-smoke")]
    #[test]
    fn restores_source_sidebar_and_detail_without_requesting_a_competing_home_page() -> Result<()> {
        let _clock = crate::test_ui::test_clock()?;
        let ui = AppWindow::new()?;
        let plugin = serde_json::from_value(
            json!({"id":"netease","abi":2,"name":"网易云","version":"1.3.0","capabilities":["settings"],"network":[],"media":[],
            "navigation":[{"title":"首页","op":"home","args":{}},{"title":"喜欢的歌曲","op":"liked","args":{}},{"title":"我的歌单","op":"playlists","args":{}}]}),
        )?;
        let mut state = UiState::new(Queue::default(), Default::default(), vec![(plugin, true)]);
        update_plugins(&ui, &mut state);
        let (jobs, rx) = mpsc::channel();
        let generation = AtomicU64::new(0);
        let detail = json!({"source":"netease","page_source":"netease","page":{"op":"playlist","args":{"id":"3344"}},"sidebar":{"op":"playlists","args":{}}});
        let loc = || serde_json::from_value(detail.clone()).unwrap();
        state.open_source(&ui, &jobs, &generation, "netease", Some(loc()));
        let requests: Vec<_> = rx
            .try_iter()
            .map(|j| (j.source, j.request.op, j.request.args, j.generation))
            .collect();
        assert_eq!(ui.get_source_index(), 1);
        assert_eq!(ui.get_nav_index(), 2);
        assert_eq!(
            requests.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(),
            ["playlist", "account", "settings"]
        );
        assert!(requests.iter().all(|r| r.0 == "netease" && r.3 == 1));
        assert_eq!(requests[0].2, json!({"id":"3344"}));
        assert_eq!(json!(Location::capture(&ui, &state)), detail);

        let cross = serde_json::from_value(json!({"source":"local","page_source":"netease","page":{"op":"playlist","args":{"id":"3344"}}})).unwrap();
        state.open_source(&ui, &jobs, &generation, "local", Some(cross));
        assert_eq!(ui.get_source_index(), 0);
        assert_eq!(rx.try_recv()?.source, "netease");

        state.plugins[0].1 = false;
        update_plugins(&ui, &mut state);
        state.open_source(&ui, &jobs, &generation, "netease", Some(loc()));
        assert_eq!(ui.get_source_index(), 0);
        let request = rx.try_recv()?;
        assert_eq!(
            (request.source.as_str(), request.request.op.as_str()),
            ("local", "library")
        );
        assert!(rx.try_recv().is_err());
        Ok(())
    }
}
