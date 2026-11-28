use super::{Handle, MpvCommand, OwnedMpv, dispatch};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Normalization {
    #[default]
    Off,
    Track,
    Album(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Gains {
    pub track: Option<f64>,
    pub album: Option<f64>,
}

fn safe_gain(db: f64, peak: Option<f64>) -> Option<f64> {
    if !db.is_finite() {
        return None;
    }
    let ceiling = peak
        .filter(|p| p.is_finite() && *p > 0.)
        .map_or(0., |p| -1. - 20. * p.log10());
    Some(db.clamp(-60., 18.).min(ceiling))
}

impl Gains {
    pub fn from_stream(v: &serde_json::Value) -> Self {
        let gain = |g: &str, p: &str| safe_gain(v[g].as_f64()?, v[p].as_f64());
        Self {
            track: gain("gain", "peak"),
            album: gain("albumGain", "albumPeak"),
        }
    }
    fn merge(self, fallback: Self) -> Self {
        Self {
            track: self.track.or(fallback.track),
            album: self.album.or(fallback.album),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Equalizer(pub [i8; 3]);

impl Equalizer {
    pub fn bands(self) -> [i8; 3] {
        self.0.map(|v| v.clamp(-9, 9))
    }
    pub const PRESETS: [Self; 5] = [
        Self([0, 0, 0]),
        Self([1, 2, 1]),
        Self([3, -1, 2]),
        Self([-1, 2, 0]),
        Self([0, 0, 1]),
    ];
    fn filter(self) -> String {
        let [low, mid, high] = self.bands();
        let headroom = [low, mid, high]
            .into_iter()
            .map(|g| g.max(0) as i16)
            .sum::<i16>();
        format!(
            "@music-eq:lavfi=[volume=-{headroom}dB,bass=f=120:g={low},equalizer=f=1200:t=o:w=1:g={mid},treble=f=6000:g={high}]"
        )
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct OutputDevice {
    pub name: String,
    pub description: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outputs {
    pub devices: Vec<OutputDevice>,
    pub active: String,
    pub selected: String,
}

#[derive(Clone, Copy)]
pub(super) struct AudioApi {
    pub create: unsafe extern "C" fn() -> Handle,
    pub init: unsafe extern "C" fn(Handle) -> c_int,
    pub destroy: unsafe extern "C" fn(Handle),
    pub set: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    pub cmd: MpvCommand,
    pub get: unsafe extern "C" fn(Handle, *const c_char) -> *mut c_char,
    pub free: unsafe extern "C" fn(*mut c_void),
}
impl AudioApi {
    pub fn text(&self, h: Handle, key: &str) -> Option<String> {
        let k = CString::new(key).ok()?;
        unsafe {
            let ptr = (self.get)(h, k.as_ptr());
            if ptr.is_null() {
                return None;
            }
            let v = CStr::from_ptr(ptr).to_string_lossy().into_owned();
            (self.free)(ptr.cast());
            Some(v)
        }
    }
    fn number(&self, h: Handle, key: &str) -> Option<f64> {
        self.text(h, key)?
            .split_ascii_whitespace()
            .next()?
            .parse()
            .ok()
    }
    pub fn command(&self, h: Handle, args: &[&str]) -> Result<()> {
        ensure!(
            unsafe { dispatch(h, self.cmd, args)? } >= 0,
            "音频设置失败 ({})",
            args[0]
        );
        Ok(())
    }
    fn tags(&self, h: Handle) -> Gains {
        let metadata = self.text(h, "metadata").and_then(|s| {
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&s).ok()
        });
        let tag = |name: &str| -> Option<f64> {
            metadata
                .as_ref()?
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))?
                .1
                .as_str()?
                .split_ascii_whitespace()
                .next()?
                .parse()
                .ok()
        };
        let track = |name: &str| self.number(h, &format!("current-tracks/audio/replaygain-{name}"));
        Gains {
            track: safe_gain(
                tag("REPLAYGAIN_TRACK_GAIN")
                    .or_else(|| track("track-gain"))
                    .unwrap_or(f64::NAN),
                tag("REPLAYGAIN_TRACK_PEAK").or_else(|| track("track-peak")),
            ),
            // mpv can report track gain as album gain when only the track is tagged.
            album: tag("REPLAYGAIN_ALBUM_GAIN")
                .and_then(|g| safe_gain(g, tag("REPLAYGAIN_ALBUM_PEAK"))),
        }
    }
}

// A second, silent decoder measures untagged recordings off the audio output. Its lifetime
// is bounded by the pending request: skipping, stopping and disabling normalization cancel it.
struct Measurement {
    mpv: OwnedMpv,
    started: Instant,
    remote: bool,
}
impl Measurement {
    fn start(api: AudioApi, url: &str, referrer: Option<&str>) -> Result<Self> {
        let h = unsafe { (api.create)() };
        ensure!(!h.is_null(), "无法创建响度分析器");
        let mpv = OwnedMpv {
            handle: h,
            destroy: api.destroy,
        };
        let remote = !Path::new(url).is_file();
        let mut opts = vec![
            ("config", "no"),
            ("load-scripts", "no"),
            ("ytdl", "no"),
            ("terminal", "no"),
            ("vid", "no"),
            ("aid", "auto"),
            ("audio-display", "no"),
            ("ao", "null"),
            ("ao-null-untimed", "yes"),
            ("untimed", "yes"),
            ("keep-open", "always"),
            ("cache", "no"),
            ("demuxer-max-bytes", "1MiB"),
            ("network-timeout", "3"),
            ("af", "@measure:lavfi=[ebur128=metadata=1:peak=true]"),
        ];
        if remote {
            opts.push(("length", "24"));
        }
        if let Some(page) = referrer {
            opts.push(("referrer", page));
        }
        for (key, value) in opts {
            ensure!(
                unsafe {
                    (api.set)(
                        h,
                        CString::new(key)?.as_ptr(),
                        CString::new(value)?.as_ptr(),
                    )
                } >= 0,
                "不支持响度分析选项"
            );
        }
        ensure!(unsafe { (api.init)(h) } >= 0, "无法初始化响度分析器");
        api.command(h, &["loadfile", url])?;
        Ok(Self {
            mpv,
            started: Instant::now(),
            remote,
        })
    }

    fn poll(&self, api: AudioApi, mode: &Normalization) -> Option<Gains> {
        let h = self.mpv.handle;
        let tags = api.tags(h);
        if tags.album.is_some() && matches!(mode, Normalization::Album(_))
            || tags.track.is_some() && *mode == Normalization::Track
        {
            return Some(tags);
        }
        let eof = api.text(h, "eof-reached").as_deref() == Some("yes");
        if !(eof || self.started.elapsed() >= Duration::from_secs(if self.remote { 3 } else { 5 }))
        {
            return None;
        }
        let integrated = api.number(h, "af-metadata/measure/lavfi.r128.I");
        let peak = api.number(h, "af-metadata/measure/lavfi.r128.true_peak");
        let gain = integrated
            .filter(|v| (-69.9..=0.).contains(v))
            .and_then(|lufs| safe_gain(-18. - lufs, peak));
        // A partial scan cannot establish the true peak of the entire recording.
        let duration = api.number(h, "duration");
        let complete = eof && (!self.remote || duration.is_some_and(|d| d > 0. && d <= 24.));
        Some(tags.merge(Gains {
            track: gain.map(|g| if complete { g } else { g.min(0.) }),
            album: None,
        }))
    }
}

#[derive(Default)]
pub(super) struct Loudness {
    pub mode: Normalization,
    shared: Option<(String, f64)>,
    source: Option<(String, Option<String>, Gains)>,
    meter: Option<Measurement>,
    cache: std::collections::HashMap<String, Gains>,
    cache_key: String,
    gain: f64,
}
impl Loudness {
    pub fn stop(&mut self) {
        self.meter = None;
        self.source = None;
    }
    pub fn open(
        &mut self,
        api: AudioApi,
        h: Handle,
        url: &str,
        referrer: Option<&str>,
        gains: Gains,
    ) -> Result<()> {
        self.cache_key = url.to_owned();
        if let Ok(stat) = std::fs::metadata(url) {
            self.cache_key
                .push_str(&format!(":{:?}:{}", stat.modified().ok(), stat.len()));
        }
        self.source = Some((url.to_owned(), referrer.map(str::to_owned), gains));
        self.prepare(api, h)
    }
    pub fn set_mode(&mut self, api: AudioApi, h: Handle, mode: Normalization) -> Result<()> {
        if self.mode == mode {
            return Ok(());
        }
        self.mode = mode;
        self.prepare(api, h)
    }
    fn selected(&self, g: Gains) -> Option<f64> {
        match &self.mode {
            Normalization::Off => Some(0.),
            Normalization::Track => g.track,
            Normalization::Album(id) => self
                .shared
                .as_ref()
                .filter(|(key, _)| key == id)
                .map(|v| v.1)
                .or(g.album),
        }
    }
    fn apply(&mut self, api: AudioApi, h: Handle, gain: f64) -> Result<()> {
        if self.gain != gain {
            if api.text(h, "volume-gain").is_some() {
                api.command(h, &["set", "volume-gain", &gain.to_string()])?;
            } else if gain == 0. {
                api.command(h, &["af", "remove", "@music-loudness"])?;
            } else {
                let filter = format!("@music-loudness:lavfi=[volume={gain}dB]");
                api.command(h, &["af", "add", &filter])?;
            }
            self.gain = gain;
        }
        Ok(())
    }
    fn finish(&mut self, api: AudioApi, h: Handle, gains: Gains) -> Result<()> {
        let gain = self.selected(gains).unwrap_or_else(|| match self.mode {
            Normalization::Album(_) => gains.track.unwrap_or(0.).min(0.),
            _ => 0.,
        });
        if let Normalization::Album(id) = &self.mode {
            self.shared = Some((id.clone(), gain));
        }
        self.apply(api, h, gain)
    }
    fn prepare(&mut self, api: AudioApi, h: Handle) -> Result<()> {
        self.meter = None;
        let Some((url, referrer, given)) = &self.source else {
            return self.apply(api, h, 0.);
        };
        let cached = self.cache.get(&self.cache_key).copied();
        let gains = given.merge(cached.unwrap_or_default());
        if self.selected(gains).is_some() || cached.is_some() {
            return self.finish(api, h, gains);
        }
        match Measurement::start(api, url, referrer.as_deref()) {
            Ok(m) => self.meter = Some(m),
            Err(_) => return self.finish(api, h, gains),
        }
        Ok(())
    }
    pub fn waiting(&self) -> bool {
        self.meter.is_some()
    }
    pub fn poll(&mut self, api: AudioApi, h: Handle) -> Result<()> {
        let Some(result) = self.meter.as_ref().and_then(|m| m.poll(api, &self.mode)) else {
            return Ok(());
        };
        self.meter = None;
        let gains = self.source.as_ref().map_or(result, |s| s.2.merge(result));
        if gains.track.is_some() || gains.album.is_some() {
            if self.cache.len() >= 512 {
                self.cache.clear();
            }
            self.cache.insert(self.cache_key.clone(), gains);
        }
        self.finish(api, h, gains)
    }
}

pub(super) fn set_eq(api: AudioApi, h: Handle, previous: Equalizer, next: Equalizer) -> Result<()> {
    if previous == next {
        return Ok(());
    }
    if next.bands() == [0; 3] {
        api.command(h, &["af", "remove", "@music-eq"])
    } else {
        api.command(h, &["af", "add", &next.filter()])
    }
}

#[cfg(test)]
#[path = "audio_tests.rs"]
mod tests;
