use super::*;
use rand::seq::SliceRandom;

pub(super) struct Heartbeat {
    playlist: String,
    favorites: Queue,
    pending: Option<u64>,
    waiting_for_next: bool,
    failed_tracks: u8,
}

pub(super) fn playlist_id<'a>(
    source: &str,
    request: &Request,
    marker: &'a Value,
) -> Option<&'a str> {
    let id = marker.as_str().filter(|id| !id.is_empty())?;
    (source == "netease"
        && (request.op == "liked"
            || (request.op == "playlist" && request.args["id"].as_str() == Some(id))))
    .then_some(id)
}

impl UiState {
    pub(super) fn saved_queue(&self) -> &Queue {
        self.heartbeat
            .as_ref()
            .map_or(&self.queue, |mix| &mix.favorites)
    }

    pub(super) fn on_heartbeat_page(&self, ui: &AppWindow) -> bool {
        self.heartbeat
            .as_ref()
            .is_some_and(|mix| mix.playlist == ui.get_heartbeat_playlist_id().as_str())
    }

    pub(super) fn change_playback_mode(
        &mut self,
        ui: &AppWindow,
        jobs: &mpsc::Sender<Job>,
        player: &mpsc::Sender<Command>,
        shuffle_shortcut: bool,
    ) {
        self.resume_heartbeat = None;
        let was_heartbeat = self.stop_heartbeat(ui);
        if shuffle_shortcut {
            if !was_heartbeat || self.queue.playback_mode() != PlaybackMode::Shuffle {
                self.queue.toggle_shuffle();
            }
        } else if !was_heartbeat
            && self.queue.playback_mode() == PlaybackMode::Shuffle
            && self.start_heartbeat(ui, jobs, player, None)
        {
            ui.invoke_notify_row("心动模式 · 喜欢的歌曲中穿插网易云推荐".into());
            return;
        } else {
            self.queue.cycle_playback_mode();
        }
        ui.set_playback_mode(self.queue.playback_mode().into());
        ui.invoke_notify_row(ui.get_playback_mode_name());
    }

    pub(super) fn start_heartbeat(
        &mut self,
        ui: &AppWindow,
        jobs: &mpsc::Sender<Job>,
        player: &mpsc::Sender<Command>,
        selected: Option<&ResourceKey>,
    ) -> bool {
        let playlist = ui.get_heartbeat_playlist_id().to_string();
        if playlist.is_empty()
            || self.page_source != "netease"
            || !has_capability(&self.plugins, "netease", "heartbeat")
        {
            return false;
        }
        let favorites: Vec<_> = self
            .items
            .iter()
            .filter(|t| t.key.source == "netease" && t.kind == "track" && !t.unavailable)
            .cloned()
            .collect();
        let seed = selected
            .or(self.loaded.as_ref())
            .and_then(|key| favorites.iter().find(|t| t.key == *key))
            .or_else(|| {
                let current = self.queue.items.get(self.queue.index?)?;
                favorites.iter().find(|t| t.key == current.key)
            })
            .or_else(|| favorites.choose(&mut rand::thread_rng()))
            .cloned();
        let Some(seed) = seed else { return false };
        let mut normal = self
            .heartbeat
            .take()
            .map(|mix| mix.favorites)
            .unwrap_or_else(|| std::mem::take(&mut self.queue));
        normal.index = favorites.iter().position(|t| t.key == seed.key);
        normal.items = favorites;
        normal.album_origin = None;
        self.heartbeat = Some(Heartbeat {
            playlist,
            favorites: normal,
            pending: None,
            waiting_for_next: false,
            failed_tracks: 0,
        });
        self.queue = Queue::default();
        self.queue.items.push(seed.clone());
        let seed_loaded = self.loaded.as_ref() == Some(&seed.key);
        self.queue.index = (seed_loaded || selected.is_some()).then_some(0);
        ui.set_playback_mode(PlayerMode::Heartbeat);
        refresh_queue(ui, self);
        if seed_loaded {
            if selected.is_some() && !self.playing {
                let _ = player.send(Command::Pause(false));
                self.playing = true;
                ui.set_playing(true);
            }
            self.request_heartbeat(jobs);
        } else if selected.is_some() {
            play(ui, self, jobs, player, seed, false);
        }
        true
    }

    pub(super) fn stop_heartbeat(&mut self, ui: &AppWindow) -> bool {
        let Some(mix) = self.heartbeat.take() else {
            return false;
        };
        self.queue = mix.favorites;
        ui.set_playback_mode(self.queue.playback_mode().into());
        refresh_queue(ui, self);
        true
    }

    pub(super) fn heartbeat_track(&mut self, item: &Item) {
        if let Some(mix) = self.heartbeat.as_mut() {
            mix.waiting_for_next = false;
            if let Some(index) = mix.favorites.items.iter().position(|t| t.key == item.key) {
                mix.favorites.index = Some(index);
            }
        }
    }

