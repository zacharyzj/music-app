use super::*;
use anyhow::{Context, ensure};
use std::collections::VecDeque;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct Import {
    ticket: u64,
    sources: Vec<String>,
    choices: Vec<Item>,
    choices_offset: u64,
    targets: Vec<Item>,
    destination: String,
    added: usize,
    replaced: usize,
    missing: VecDeque<Item>,
    search: Option<search::Search>,
    query: String,
}

pub(super) fn read_playlist(
    source: &str,
    mut read: impl FnMut(u64) -> Result<Page>,
) -> Result<Page> {
    let mut result = Page::default();
    let mut seen = HashSet::new();
    let mut offset = 0;
    loop {
        let page = read(offset)?;
        if offset == 0 {
            result.title = page.title;
        }
        let count = page.items.len();
        let before = result.items.len();
        for mut track in page.items.into_iter().filter(|t| t.kind == "track") {
            ensure!(!track.key.id.is_empty(), "插件返回了缺少编号的歌曲");
            if seen.insert(track.key.id.clone()) {
                track.key.source = source.to_owned();
                result.items.push(track);
            }
        }
        if !page.more {
            return Ok(result);
        }
        ensure!(
            result.items.len() > before,
            "插件返回了重复或空的歌单分页，导入已取消"
        );
        offset += count as u64;
    }
}

fn playlist_id(source: &str, input: &str) -> Result<String> {
    let input = input.trim();
    ensure!(!input.is_empty(), "请输入来源歌单 ID");
    if source != "netease" || !input.contains("://") {
        return Ok(input.to_owned());
    }
    let mut url = reqwest::Url::parse(input)?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && matches!(url.host_str(), Some("music.163.com" | "y.music.163.com")),
        "请填写所选插件的歌单 ID"
    );
    if let Some(fragment) = url.fragment().and_then(|s| s.strip_prefix('/')) {
        url = url.join(&format!("/{fragment}"))?;
    }
    let segments: Vec<_> = url.path_segments().into_iter().flatten().collect();
    let pos = segments
        .iter()
        .position(|s| *s == "playlist")
        .context("请输入网易云歌单链接")?;
    url.query_pairs()
        .find(|(key, _)| key == "id")
        .map(|(_, id)| id.into_owned())
        .or_else(|| segments.get(pos + 1).map(|id| id.to_string()))
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        .context("链接中没有有效的歌单 ID")
}

fn request(jobs: &mpsc::Sender<Job>, ticket: u64, source: &str, op: &str, args: Value) {
    let _ = jobs.send(Job {
        source: source.into(),
        generation: ticket,
        request: Request::new(op, args),
    });
}

fn labels(values: impl IntoIterator<Item = String>) -> ModelRc<slint::SharedString> {
    ModelRc::new(VecModel::from(
        values.into_iter().map(Into::into).collect::<Vec<_>>(),
    ))
}

impl UiState {
    pub(super) fn refresh_import_sources(&mut self, ui: &AppWindow) {
        if ui.get_import_step() == ImportStep::Form {
            let selected = self
                .import
                .sources
                .get(ui.get_import_source_index() as usize)
                .cloned()
                .unwrap_or_default();
            self.import_sources(ui, &selected);
            if !self.import.sources.contains(&selected) {
                ui.set_import_id("".into());
                self.reset_choices(ui);
            }
        }
    }

    fn import_sources(&mut self, ui: &AppWindow, selected: &str) {
        let plugins: Vec<_> = self
            .plugins
            .iter()
            .filter(|(m, on)| *on && !m.capabilities.iter().any(|c| c == "tracksOnly"))
            .collect();
        self.import.sources = plugins.iter().map(|(m, _)| m.id.clone()).collect();
        ui.set_import_sources(labels(plugins.iter().map(|(m, _)| m.name.clone())));
        ui.set_import_source_index(
            self.import
                .sources
                .iter()
                .position(|s| s == selected)
                .unwrap_or(0) as i32,
        );
    }

