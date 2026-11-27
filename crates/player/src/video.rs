use super::{Handle, MpvCommand, dispatch};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::ffi::{CStr, c_char, c_int};

pub(super) const OSC_OPTIONS: &str = concat!(
    "osc-layout=floating,osc-vidscale=no,osc-hidetimeout=2200,osc-seekbarkeyframes=no,osc-idlescreen=no,",
    "osc-volume_mbtn_left_command=script-message music-video volume mute,",
    "osc-volume_wheel_up_command=script-message music-video volume up,",
    "osc-volume_wheel_down_command=script-message music-video volume down,",
    "osc-close_mbtn_left_command=set vid no"
);

#[derive(Debug, Deserialize, Serialize)]
pub struct VideoQuality {
    pub height: u32,
    pub url: String,
}

#[derive(Debug)]
pub struct Video {
    pub audio_url: Option<String>,
    pub title: String,
    pub speed: f64,
    pub preferred_height: u32,
    pub qualities: Vec<VideoQuality>,
}

impl VideoQuality {
    pub fn preferred(qualities: &[Self], height: u32) -> Option<&Self> {
        if height == 0 {
            return None;
        }
        qualities
            .iter()
            .filter(|q| q.height <= height)
            .max_by_key(|q| q.height)
            .or_else(|| qualities.iter().min_by_key(|q| q.height))
    }
}

impl Video {
    pub(super) fn initial_url<'a>(&'a self, default: &'a str) -> &'a str {
        VideoQuality::preferred(&self.qualities, self.preferred_height)
            .map_or(default, |q| q.url.as_str())
    }

    pub(super) fn menu_data(&self, url: &str, default: &str) -> String {
        json!({"path":url, "auto":default, "qualities":self.qualities, "preferred":self.preferred_height}).to_string()
    }
}

pub(super) unsafe fn load_controls(h: Handle, command: MpvCommand) -> Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("osc.lua");
    std::fs::write(&path, include_bytes!("music_video.lua"))?;
    let status = unsafe { dispatch(h, command, &["load-script", &path.to_string_lossy()])? };
    ensure!(status >= 0, "无法加载视频控制栏");
    Ok(dir)
}

#[derive(Debug)]
pub enum VideoControl {
    Mute,
    VolumeStep(f32),
    Speed(f64),
    Height(u32),
}

// Layout of mpv_event_client_message from the libmpv client API.
#[repr(C)]
pub(super) struct ClientMessage {
    num_args: c_int,
    args: *const *const c_char,
}

impl ClientMessage {
    pub(super) unsafe fn control(&self) -> Option<VideoControl> {
        if self.num_args != 3 {
            return None;
        }
        let args = unsafe { std::slice::from_raw_parts(self.args, 3) };
        let arg = |index: usize| unsafe { CStr::from_ptr(args[index]).to_str().ok() };
        if arg(0)? != "music-video" {
            return None;
        }
        match (arg(1)?, arg(2)?) {
            ("volume", "mute") => Some(VideoControl::Mute),
            ("volume", "up") => Some(VideoControl::VolumeStep(5.)),
            ("volume", "down") => Some(VideoControl::VolumeStep(-5.)),
            ("speed", value) => value
                .parse()
                .ok()
                .filter(|v| (0.5..=2.).contains(v))
                .map(VideoControl::Speed),
            ("height", value) => value.parse().ok().map(VideoControl::Height),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_resolution_uses_an_available_stream() {
        let mut video = Video {
            audio_url: None,
            title: String::new(),
            speed: 1.,
            preferred_height: 0,
            qualities: vec![
                VideoQuality {
                    height: 360,
                    url: "low".into(),
                },
                VideoQuality {
                    height: 1080,
                    url: "high".into(),
                },
            ],
        };
        for (height, expected) in [(0, "auto"), (1080, "high"), (720, "low"), (240, "low")] {
            video.preferred_height = height;
            assert_eq!(video.initial_url("auto"), expected);
        }
        video.qualities.clear();
        assert_eq!(video.initial_url("legacy"), "legacy");
    }
}