    pub(super) fn advance_track(
        &mut self,
        natural: bool,
        jobs: &mpsc::Sender<Job>,
    ) -> Option<Item> {
        let next = self.advance_available(natural);
        if next.is_none()
            && let Some(mix) = self.heartbeat.as_mut()
        {
            mix.waiting_for_next = true;
            self.request_heartbeat(jobs);
        }
        next
    }

    pub(super) fn request_heartbeat(&mut self, jobs: &mpsc::Sender<Job>) {
        let (Some(mix), Some(index)) = (self.heartbeat.as_mut(), self.queue.index) else {
            return;
        };
        let Some(current) = self.queue.items.get(index) else {
            return;
        };
        if mix.pending.is_some() || self.queue.items.len().saturating_sub(index + 1) > 2 {
            return;
        }
        let recent: Vec<_> = self
            .queue
            .items
            .iter()
            .rev()
            .take(40)
            .rev()
            .map(|t| &t.key.id)
            .collect();
        self.heartbeat_generation += 1;
        let generation = self.heartbeat_generation;
        mix.pending = Some(generation);
        let _ = jobs.send(Job {
            source: "netease".into(),
            request: Request::new(
                "heartbeat",
                json!({"playlistId":mix.playlist,"currentId":current.key.id,"recent":recent}),
            ),
            generation,
        });
    }

    pub(super) fn finish_heartbeat(
        &mut self,
        ui: &AppWindow,
        reply: Reply,
        jobs: &mpsc::Sender<Job>,
        player: &mpsc::Sender<Command>,
        covers: &mpsc::SyncSender<covers::Cover>,
        page_generation: u64,
    ) {
        let Some(mix) = self.heartbeat.as_mut() else {
            return;
        };
        if reply.source != "netease" || mix.pending != Some(reply.generation) {
            return;
        }
        mix.pending = None;
        let waiting = std::mem::take(&mut mix.waiting_for_next);
        let batch = reply
            .value
            .and_then(|v| serde_json::from_value::<Page>(v).map_err(|e| e.to_string()));
        let failure = match batch {
            Ok(mut page) => {
                let upcoming = self.queue.index.map_or(0, |i| i + 1);
                let mut queued: HashSet<_> = self
                    .queue
                    .items
                    .iter()
                    .skip(upcoming)
                    .map(|t| t.key.id.clone())
                    .collect();
                page.items.retain(|t| {
                    t.kind == "track"
                        && !t.unavailable
                        && !t.key.id.is_empty()
                        && queued.insert(t.key.id.clone())
                });
                if page.items.is_empty() {
                    Some("暂时没有新的可播曲目".to_owned())
                } else {
                    prepare_items(
                        &mut page.items,
                        &self.plugins,
                        "netease",
                        covers,
                        page_generation,
                    );
                    self.queue.items.extend(page.items);
                    refresh_queue(ui, self);
                    None
                }
            }
            Err(error) => Some(error),
        };
        if let Some(reason) = failure {
            self.stop_heartbeat(ui);
            ui.invoke_notify_row(format!("心动模式已返回喜欢的歌曲 · {reason}").into());
        }
        if waiting && let Some(next) = self.queue.advance(false, rand::random()) {
            play(ui, self, jobs, player, next, false);
        }
    }

    pub(super) fn skip_failed_heartbeat_track(
        &mut self,
        ui: &AppWindow,
        jobs: &mpsc::Sender<Job>,
        player: &mpsc::Sender<Command>,
    ) -> bool {
        let Some(mix) = self.heartbeat.as_mut() else {
            return false;
        };
        mix.failed_tracks += 1;
        if mix.failed_tracks >= 3 {
            self.stop_heartbeat(ui);
            return false;
        }
        ui.invoke_notify_row("心动模式已跳过无法播放的歌曲".into());
        if let Some(next) = self.advance_track(false, jobs) {
            play(ui, self, jobs, player, next, false);
        }
        true
    }

    pub(super) fn heartbeat_resolved(&mut self) {
        if let Some(mix) = self.heartbeat.as_mut() {
            mix.failed_tracks = 0;
        }
    }

