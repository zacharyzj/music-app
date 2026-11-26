use super::*;

// The player's lookup and all edits follow a track even after page navigation.
pub(super) const TRACK_SCOPE: u64 = 0;

fn send_favorite(jobs: &mpsc::Sender<Job>, source: &str, op: &str, args: Value) {
    let _ = jobs.send(Job {
        source: source.to_owned(),
        request: Request::new(op, args),
        generation: TRACK_SCOPE,
    });
}

impl UiState {
    fn supports_favorites(&self, source: &str) -> bool {
        source == "local"
            || self
                .plugins
                .iter()
                .any(|(m, enabled)| *enabled && m.id == source && m.supports_likes())
    }

    pub(super) fn is_liked(&self, key: &ResourceKey) -> bool {
        self.likes.get(key) == Some(&true)
    }

    pub(super) fn request_likes(
        &self,
        jobs: &mpsc::Sender<Job>,
        generation: &AtomicU64,
        source: &str,
        items: &[Item],
    ) {
        if !self.supports_favorites(source) {
            return;
        }
        let ids: Vec<_> = items
            .iter()
            .filter(|item| item.kind == "track" && item.key.source == source)
            .map(|item| &item.key.id)
            .collect();
        for batch in ids.chunks(200) {
            send(jobs, generation, source, "isLiked", json!({"ids":batch}));
        }
    }

    pub(super) fn current_favorite(&self) -> FavoriteState {
        let Some(key) = &self.now_playing else {
            return FavoriteState::NoTrack;
        };
        if !self.supports_favorites(&key.source) {
            return FavoriteState::Unsupported;
        }
        if self.pending_likes.contains(key) {
            return if self.is_liked(key) {
                FavoriteState::Removing
            } else {
                FavoriteState::Adding
            };
        }
        match self.likes.get(key) {
            Some(true) => FavoriteState::Liked,
            Some(false) => FavoriteState::Unliked,
            None if self.checking_current_like => FavoriteState::Checking,
            None => FavoriteState::Retry,
        }
    }

    pub(super) fn request_current_like(&mut self, ui: &AppWindow, jobs: &mpsc::Sender<Job>) {
        if self.current_favorite() == FavoriteState::Retry
            && let Some(key) = &self.now_playing
        {
            self.checking_current_like = true;
            send_favorite(jobs, &key.source, "isLiked", json!({"ids":[key.id]}));
        }
        ui.set_current_favorite(self.current_favorite());
    }

    pub(super) fn toggle_current_favorite(&mut self, ui: &AppWindow, jobs: &mpsc::Sender<Job>) {
        match self.current_favorite() {
            state @ (FavoriteState::Liked | FavoriteState::Unliked) => {
                if let Some(key) = self.now_playing.clone() {
                    self.set_track_liked(ui, jobs, &key, state == FavoriteState::Unliked);
                }
            }
            FavoriteState::Retry => self.request_current_like(ui, jobs),
            _ => {}
        }
    }

    pub(super) fn set_track_liked(
        &mut self,
        ui: &AppWindow,
        jobs: &mpsc::Sender<Job>,
        key: &ResourceKey,
        on: bool,
    ) {
        if self.supports_favorites(&key.source) && self.pending_likes.insert(key.clone()) {
            self.refresh_hearts(ui);
            send_favorite(jobs, &key.source, "like", json!({"id":key.id,"on":on}));
        }
    }

