use crate::AppWindow;
use anyhow::Result;
use music_library::Library;
use music_player::VideoControl;
use serde_json::json;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PlaybackSettings {
    pub volume: f32,
    pub muted: bool,
    pub video_speed: f64,
    pub video_height: u32,
}

impl PlaybackSettings {
    pub fn read(db: &Library) -> Self {
        let volume = db.get("player-volume");
        Self {
            volume: volume["level"]
                .as_f64()
                .filter(|&v| v > 0.0 && v <= 100.0)
                .unwrap_or(70.) as f32,
            muted: volume["muted"].as_bool().unwrap_or(false),
            video_speed: db
                .get("video-speed")
                .as_f64()
                .filter(|v| (0.5..=2.).contains(v))
                .unwrap_or(1.),
            video_height: db
                .get("video-height")
                .as_u64()
                .and_then(|v| v.try_into().ok())
                .unwrap_or(0),
        }
    }

    pub fn save_changes(self, db: &Library, saved: &Cell<Self>) -> Result<()> {
        let old = saved.get();
        if self.volume != old.volume || self.muted != old.muted {
            db.set(
                "player-volume",
                json!({"level":self.volume,"muted":self.muted}),
            )?;
        }
        if self.video_speed != old.video_speed {
            db.set("video-speed", json!(self.video_speed))?;
        }
        if self.video_height != old.video_height {
            db.set("video-height", json!(self.video_height))?;
        }
        saved.set(self);
        Ok(())
    }

    pub fn apply_video_control(settings: &Cell<Self>, ui: &AppWindow, control: VideoControl) {
        let mut next = settings.get();
        match control {
            VideoControl::Mute => ui.invoke_toggle_mute(),
            VideoControl::VolumeStep(step) => {
                ui.invoke_set_player_volume(ui.get_volume_level() + step)
            }
            VideoControl::Speed(speed) => {
                next.video_speed = speed;
                settings.set(next);
            }
            VideoControl::Height(height) => {
                next.video_height = height;
                settings.set(next);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_mute_and_video_preferences_survive_reopening_the_database() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = Library::open(dir.path())?;
        let initial = PlaybackSettings::read(&db);
        assert_eq!(
            (
                initial.volume,
                initial.muted,
                initial.video_speed,
                initial.video_height
            ),
            (70., false, 1., 0)
        );
        let saved = Cell::new(initial);
        let muted = PlaybackSettings {
            volume: 34.,
            muted: true,
            video_speed: 1.5,
            video_height: 720,
        };
        muted.save_changes(&db, &saved)?;
        drop(db);
        let reopened = Library::open(dir.path())?;
        assert_eq!(PlaybackSettings::read(&reopened), muted);
        // A second instance changing only its picture size must not overwrite the volume.
        let other = PlaybackSettings {
            video_height: 480,
            ..initial
        };
        other.save_changes(&reopened, &Cell::new(initial))?;
        assert_eq!(
            PlaybackSettings::read(&reopened),
            PlaybackSettings {
                video_height: 480,
                ..muted
            }
        );
        Ok(())
    }
}