    pub(super) fn cancel_heartbeat_advance(&mut self) {
        if let Some(mix) = self.heartbeat.as_mut() {
            mix.waiting_for_next = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_requires_the_current_users_netease_favorite_playlist() {
        let marker = json!("10");
        for (source, op, id, expected) in [
            ("netease", "liked", "", true),
            ("netease", "playlist", "10", true),
            ("netease", "playlist", "12", false),
            ("netease", "search", "", false),
            ("local", "liked", "", false),
            ("bilibili", "liked", "", false),
        ] {
            assert_eq!(
                playlist_id(source, &Request::new(op, json!({"id":id})), &marker).is_some(),
                expected
            );
        }
        assert!(playlist_id("netease", &Request::new("liked", json!({})), &Value::Null).is_none());
    }

    #[cfg(feature = "ui-smoke")]
    #[test]
    fn mix_refills_without_restarting_the_seed_and_ignores_late_replies() -> Result<()> {
        let _clock = crate::test_ui::test_clock()?;
        let ui = AppWindow::new()?;
        ui.show()?;
        let plugin = serde_json::from_value(
            json!({"id":"netease","abi":2,"name":"网易云","version":"1.3.0","capabilities":["heartbeat"],"network":[],"media":[]}),
        )?;
        let mut state = UiState::new(Queue::default(), Default::default(), vec![(plugin, true)]);
        let track = |id: &str| Item {
            key: ResourceKey {
                source: "netease".into(),
                id: id.into(),
            },
            kind: "track".into(),
            title: id.into(),
            ..Default::default()
        };
        state.page_source = "netease".into();
        state.items = vec![track("1"), track("2")];
        state.loaded = Some(track("1").key);
        state.playing = true;
        state.queue.toggle_shuffle();
        state.update_rows(&ui);
        ui.window().set_size(slint::PhysicalSize::new(720, 480));
        let clicked = Rc::new(Cell::new(false));
        let observed = clicked.clone();
        ui.on_action(move |action| observed.set(action == "cycle-mode"));
        ui.window().take_snapshot()?;
        crate::test_ui::click(ui.window(), 251., 415.);
        assert!(
            clicked.get(),
            "the compact player bar must dispatch the mode action"
        );
        let (jobs, rx) = mpsc::channel();
        let (player, commands) = mpsc::channel();
        let (covers, _covers) = mpsc::sync_channel(5);
        state.change_playback_mode(&ui, &jobs, &player, false);
        assert_eq!(ui.get_playback_mode(), PlayerMode::Sequential);
        ui.set_heartbeat_playlist_id("10".into());
        for mode in [
            PlayerMode::RepeatAll,
            PlayerMode::RepeatOne,
            PlayerMode::Shuffle,
            PlayerMode::Heartbeat,
        ] {
            state.change_playback_mode(&ui, &jobs, &player, false);
            assert_eq!(ui.get_playback_mode(), mode);
        }
        assert!(ui.get_heartbeat_active() && commands.try_recv().is_err());
        let request = rx.try_recv()?;
        assert_eq!(request.request.args["currentId"], "1");
        assert_eq!(request.request.args["playlistId"], "10");
        assert_eq!(state.queue.playback_mode(), PlaybackMode::Sequential);
        assert_eq!(state.saved_queue().playback_mode(), PlaybackMode::Shuffle);
        assert!(state.advance_track(true, &jobs).is_none());
        assert!(state.advance_track(false, &jobs).is_none());
        assert!(rx.try_recv().is_err());
        let result = |generation, value| Reply {
            source: "netease".into(),
            op: "heartbeat".into(),
            args: Value::Null,
            generation,
            value,
        };
        let batch = json!({"items":[{"kind":"track","key":{"source":"netease","id":"9"},"heartbeat_recommendation":true},
            {"kind":"track","key":{"source":"netease","id":"2"}},{"kind":"track","key":{"source":"netease","id":"2"}}]});
        state.finish_heartbeat(
            &ui,
            result(request.generation, Ok(batch.clone())),
            &jobs,
            &player,
            &covers,
            1,
        );
        assert_eq!(state.queue.index, Some(1));
        assert_eq!(state.queue.items.len(), 3);
        assert!(ui.get_queue().row_data(1).unwrap().recommended);
        assert_eq!(rx.try_recv()?.request.op, "resolve");
        let refill = rx.try_recv()?;
        assert_eq!(refill.request.op, "heartbeat");
        state.change_playback_mode(&ui, &jobs, &player, false);
        assert_eq!(ui.get_playback_mode(), PlayerMode::Sequential);
        assert_eq!(state.queue.playback_mode(), PlaybackMode::Sequential);
        state.queue.toggle_shuffle();
        state.start_heartbeat(&ui, &jobs, &player, Some(&track("1").key));
        let next = rx.try_iter().find(|j| j.request.op == "heartbeat").unwrap();
        state.finish_heartbeat(
            &ui,
            result(refill.generation, Ok(batch)),
            &jobs,
            &player,
            &covers,
            2,
        );
        assert_eq!(state.queue.items.len(), 1);
        state.finish_heartbeat(
            &ui,
            result(next.generation, Err("网络不可用".into())),
            &jobs,
            &player,
            &covers,
            2,
        );
        assert!(!ui.get_heartbeat_active());
        assert_eq!(state.queue.playback_mode(), PlaybackMode::Shuffle);
        assert_eq!(state.queue.items.len(), 2);
        assert!(
            state
                .queue
                .items
                .iter()
                .all(|t| !t.heartbeat_recommendation)
        );
        // Selecting a mode must not unpause a song or start a new one.
        let _ = commands.try_iter().count();
        state.playing = false;
        for loaded in [Some(track("1").key), None] {
            state.loaded = loaded;
            state.change_playback_mode(&ui, &jobs, &player, false);
            assert_eq!(ui.get_playback_mode(), PlayerMode::Heartbeat);
            assert!(!state.playing && commands.try_recv().is_err());
            if state.loaded.is_none() {
                assert!(state.advance_track(false, &jobs).is_some());
            }
            state.change_playback_mode(&ui, &jobs, &player, true);
            assert_eq!(ui.get_playback_mode(), PlayerMode::Shuffle);
        }
        Ok(())
    }
}
