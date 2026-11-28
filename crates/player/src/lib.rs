mod audio;
mod inline;
mod output;
mod video;
use anyhow::{Result, anyhow};
pub use audio::{Equalizer, Gains, Normalization, Outputs};
pub use inline::{InlineVideo, VideoFrame, VideoFrames};
use libloading::Library;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};
pub use video::{Video, VideoControl, VideoQuality};
type Handle = *mut c_void;
type MpvCommand = unsafe extern "C" fn(Handle, *const *const c_char) -> c_int;
struct OwnedMpv {
    handle: Handle,
    destroy: unsafe extern "C" fn(Handle),
}
impl Drop for OwnedMpv {
    fn drop(&mut self) {
        unsafe {
            (self.destroy)(self.handle);
        }
    }
}
#[repr(C)]
struct Event {
    event: c_int,
    error: c_int,
    userdata: u64,
    data: *mut c_void,
}
#[repr(C)]
struct EndFile {
    reason: c_int,
    error: c_int,
    playlist_entry_id: i64,
    playlist_insert_id: i64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlaybackStart {
    pub position: f64,
    pub paused: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct PlaybackClock {
    pub generation: u64,
    pub start: PlaybackStart,
    pub duration: f64,
}

#[derive(Debug)]
pub enum Command {
    Open {
        generation: u64,
        start: PlaybackStart,
        url: String,
        referrer: Option<String>,
        video: Option<Video>,
        gains: Gains,
    },
    ShowInline(InlineVideo),
    HideInline,
    InlineSpeed(f64),
    Normalize(Normalization),
    Output(String),
    Equalizer {
        device: String,
        eq: Equalizer,
    },
    Pause(bool),
    Seek(f64),
    ReadClock(Sender<Option<PlaybackClock>>),
    Volume(f64),
    Stop,
    Quit,
}
#[derive(Debug)]
pub enum PlayerEvent {
    Ready,
    Outputs(Outputs),
    AudioError(String),
    Position(PlaybackClock),
    VideoControl(VideoControl),
    InlineError(u64, String),
    Ended(u64),
    TrackFailed(u64, String),
    Error(String),
}
fn load_file(
    url: String,
    referrer: Option<String>,
    video: Option<Video>,
    position: f64,
    has_playlist_index: bool,
) -> Vec<String> {
    let mut args = vec!["loadfile".into(), url, "replace".into()];
    let mut options = Vec::new();
    // libmpv's per-file option list uses commas. Length quoting preserves signed URLs and titles.
    let quoted = |key: &str, value: &str| format!("{key}=%{}%{value}", value.len());
    if let Some(page) = referrer {
        options.push(quoted("referrer", &page));
    }
    if let Some(video) = video {
        options.push("vid=auto".into());
        options.push(format!("speed={}", video.speed));
        options.push(quoted("force-media-title", &video.title.replace('\0', "")));
        if let Some(audio) = video.audio_url {
            options.push(quoted("audio-files-append", &audio));
        }
    }
    if position.is_finite() && position > 0.0 {
        options.push(format!("start={position}"));
    }
    if !options.is_empty() {
        // mpv 0.38 added a playlist-index argument before the per-file options.
        if has_playlist_index {
            args.push("-1".into());
        }
        args.push(options.join(","));
    }
    args
}

unsafe fn dispatch(h: Handle, command: MpvCommand, args: &[impl AsRef<str>]) -> Result<c_int> {
    let strings = args
        .iter()
        .map(|s| CString::new(s.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut ptrs = strings.iter().map(|s| s.as_ptr()).collect::<Vec<_>>();
    ptrs.push(std::ptr::null());
    Ok(unsafe { command(h, ptrs.as_ptr()) })
}
pub struct Player {
    pub commands: Sender<Command>,
    pub events: Receiver<PlayerEvent>,
    pub frames: VideoFrames,
}
impl Player {
    pub fn clock(&self) -> Option<PlaybackClock> {
        let (tx, rx) = mpsc::channel();
        self.commands.send(Command::ReadClock(tx)).ok()?;
        rx.recv_timeout(Duration::from_millis(500)).ok().flatten()
    }

    pub fn start(path: Option<&Path>) -> Self {
        let (tx, rx) = mpsc::channel();
        let (etx, erx) = mpsc::channel();
        let path = path.map(Path::to_path_buf);
        let frames = VideoFrames::default();
        let render_frames = frames.clone();
        std::thread::spawn(move || {
            if let Err(e) = unsafe { run(path.as_deref(), rx, etx.clone(), render_frames) } {
                let _ = etx.send(PlayerEvent::Error(e.to_string()));
            }
        });
        Self {
            commands: tx,
            events: erx,
            frames,
        }
    }
}
unsafe fn run(
    path: Option<&Path>,
    rx: Receiver<Command>,
    tx: Sender<PlayerEvent>,
    frames: VideoFrames,
) -> Result<()> {
    unsafe {
        let default = if cfg!(windows) {
            "mpv-2.dll"
        } else if cfg!(target_os = "macos") {
            "libmpv.2.dylib"
        } else if cfg!(target_os = "android") {
            "libmpv.so"
        } else {
            "libmpv.so.2"
        };
        let exe = std::env::current_exe()?.parent().unwrap().join(default);
        let lib = Library::new(path.unwrap_or(if exe.exists() {
            &exe
        } else {
            Path::new(default)
        }))
        .map_err(|e| anyhow!("无法加载 libmpv（{default}）：{e}"))?;
        #[cfg(target_os = "android")]
        {
            let set_vm = lib
                .get::<unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> c_int>(
                    b"av_jni_set_java_vm",
                )?;
            anyhow::ensure!(
                set_vm(ndk_context::android_context().vm(), std::ptr::null_mut()) >= 0,
                "Android audio initialization failed"
            );
        }
        let create: libloading::Symbol<unsafe extern "C" fn() -> Handle> =
            lib.get(b"mpv_create")?;
        let client_version =
            lib.get::<unsafe extern "C" fn() -> std::ffi::c_ulong>(b"mpv_client_api_version")?;
        let has_playlist_index = client_version() >= (2 << 16 | 3);
        let init: libloading::Symbol<unsafe extern "C" fn(Handle) -> c_int> =
            lib.get(b"mpv_initialize")?;
        let set: libloading::Symbol<
            unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
        > = lib.get(b"mpv_set_option_string")?;
        let command: libloading::Symbol<MpvCommand> = lib.get(b"mpv_command")?;
        let prop: libloading::Symbol<
            unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
        > = lib.get(b"mpv_get_property")?;
        let wait: libloading::Symbol<unsafe extern "C" fn(Handle, f64) -> *const Event> =
            lib.get(b"mpv_wait_event")?;
        let destroy: libloading::Symbol<unsafe extern "C" fn(Handle)> =
            lib.get(b"mpv_terminate_destroy")?;
        let error: libloading::Symbol<unsafe extern "C" fn(c_int) -> *const c_char> =
            lib.get(b"mpv_error_string")?;
        let api = audio::AudioApi {
            create: *create,
            init: *init,
            set: *set,
            cmd: *command,
            destroy: *destroy,
            get: *lib.get(b"mpv_get_property_string")?,
            free: *lib.get(b"mpv_free")?,
        };
        let h = create();
        if h.is_null() {
            return Err(anyhow!("mpv_create failed"));
        }
        let _owned = OwnedMpv {
            handle: h,
            destroy: *destroy,
        };
        for (k, v) in [
            ("config", "no"),
            ("volume-gain-max", "18"),
            ("vid", "no"),
            ("vo", "gpu-next,gpu"),
            ("autofit-larger", "85%x85%"),
            ("input-vo-keyboard", "yes"),
            ("input-cursor", "yes"),
            // The video controller loads the bundled OSC when a picture is first opened.
            ("osc", "no"),
            ("load-console", "yes"),
            ("script-opts", video::OSC_OPTIONS),
            ("audio-display", "no"),
            ("terminal", "no"),
            ("idle", "yes"),
            ("load-scripts", "no"),
            ("ytdl", "no"),
        ] {
            set(h, CString::new(k)?.as_ptr(), CString::new(v)?.as_ptr());
        }
        if init(h) < 0 {
            return Err(anyhow!("mpv_initialize failed"));
        }
        // Closing the picture keeps the current soundtrack and the shared player alive.
        let keys = "CLOSE_WIN set vid no\nSPACE cycle pause\nf cycle fullscreen\nESC set fullscreen no\nMBTN_LEFT_DBL cycle fullscreen\nLEFT seek -5\nRIGHT seek 5\ns script-binding osc/speed\nq script-binding osc/quality";
        for args in [
            &["define-section", "music-video", keys, "force"][..],
            &["enable-section", "music-video"][..],
        ] {
            let result = dispatch(h, *command, args)?;
            if result < 0 {
                return Err(anyhow!(
                    "视频窗口按键初始化失败：{}",
                    CStr::from_ptr(error(result)).to_string_lossy()
                ));
            }
        }
        let _ = tx.send(PlayerEvent::Ready);
        let mut active = false;
        let mut requested_entry = None;
        let mut started_entry = None;
        let mut ticks = 0;
        let mut control_script = None;
        let mut routing = output::Routing::new();
        let mut eq = Equalizer::default();
        let mut eq_device = String::new();
        let mut loudness = audio::Loudness::default();
        let mut generation = 0;
        let mut pending_seek = None;
        let read_clock = |generation, pending_seek: Option<f64>| {
            let (mut position, mut duration) = (0.0f64, 0.0f64);
            let mut paused: c_int = 0;
            if prop(
                h,
                c"time-pos".as_ptr(),
                5,
                (&mut position as *mut f64).cast(),
            ) < 0
                || !position.is_finite()
                || prop(h, c"pause".as_ptr(), 3, (&mut paused as *mut c_int).cast()) < 0
            {
                return None;
            }
            prop(
                h,
                c"duration".as_ptr(),
                5,
                (&mut duration as *mut f64).cast(),
            );
            Some(PlaybackClock {
                generation,
                start: PlaybackStart {
                    position: pending_seek.unwrap_or(position).max(0.),
                    paused: paused != 0,
                },
                duration,
            })
        };
        let mut pending_file = None;
        let mut inline = inline::InlineOutput::new(&lib, h, api, frames);
        let inline_failure = |inline: &mut inline::InlineOutput<'_>, error: anyhow::Error| {
            if let Some(ticket) = inline.ticket() {
                let _ = tx.send(PlayerEvent::InlineError(ticket, error.to_string()));
            }
            inline.close();
        };
        let report_audio = |error: anyhow::Error| {
            let _ = tx.send(PlayerEvent::AudioError(error.to_string()));
        };
        loop {
            match rx.recv_timeout(Duration::from_millis(30)) {
                Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(c) => {
                    let args: Vec<String> = match c {
                        Command::Open {
                            generation: next_generation,
                            start,
                            url,
                            referrer,
                            video,
                            gains,
                        } => {
                            inline.close();
                            generation = next_generation;
                            pending_seek = None;
                            pending_file = None;
                            requested_entry = None;
                            loudness.stop();
                            active = false;
                            let (url, menu) = if let Some(v) = &video {
                                if control_script.is_none() {
                                    match video::load_controls(h, *command) {
                                        Ok(script) => control_script = Some(script),
                                        Err(error) => {
                                            let _ = tx.send(PlayerEvent::TrackFailed(
                                                generation,
                                                error.to_string(),
                                            ));
                                            continue;
                                        }
                                    }
                                }
                                let selected = v.initial_url(&url);
                                (selected.to_owned(), v.menu_data(selected, &url))
                            } else {
                                (url, String::new())
                            };
                            dispatch(h, *command, &["set", "user-data/music-video", &menu])?;
                            let audio_url = video
                                .as_ref()
                                .and_then(|v| v.audio_url.as_deref())
                                .unwrap_or(&url);
                            if let Err(e) =
                                loudness.open(api, h, audio_url, referrer.as_deref(), gains)
                            {
                                report_audio(e);
                            }
                            api.command(
                                h,
                                &["set", "pause", if start.paused { "yes" } else { "no" }],
                            )?;
                            pending_file = Some(load_file(
                                url,
                                referrer,
                                video,
                                start.position,
                                has_playlist_index,
                            ));
                            Vec::new()
                        }
                        Command::ShowInline(v) => {
                            inline.request(v);
                            Vec::new()
                        }
                        Command::HideInline => {
                            inline.close();
                            Vec::new()
                        }
                        Command::InlineSpeed(speed) => {
                            inline.speed(speed);
                            Vec::new()
                        }
                        Command::Normalize(mode) => {
                            if let Err(e) = loudness.set_mode(api, h, mode) {
                                report_audio(e);
                            }
                            Vec::new()
                        }
                        Command::Output(id) => {
                            routing.select(id);
                            Vec::new()
                        }
                        Command::Equalizer { device, eq: next } => {
                            if routing.state.as_ref().is_some_and(|s| s.active == device) {
                                match audio::set_eq(api, h, eq, next) {
                                    Ok(()) => eq = next,
                                    Err(e) => report_audio(e),
                                }
                            }
                            Vec::new()
                        }
                        Command::Pause(p) => vec![
                            "set".into(),
                            "pause".into(),
                            if p { "yes" } else { "no" }.into(),
                        ],
                        Command::ReadClock(reply) => {
                            let _ = reply.send(if active {
                                read_clock(generation, pending_seek)
                            } else {
                                None
                            });
                            Vec::new()
                        }
                        Command::Seek(s) => {
                            pending_seek = Some(s);
                            vec!["seek".into(), s.to_string(), "absolute+exact".into()]
                        }
                        Command::Volume(v) => vec![
                            "set".into(),
                            "volume".into(),
                            v.clamp(0.0, 100.0).to_string(),
                        ],
                        Command::Stop => {
                            pending_seek = None;
                            inline.close();
                            active = false;
                            pending_file = None;
                            requested_entry = None;
                            loudness.stop();
                            dispatch(h, *command, &["set", "user-data/music-video", ""])?;
                            vec!["stop".into()]
                        }
                        Command::Quit => unreachable!(),
                    };
                    if !args.is_empty()
                        && let status = dispatch(h, *command, &args)?
                        && status < 0
                    {
                        if args[0] == "seek" {
                            pending_seek = None;
                        }
                        let _ = tx.send(PlayerEvent::AudioError(
                            CStr::from_ptr(error(status)).to_string_lossy().into_owned(),
                        ));
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Err(e) = loudness.poll(api, h) {
                report_audio(e);
            }
            if !loudness.waiting()
                && let Some(args) = pending_file.take()
            {
                let load = dispatch(h, *command, &args).and_then(|status| {
                    if status < 0 {
                        return Err(anyhow!(
                            "{}",
                            CStr::from_ptr(error(status)).to_string_lossy()
                        ));
                    }
                    let mut entry = 0i64;
                    let status = prop(
                        h,
                        c"playlist/0/id".as_ptr(),
                        4,
                        (&mut entry as *mut i64).cast(),
                    );
                    if status < 0 {
                        return Err(anyhow!(
                            "未能读取当前音轨 · {}",
                            CStr::from_ptr(error(status)).to_string_lossy()
                        ));
                    }
                    Ok(entry)
                });
                match load {
                    Ok(entry) => requested_entry = Some(entry),
                    Err(e) => {
                        let _ = tx.send(PlayerEvent::TrackFailed(generation, e.to_string()));
                    }
                }
            }
            match routing.refresh(api, h) {
                Ok(Some(outputs)) => {
                    if eq_device != outputs.active {
                        eq_device.clone_from(&outputs.active);
                        match audio::set_eq(api, h, eq, Equalizer::default()) {
                            Ok(()) => eq = Equalizer::default(),
                            Err(e) => report_audio(e),
                        }
                    }
                    let _ = tx.send(PlayerEvent::Outputs(outputs));
                }
                Err(e) => report_audio(e),
                _ => {}
            }
            loop {
                let e = &*wait(h, 0.0);
                if e.event == 0 {
                    break;
                }
                if e.event == 5
                    && let Err(err) = inline.complete(e.userdata, e.error)
                {
                    inline_failure(&mut inline, err);
                }
                // File events from the previous load can arrive after its replacement.
                if e.event == 6 && !e.data.is_null() {
                    // MPV_EVENT_START_FILE carries the playlist entry ID as its first field.
                    started_entry = Some(*e.data.cast::<i64>());
                }
                if e.event == 8 {
                    active = requested_entry.is_some() && requested_entry == started_entry;
                }
                if e.event == 21 && active {
                    // MPV_EVENT_PLAYBACK_RESTART completes a load or seek.
                    pending_seek = None;
                }
                if e.event == 17 {
                    // MPV_EVENT_VIDEO_RECONFIG
                    inline.update_picture_size();
                }
                if e.event == 16
                    && !e.data.is_null()
                    && let Some(control) = (&*e.data.cast::<video::ClientMessage>()).control()
                {
                    let _ = tx.send(PlayerEvent::VideoControl(control));
                }
                if e.event == 7 && !e.data.is_null() {
                    let end = &*(e.data as *const EndFile);
                    if requested_entry == Some(end.playlist_entry_id) {
                        active = false;
                        requested_entry = None;
                        match end.reason {
                            0 => {
                                let _ = tx.send(PlayerEvent::Ended(generation));
                            }
                            4 => {
                                let reason = CStr::from_ptr(error(end.error)).to_string_lossy();
                                let _ = tx.send(PlayerEvent::TrackFailed(
                                    generation,
                                    reason.into_owned(),
                                ));
                            }
                            5 => requested_entry = Some(end.playlist_insert_id),
                            _ => {}
                        }
                    }
                }
            }
            if active && let Err(err) = inline.start_if_needed() {
                inline_failure(&mut inline, err);
            }
            ticks += 1;
            if active
                && ticks % 8 == 0
                && let Some(clock) = read_clock(generation, pending_seek)
            {
                let _ = tx.send(PlayerEvent::Position(clock));
            }
        }
        inline.close();
        Ok(())
    }
}