    fn open_import(&mut self, ui: &AppWindow, jobs: &mpsc::Sender<Job>) {
        let ticket = self.import.ticket.wrapping_add(1);
        self.import = Import {
            ticket,
            ..Default::default()
        };
        let source = self.page_source.clone();
        self.import_sources(ui, &source);
        self.reset_choices(ui);
        let is_remote = ui.get_remote_playlist() && self.import.sources.contains(&source);
        ui.set_import_id(
            if is_remote {
                self.last.args["id"].as_str().unwrap_or_default()
            } else {
                ""
            }
            .into(),
        );
        ui.set_import_name("".into());
        ui.set_import_targets(labels(["创建新歌单".to_owned()]));
        ui.set_import_target_index(0);
        ui.set_import_status("正在读取本地歌单…".into());
        ui.set_import_candidates(ModelRc::default());
        ui.set_import_step(ImportStep::Form);
        ui.set_import_busy(true);
        request(jobs, ticket, "local", "import-targets", json!({}));
    }

    fn reset_choices(&mut self, ui: &AppWindow) {
        self.import.choices.clear();
        self.import.choices_offset = 0;
        ui.set_import_choices(ModelRc::default());
        ui.set_import_can_browse(
            self.import
                .sources
                .get(ui.get_import_source_index() as usize)
                .is_some_and(|id| {
                    self.plugins.iter().any(|(m, on)| {
                        *on && m.id == *id && m.navigation.iter().any(|link| link.op == "playlists")
                    })
                }),
        );
    }

    fn start_import(&self, ui: &AppWindow, jobs: &mpsc::Sender<Job>) -> Result<()> {
        let source = self
            .import
            .sources
            .get(ui.get_import_source_index() as usize)
            .context("请先安装并启用支持歌单的插件")?;
        ensure!(
            self.plugins.iter().any(|(m, on)| *on && m.id == *source),
            "来源插件已停用"
        );
        let id = playlist_id(source, &ui.get_import_id())?;
        let target = match ui.get_import_target_index() {
            0 => None,
            n => Some(
                &self
                    .import
                    .targets
                    .get((n - 1) as usize)
                    .context("请选择目标歌单")?
                    .key
                    .id,
            ),
        };
        ui.set_import_busy(true);
        ui.set_import_status("正在读取完整歌单…".into());
        request(
            jobs,
            self.import.ticket,
            source,
            "import-playlist",
            json!({"id":id,"name":ui.get_import_name().as_str(),"target":target}),
        );
        Ok(())
    }

    fn close_import(&mut self, ui: &AppWindow, jobs: &mpsc::Sender<Job>, generation: &AtomicU64) {
        ui.set_import_step(ImportStep::Closed);
        let completed = std::mem::take(&mut self.import);
        self.import.ticket = completed.ticket.wrapping_add(1);
        if !completed.destination.is_empty() {
            self.open_source(
                ui,
                jobs,
                generation,
                "local",
                Some(navigation::Location::local_playlist(completed.destination)),
            );
            send(jobs, generation, "local", "local-playlists", json!({}));
            ui.invoke_notify_row(
                format!(
                    "歌单已保存 · 新增 {} 首，手动替换 {} 首",
                    completed.added, completed.replaced
                )
                .into(),
            );
        }
    }

    fn start_matching(&mut self, ui: &AppWindow, jobs: &mpsc::Sender<Job>, generation: &AtomicU64) {
        let Some(original) = self.import.missing.front() else {
            self.close_import(ui, jobs, generation);
            return;
        };
        let name = if original.title.is_empty() {
            &original.key.id
        } else {
            &original.title
        };
        ui.set_import_original(
            format!("《{name}》 · 待处理 {} 首", self.import.missing.len()).into(),
        );
        ui.set_import_query(
            format!("{} {}", original.title, original.artist)
                .trim()
                .into(),
        );
        ui.set_import_step(ImportStep::Matching);
        self.search_replacements(ui, jobs);
    }

    fn search_replacements(&mut self, ui: &AppWindow, jobs: &mpsc::Sender<Job>) {
        let Some(original) = self.import.missing.front() else {
            return;
        };
        self.import.ticket = self.import.ticket.wrapping_add(1);
        self.import.query = ui.get_import_query().trim().to_owned();
        ui.set_import_candidates(ModelRc::default());
        ui.set_import_more(false);
        self.import.search = search::Search::new(&self.plugins, "local", "track");
        if let Some(search) = &mut self.import.search {
            search.sources.retain(|s| s.id != original.key.source);
        }
        if self
            .import
            .search
            .as_ref()
            .is_none_or(|s| s.sources.is_empty())
        {
            ui.set_import_status("没有其他已启用的音乐源，可保留原曲并继续".into());
        } else if self.import.query.is_empty() {
            ui.set_import_status("请输入要查找的曲名或歌手".into());
        } else {
            self.more_replacements(ui, jobs);
        }
    }

