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
