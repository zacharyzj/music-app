use super::*;
use music_player::{PlaybackClock, PlaybackStart};
use serde::{Deserialize, Serialize};

pub(super) const KEY: &str = "playback-resume";

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ResumePoint {
    pub item: Item,
    #[serde(flatten)]
    pub start: PlaybackStart,
}

impl ResumePoint {
    pub fn read(db: &Library) -> Option<Self> {
        let point: Self = serde_json::from_value(db.get(KEY)).ok()?;
        let item = &point.item;
        if item.kind != "track"
            || item.key.id.is_empty()
            || item.key.source.is_empty()
            || !point.start.position.is_finite()
            || point.start.position < 0.
            || (item.key.source == "local" && !db.path(&item.key.id).ok()?.is_file())
        {
            return None;
        }
        Some(point)
    }

    pub fn restore(
        self,
        ui: &AppWindow,
        state: &mut UiState,
        jobs: &mpsc::Sender<Job>,
        player: &mpsc::Sender<Command>,
    ) {
        if !playable(&self.item, &state.plugins) {
            return;
        }
        let queue = &mut state.queue;
        if queue
            .index
            .and_then(|i| queue.items.get(i))
            .is_none_or(|item| item.key != self.item.key)
            && let Some(i) = queue
                .items
                .iter()
                .position(|item| item.key == self.item.key)
        {
            queue.index = Some(i);
        }
        play(
            ui,
            state,
            jobs,
            player,
            self.item,
            PlayKind::Resume(self.start),
        );
    }
}

impl UiState {
    pub(super) fn playback_start(&self) -> PlaybackStart {
        if self.previewing {
            return PlaybackStart::default();
        }
        self.resume.as_ref().map(|p| p.start).unwrap_or_default()
    }

    pub(super) fn record_clock(&mut self, clock: PlaybackClock) -> bool {
        if clock.generation != self.play_generation || self.loaded.is_none() {
            return false;
        }
        if !self.previewing
            && let Some(point) = &mut self.resume
        {
            point.start = clock.start;
        }
        self.playing = !clock.start.paused;
        if !self.previewing && self.playing && clock.start.position > 0. {
            if let Some(failed) = &mut self.failed_queue_tracks {
                failed.clear();
            }
            self.heartbeat_resolved();
        }
        true
    }
}

pub(super) fn set_timeline(ui: &AppWindow, position: f64, duration: f64) {
    ui.set_duration(duration as f32);
    if !ui.get_scrubbing() {
        ui.set_position(position as f32);
    }
    let (p, d) = (position as u64, duration as u64);
    ui.set_time_label(format!("{:02}:{:02} / {:02}:{:02}", p / 60, p % 60, d / 60, d % 60).into());
}

#[cfg(all(test, feature = "ui-smoke"))]
mod tests {
    use super::*;

    fn track(source: &str, id: &str) -> Item {
        Item {
            key: ResourceKey {
                source: source.into(),
                id: id.into(),
            },
            kind: "track".into(),
            title: format!("track {id}"),
            duration_ms: 180_000,
            ..Default::default()
        }
    }

    #[test]
    fn restore_uses_the_played_source_and_ignores_outdated_clocks_and_previews() -> Result<()> {
        let _clock = crate::test_ui::test_clock()?;
        let dir = tempfile::tempdir()?;
        let db = Library::open(dir.path())?;
        let song = track("plugin", "7");
        let queue_song = track("local", "7");
        let mut queue = Queue::default();
        queue.items.push(queue_song);
        queue.index = Some(0);
        let ui = AppWindow::new()?;
        let plugin = serde_json::from_value(
            json!({"id":"plugin","name":"测试源","version":"1","abi":2,"capabilities":[],"network":[],"media":[]}),
        )?;
        let mut state = UiState::new(queue, Default::default(), vec![(plugin, true)]);
        let (jobs, rx) = mpsc::channel();
        let (commands, _engine) = mpsc::channel();
        assert!(ResumePoint::read(&db).is_none()); // Existing installations have only a queue.
        let start = PlaybackStart {
            position: 41.75,
            paused: true,
        };
        db.set(
            KEY,
            json!(ResumePoint {
                item: song.clone(),
                start
            }),
        )?;
        let point = ResumePoint::read(&db).unwrap();
        point.restore(&ui, &mut state, &jobs, &commands);
        let resolve = rx.try_iter().find(|j| j.request.op == "resolve").unwrap();
        assert_eq!(
            (resolve.source.as_str(), resolve.request.args["id"].as_str()),
            ("plugin", Some("7"))
        );
        assert_eq!(ui.get_position(), 41.75);
        state.stream_started(&ui, song.key.clone());
        assert!(!ui.get_playing());
        let generation = state.play_generation;
        let playing = PlaybackStart {
            position: 52.25,
            paused: false,
        };
        let clock = PlaybackClock {
            generation,
            start: playing,
            duration: 180.,
        };
        assert!(!state.record_clock(PlaybackClock {
            generation: generation - 1,
            ..clock
        }));
        assert_eq!(state.playback_start(), start);
        assert!(state.record_clock(clock));
        assert_eq!(state.playback_start(), playing);

        let preview = track("plugin", "9");
        play(
            &ui,
            &mut state,
            &jobs,
            &commands,
            preview.clone(),
            PlayKind::Preview,
        );
        state.stream_started(&ui, preview.key);
        assert!(state.record_clock(PlaybackClock {
            generation: state.play_generation,
            start: PlaybackStart {
                position: 2.,
                paused: false
            },
            ..clock
        }));
        state.playback_ended(&ui, &jobs, &commands);
        let saved = state.resume.clone().unwrap();
        assert_eq!(saved.item.key, song.key);
        assert_eq!(saved.start, playing);
        saved.restore(&ui, &mut state, &jobs, &commands);
        state.stream_started(&ui, song.key);
        state.playback_ended(&ui, &jobs, &commands);
        assert!(state.resume.is_none());
        Ok(())
    }