    pub(super) fn finish_like_query(&mut self, ui: &AppWindow, reply: Reply) {
        let states = reply.value.ok();
        for id in reply.args["ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let key = ResourceKey {
                source: reply.source.clone(),
                id: id.to_owned(),
            };
            if self.now_playing.as_ref() == Some(&key) {
                self.checking_current_like = false;
            }
            if !self.pending_likes.contains(&key)
                && let Some(on) = states.as_ref().and_then(|v| v[id].as_bool())
            {
                self.likes.insert(key, on);
            }
        }
        self.refresh_hearts(ui);
    }

    pub(super) fn finish_like(
        &mut self,
        ui: &AppWindow,
        reply: Reply,
        jobs: &mpsc::Sender<Job>,
        generation: &AtomicU64,
    ) {
        let key = ResourceKey {
            source: reply.source,
            id: reply.args["id"].as_str().unwrap_or_default().to_owned(),
        };
        if !self.pending_likes.remove(&key) {
            return;
        }
        let title = self
            .items
            .iter()
            .find(|item| item.key == key)
            .map(|t| t.title.clone())
            .or_else(|| {
                (self.now_playing.as_ref() == Some(&key)).then(|| ui.get_now_title().to_string())
            })
            .map_or_else(|| "这首歌".into(), |name| format!("《{name}》"));
        let message = match reply.value {
            Ok(_) => {
                let on = reply.args["on"] == true;
                self.likes.insert(key.clone(), on);
                let favorites_page = self.page_source == key.source
                    && (self.last.op == "liked"
                        || (self.last.op == "playlist"
                            && !ui.get_heartbeat_playlist_id().is_empty()
                            && self.last.args["id"] == ui.get_heartbeat_playlist_id().as_str()));
                if favorites_page {
                    if !on {
                        self.remove_visible_track(ui, &key);
                    } else if !self.items.iter().any(|t| t.key == key) {
                        let mut args = self.last.args.clone();
                        args["offset"] = json!(0);
                        send(jobs, generation, &self.page_source, &self.last.op, args);
                    }
                }
                if on {
                    format!("已喜欢{title}")
                } else {
                    format!("已取消喜欢{title}")
                }
            }
            Err(error) => format!("{title}操作失败 · {error}"),
        };
        self.refresh_hearts(ui);
        ui.invoke_notify_row(message.into());
    }

    pub(super) fn refresh_hearts(&self, ui: &AppWindow) {
        ui.set_current_favorite(self.current_favorite());
        let rows = ui.get_rows();
        for (i, item) in self.items.iter().enumerate() {
            if item.kind != "track" {
                continue;
            }
            let liked = self.is_liked(&item.key);
            let pending = self.pending_likes.contains(&item.key);
            if let Some(mut row) = rows.row_data(i)
                && (row.liked != liked || row.like_pending != pending)
            {
                row.liked = liked;
                row.like_pending = pending;
                rows.set_row_data(i, row);
            }
        }
    }

    pub(super) fn forget_source_likes(&mut self, source: &str) {
        self.likes.retain(|key, _| key.source != source);
        if self
            .now_playing
            .as_ref()
            .is_some_and(|key| key.source == source)
        {
            self.checking_current_like = false;
        }
    }

    pub(super) fn clear_now_playing(&mut self, ui: &AppWindow) {
        self.now_playing = None;
        self.checking_current_like = false;
        ui.set_current_favorite(FavoriteState::NoTrack);
        ui.set_now_title("还没有正在播放的歌曲".into());
        ui.set_now_artist("".into());
        ui.set_now_cover(slint::Image::default());
    }
}

#[cfg(all(test, feature = "ui-smoke"))]
mod tests {
    use super::*;

    fn reply(job: Job, value: std::result::Result<Value, String>) -> Reply {
        Reply {
            source: job.source,
            op: job.request.op,
            args: job.request.args,
            generation: job.generation,
            value,
        }
    }