    fn more_replacements(&mut self, ui: &AppWindow, jobs: &mpsc::Sender<Job>) {
        let Some(search) = &mut self.import.search else {
            return;
        };
        ui.set_import_more(false);
        ui.set_import_status("正在其他已启用的音乐源中查找…".into());
        for (source, offset) in search.requests(false) {
            request(
                jobs,
                self.import.ticket,
                &source,
                "import-search",
                json!({"kind":"track","query":self.import.query,"offset":offset}),
            );
        }
    }

    fn show_candidates(&self, ui: &AppWindow) {
        let Some(search) = &self.import.search else {
            return;
        };
        let rows = search
            .items
            .iter()
            .map(|track| {
                let source = self
                    .plugins
                    .iter()
                    .find(|(m, _)| m.id == track.key.source)
                    .map_or(track.key.source.as_str(), |(m, _)| m.name.as_str());
                ImportCandidate {
                    title: track.title.clone().into(),
                    detail: [source, &track.artist, &track.album]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ")
                        .into(),
                }
            })
            .collect::<Vec<_>>();
        let mut status = if rows.is_empty() && search.pending(false) == 0 {
            "没有找到可用版本，可修改关键词或跳过".to_owned()
        } else {
            format!("{} 个可选版本", rows.len())
        };
        if search.pending(false) != 0 {
            status.push_str(" · 搜索中");
        }
        for error in search.errors(false) {
            status.push_str(&format!(" · {error}"));
        }
        ui.set_import_status(status.into());
        ui.set_import_candidates(ModelRc::new(VecModel::from(rows)));
        ui.set_import_more(search.more(false) && search.pending(false) == 0);
    }

    pub(super) fn finish_import_reply(
        &mut self,
        ui: &AppWindow,
        jobs: &mpsc::Sender<Job>,
        generation: &AtomicU64,
        reply: Reply,
    ) {
        if self.import.ticket != reply.generation || ui.get_import_step() == ImportStep::Closed {
            return;
        }
        if reply.op == "import-search" {
            if let Some(search) = &mut self.import.search {
                let result = reply
                    .value
                    .and_then(|v| serde_json::from_value::<Page>(v).map_err(|e| e.to_string()))
                    .map(|mut page| {
                        page.items.retain(|i| {
                            i.kind == "track" && !i.unavailable && !i.key.id.is_empty()
                        });
                        for item in &mut page.items {
                            item.key.source = reply.source.clone();
                        }
                        page
                    });
                search.accept(
                    &reply.source,
                    reply.args["offset"].as_u64().unwrap_or_default(),
                    result,
                );
                if !ui.get_import_busy() {
                    self.show_candidates(ui);
                }
            }
            return;
        }
        ui.set_import_busy(false);
        let value = match reply.value {
            Ok(v) => v,
            Err(e) => {
                ui.set_import_status(e.into());
                return;
            }
        };
        match reply.op.as_str() {
            "import-choices" => {
                if self
                    .import
                    .sources
                    .get(ui.get_import_source_index() as usize)
                    != Some(&reply.source)
                {
                    return;
                }
                let page = match serde_json::from_value::<Page>(value) {
                    Ok(page) => page,
                    Err(error) => {
                        ui.set_import_status(error.to_string().into());
                        return;
                    }
                };
                self.import.choices_offset += page.items.len() as u64;
                ui.set_import_can_browse(page.more && !page.items.is_empty());
                self.import.choices.extend(
                    page.items
                        .into_iter()
                        .filter(|i| i.kind == "playlist" && !i.key.id.is_empty()),
                );
                if self.import.choices.is_empty() {
                    ui.set_import_status("当前账号没有可选歌单，也可以填写公开歌单 ID".into());
                } else {
                    ui.set_import_choices(labels(
                        std::iter::once("选择来源歌单…".into())
                            .chain(self.import.choices.iter().map(|p| p.title.clone())),
                    ));
                    ui.set_import_status("".into());
                }
            }
            "import-targets" => {
                self.import.targets = serde_json::from_value(value).unwrap_or_default();
                ui.set_import_targets(labels(
                    std::iter::once("创建新歌单".into())
                        .chain(self.import.targets.iter().map(|p| p.title.clone())),
                ));
                if ui.get_local_playlist()
                    && let Some(i) = self
                        .import
                        .targets
                        .iter()
                        .position(|p| self.last.args["id"] == p.key.id)
                {
                    ui.set_import_target_index((i + 1) as i32);
                }
                ui.set_import_status(
                    if self.import.sources.is_empty() {
                        "请在设置中安装并启用支持歌单的插件"
                    } else {
                        ""
                    }
                    .into(),
                );
            }
            "import-playlist" => {
                self.import.destination = value["id"].as_str().unwrap_or_default().into();
                self.import.added = value["added"].as_u64().unwrap_or_default() as usize;
                self.import.missing =
                    serde_json::from_value(value["missing"].clone()).unwrap_or_default();
                if self.import.missing.is_empty() {
                    self.close_import(ui, jobs, generation);
                } else {
                    ui.set_import_status(format!("有 {} 首歌曲在原来源已下架或不可用。是否查询其他来源，手动选择要加入的版本？", self.import.missing.len()).into());
                    ui.set_import_step(ImportStep::Confirm);
                }
            }
            "import-replace" => {
                self.import.replaced += 1;
                self.import.missing.pop_front();
                self.start_matching(ui, jobs, generation);
            }
            _ => {}
        }
    }
}