    #[test]
    fn automatic_failures_try_each_available_recording_once() -> Result<()> {
        let _clock = crate::test_ui::test_clock()?;
        let ui = AppWindow::new()?;
        let (jobs, rx) = mpsc::channel();
        let (commands, _engine) = mpsc::channel();
        for mode_steps in 0..4 {
            let first = track("local", "a");
            let second = track("local", "b");
            let mut queue = Queue::default();
            for _ in 0..mode_steps {
                queue.cycle_playback_mode();
            }
            queue.items = vec![
                first.clone(),
                track("disabled", "c"),
                first.clone(),
                second.clone(),
            ];
            queue.index = Some(0);
            let mut state = UiState::new(queue, Default::default(), vec![]);
            play(&ui, &mut state, &jobs, &commands, first, PlayKind::Queue);
            let first_load = state.play_generation;
            state.track_failed(&ui, &jobs, &commands, first_load, "音乐源未返回播放地址");
            assert_eq!(state.queue.index, Some(3));
            let second_load = state.play_generation;
            state.stream_started(&ui, second.key);
            state.track_failed(&ui, &jobs, &commands, first_load, "上一首的延迟错误");
            assert!(ui.get_playing());
            // Obtaining a URL alone must not clear the first failure.
            state.track_failed(&ui, &jobs, &commands, second_load, "无法解码");
            let ids: Vec<_> = rx
                .try_iter()
                .filter(|j| j.request.op == "resolve")
                .map(|j| j.request.args["id"].as_str().unwrap().to_owned())
                .collect();
            assert_eq!(ids, ["a", "b"]);
            assert!(ui.get_status().contains("无法解码"));
            assert!(ui.get_status().contains("没有可继续播放的歌曲"));
            assert!(!ui.get_playing() && state.resume.is_none());
        }
        Ok(())
    }

    #[test]
    fn failed_manual_play_preview_and_paused_playback_keep_the_queue_cursor() -> Result<()> {
        let _clock = crate::test_ui::test_clock()?;
        let ui = AppWindow::new()?;
        let first = track("local", "a");
        let second = track("local", "b");
        let mut queue = Queue::default();
        queue.items = vec![first.clone(), second.clone()];
        queue.index = Some(0);
        let mut state = UiState::new(queue, Default::default(), vec![]);
        let (jobs, rx) = mpsc::channel();
        let (commands, _engine) = mpsc::channel();
        let saved = PlaybackStart {
            position: 37.,
            paused: true,
        };
        for kind in [
            PlayKind::Audio,
            PlayKind::Resume(saved),
            PlayKind::Preview,
            PlayKind::Queue,
        ] {
            let item = if kind == PlayKind::Preview {
                second.clone()
            } else {
                first.clone()
            };
            play(&ui, &mut state, &jobs, &commands, item.clone(), kind);
            if kind == PlayKind::Queue {
                state.stream_started(&ui, item.key);
                state.toggle_pause(&ui, &commands);
            }
            let generation = state.play_generation;
            state.track_failed(&ui, &jobs, &commands, generation, "本次无法播放");
            assert_eq!(
                rx.try_iter().filter(|j| j.request.op == "resolve").count(),
                1
            );
            assert_eq!(state.queue.index, Some(0));
            let point = state.resume.as_ref().unwrap();
            assert_eq!(point.item.key, first.key);
            assert!(point.start.paused);
            if matches!(kind, PlayKind::Preview | PlayKind::Resume(_)) {
                assert_eq!(point.start, saved);
            }
        }
        Ok(())
    }

    #[test]
    fn missing_files_disabled_sources_and_invalid_positions_do_not_autoplay() -> Result<()> {
        let _clock = crate::test_ui::test_clock()?;
        let dir = tempfile::tempdir()?;
        let db = Library::open(dir.path())?;
        for (song, position) in [
            (track("local", "missing"), 12.),
            (track("plugin", "1"), -1.),
        ] {
            db.set(
                KEY,
                json!(ResumePoint {
                    item: song,
                    start: PlaybackStart {
                        position,
                        paused: false
                    }
                }),
            )?;
            assert!(ResumePoint::read(&db).is_none());
        }
        let ui = AppWindow::new()?;
        let plugin = serde_json::from_value(
            json!({"id":"plugin","name":"测试源","version":"1","abi":2,"capabilities":[],"network":[],"media":[]}),
        )?;
        let mut state = UiState::new(Queue::default(), Default::default(), vec![(plugin, false)]);
        let (jobs, rx) = mpsc::channel();
        let (commands, _engine) = mpsc::channel();
        ResumePoint {
            item: track("plugin", "1"),
            start: PlaybackStart::default(),
        }
        .restore(&ui, &mut state, &jobs, &commands);
        assert!(rx.try_recv().is_err() && state.now_playing.is_none());
        Ok(())
    }
}