    #[test]
    fn bar_and_track_row_update_the_same_library_entry_across_navigation() -> Result<()> {
        let _clock = crate::test_ui::test_clock()?;
        let ui = AppWindow::new()?;
        let dir = tempfile::tempdir()?;
        let mut db = Library::open(&dir.path().join("data"))?;
        std::fs::write(dir.path().join("收藏测试.mp3"), b"fixture")?;
        db.add_folder(dir.path())?;
        db.scan(&AtomicBool::new(false))?;
        let track = db.tracks()?.remove(0);
        let mut s = UiState::new(Queue::default(), Default::default(), Vec::new());
        let (jobs, rx) = mpsc::channel();
        let (player, _commands) = mpsc::channel();
        let generation = AtomicU64::new(3);
        play(&ui, &mut s, &jobs, &player, track.clone(), false);
        assert_eq!(ui.get_current_favorite(), FavoriteState::Checking);
        assert_eq!(rx.try_recv()?.request.op, "resolve");
        let read = rx.try_recv()?;
        assert_eq!(read.request.op, "isLiked");
        let states = db.call(read.request.clone())?;
        s.finish_like_query(&ui, reply(read, Ok(states)));
        assert_eq!(ui.get_current_favorite(), FavoriteState::Unliked);

        s.toggle_current_favorite(&ui, &jobs);
        s.toggle_current_favorite(&ui, &jobs);
        assert_eq!(ui.get_current_favorite(), FavoriteState::Adding);
        let write = rx.try_recv()?;
        assert_eq!(write.request.args, json!({"id":track.key.id,"on":true}));
        assert!(rx.try_recv().is_err());
        s.clear_page(&ui);
        s.last = Request::new("liked", json!({}));
        let accepted = db.call(write.request.clone())?;
        s.finish_like(&ui, reply(write, Ok(accepted)), &jobs, &generation);
        assert_eq!(ui.get_current_favorite(), FavoriteState::Liked);
        let reload = rx.try_recv()?;
        assert_eq!(reload.request.op, "liked");
        let page: Page = serde_json::from_value(db.call(reload.request)?)?;
        assert_eq!(page.items[0].key, track.key);
        s.items = page.items;
        s.update_rows(&ui);
        assert!(ui.get_rows().row_data(0).unwrap().liked);

        s.set_track_liked(&ui, &jobs, &track.key, false);
        assert_eq!(ui.get_current_favorite(), FavoriteState::Removing);
        let failed = rx.try_recv()?;
        s.finish_like(
            &ui,
            reply(failed, Err("暂时离线".into())),
            &jobs,
            &generation,
        );
        assert_eq!(ui.get_current_favorite(), FavoriteState::Liked);
        assert!(ui.get_rows().row_data(0).unwrap().liked);
        s.toggle_current_favorite(&ui, &jobs);
        let remove = rx.try_recv()?;
        let accepted = db.call(remove.request.clone())?;
        s.finish_like(&ui, reply(remove, Ok(accepted)), &jobs, &generation);
        assert_eq!(ui.get_current_favorite(), FavoriteState::Unliked);
        assert_eq!(ui.get_rows().row_count(), 0);
        assert_eq!(s.now_playing.as_ref(), Some(&track.key));
        assert_eq!(db.call(s.last.clone())?["items"], json!([]));
        Ok(())
    }

    #[test]
    fn a_late_lookup_never_targets_the_next_song_or_a_different_source() -> Result<()> {
        let _clock = crate::test_ui::test_clock()?;
        let ui = AppWindow::new()?;
        let plugin = serde_json::from_value(
            json!({"id":"netease","name":"网易云","version":"1.3.0","abi":2,"capabilities":[],"network":[],"media":[]}),
        )?;
        let mut s = UiState::new(Queue::default(), Default::default(), vec![(plugin, true)]);
        let (jobs, rx) = mpsc::channel();
        let (player, _commands) = mpsc::channel();
        for source in ["local", "netease"] {
            let item = Item {
                key: ResourceKey {
                    source: source.into(),
                    id: "42".into(),
                },
                kind: "track".into(),
                ..Default::default()
            };
            play(&ui, &mut s, &jobs, &player, item, false);
        }
        let mut reads = rx.try_iter().filter(|j| j.request.op == "isLiked");
        let first = reads.next().unwrap();
        assert_eq!(first.source, "local");
        s.finish_like_query(&ui, reply(first, Ok(json!({"42":true}))));
        assert_eq!(ui.get_current_favorite(), FavoriteState::Checking);
        s.finish_like_query(&ui, reply(reads.next().unwrap(), Err("离线".into())));
        assert_eq!(ui.get_current_favorite(), FavoriteState::Retry);
        s.toggle_current_favorite(&ui, &jobs);
        let retry = rx.try_recv()?;
        assert_eq!(
            (retry.source.as_str(), retry.request.op.as_str()),
            ("netease", "isLiked")
        );
        s.finish_like_query(&ui, reply(retry, Ok(json!({"42":false}))));
        assert_eq!(ui.get_current_favorite(), FavoriteState::Unliked);
        s.toggle_current_favorite(&ui, &jobs);
        let write = rx.try_recv()?;
        assert_eq!(
            (write.source.as_str(), write.request.op.as_str()),
            ("netease", "like")
        );
        s.plugins[0].0.capabilities.push("readOnly".into());
        s.request_current_like(&ui, &jobs);
        s.toggle_current_favorite(&ui, &jobs);
        assert_eq!(ui.get_current_favorite(), FavoriteState::Unsupported);
        assert!(rx.try_recv().is_err());
        Ok(())
    }
}