pub(super) fn install(
    ui: &AppWindow,
    state: Rc<RefCell<UiState>>,
    jobs: mpsc::Sender<Job>,
    generation: Arc<AtomicU64>,
) {
    let weak = ui.as_weak();
    ui.on_import_action(move |action, index| {
        let Some(ui) = weak.upgrade() else { return };
        let mut s = state.borrow_mut();
        if ui.get_import_busy() {
            return;
        }
        match (ui.get_import_step(), action.as_str()) {
            (ImportStep::Closed, "open") => s.open_import(&ui, &jobs),
            (_, "close") => s.close_import(&ui, &jobs, &generation),
            (ImportStep::Form, "source") => {
                ui.set_import_id("".into());
                ui.set_import_status("".into());
                s.reset_choices(&ui);
            }
            (ImportStep::Form, "browse") if ui.get_import_can_browse() => {
                if let Some(source) = s.import.sources.get(ui.get_import_source_index() as usize) {
                    ui.set_import_busy(true);
                    ui.set_import_status("正在读取插件中的歌单…".into());
                    request(
                        &jobs,
                        s.import.ticket,
                        source,
                        "import-choices",
                        json!({"offset":s.import.choices_offset}),
                    );
                }
            }
            (ImportStep::Form, "pick") if index > 0 => {
                if let Some(p) = s.import.choices.get((index - 1) as usize) {
                    ui.set_import_id(p.key.id.as_str().into());
                }
            }
            (ImportStep::Form, "submit") => {
                if let Err(e) = s.start_import(&ui, &jobs) {
                    ui.set_import_status(e.to_string().into());
                }
            }
            (ImportStep::Confirm, "match") => s.start_matching(&ui, &jobs, &generation),
            (ImportStep::Matching, "search") => s.search_replacements(&ui, &jobs),
            (ImportStep::Matching, "more") => s.more_replacements(&ui, &jobs),
            (ImportStep::Matching, "skip") => {
                s.import.missing.pop_front();
                s.start_matching(&ui, &jobs, &generation);
            }
            (ImportStep::Matching, "choose") => {
                let Some(track) = s
                    .import
                    .search
                    .as_ref()
                    .and_then(|s| s.items.get(index as usize))
                else {
                    return;
                };
                let Some(original) = s.import.missing.front() else {
                    return;
                };
                if track.key.source == original.key.source || !playable(track, &s.plugins) {
                    return;
                }
                ui.set_import_busy(true);
                ui.set_import_status("正在保存选定的版本…".into());
                request(
                    &jobs,
                    s.import.ticket,
                    "local",
                    "import-replace",
                    json!({"id":s.import.destination,"original":original.key,"track":track}),
                );
            }
            _ => {}
        }
    });
}
