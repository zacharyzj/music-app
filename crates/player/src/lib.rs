mod video;
use anyhow::{Result, anyhow};
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
}
#[derive(Debug)]
pub enum Command {
    Open {
        url: String,
        referrer: Option<String>,
        video: Option<Video>,
    },
    Pause(bool),
    Seek(f64),
    Volume(f64),
    Stop,
    Quit,
}
#[derive(Debug)]
pub enum PlayerEvent {
    Ready,
    Position(f64, f64, bool),
    VideoControl(VideoControl),
    Ended,
    Error(String),
}
fn load_file(url: String, referrer: Option<String>, video: Option<Video>) -> Vec<String> {
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
        if video.start > 0.0 {
            options.push(format!("start={}", video.start));
        }
    }
    if !options.is_empty() {
        args.extend(["-1".into(), options.join(",")]);
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
}
impl Player {
    pub fn start(path: Option<&Path>) -> Self {
        let (tx, rx) = mpsc::channel();
        let (etx, erx) = mpsc::channel();
        let path = path.map(Path::to_path_buf);
        std::thread::spawn(move || {
            if let Err(e) = unsafe { run(path.as_deref(), rx, etx.clone()) } {
                let _ = etx.send(PlayerEvent::Error(e.to_string()));
            }
        });
        Self {
            commands: tx,
            events: erx,
        }
    }
}
unsafe fn run(path: Option<&Path>, rx: Receiver<Command>, tx: Sender<PlayerEvent>) -> Result<()> {
    unsafe {
        let default = if cfg!(windows) {
            "mpv-2.dll"
        } else if cfg!(target_os = "macos") {
            "libmpv.2.dylib"
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
        let create: libloading::Symbol<unsafe extern "C" fn() -> Handle> =
            lib.get(b"mpv_create")?;
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
        let mut ticks = 0;
        let mut control_script = None;
        loop {
            match rx.recv_timeout(Duration::from_millis(30)) {
                Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(c) => {
                    let args: Vec<String> = match c {
                        Command::Open {
                            url,
                            referrer,
                            video,
                        } => {
                            active = false;
                            let (url, menu) = if let Some(v) = &video {
                                if control_script.is_none() {
                                    match video::load_controls(h, *command) {
                                        Ok(script) => control_script = Some(script),
                                        Err(error) => {
                                            let _ = tx.send(PlayerEvent::Error(error.to_string()));
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
                            load_file(url, referrer, video)
                        }
                        Command::Pause(p) => vec![
                            "set".into(),
                            "pause".into(),
                            if p { "yes" } else { "no" }.into(),
                        ],
                        Command::Seek(s) => {
                            vec!["seek".into(), s.to_string(), "absolute+exact".into()]
                        }
                        Command::Volume(v) => vec![
                            "set".into(),
                            "volume".into(),
                            v.clamp(0.0, 100.0).to_string(),
                        ],
                        Command::Stop => {
                            active = false;
                            dispatch(h, *command, &["set", "user-data/music-video", ""])?;
                            vec!["stop".into()]
                        }
                        Command::Quit => unreachable!(),
                    };
                    let status = dispatch(h, *command, &args)?;
                    if status < 0 {
                        let _ = tx.send(PlayerEvent::Error(
                            CStr::from_ptr(error(status)).to_string_lossy().into_owned(),
                        ));
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            loop {
                let e = &*wait(h, 0.0);
                if e.event == 0 {
                    break;
                }
                if e.event == 8 {
                    active = true;
                }
                if e.event == 16
                    && !e.data.is_null()
                    && let Some(control) = (&*e.data.cast::<video::ClientMessage>()).control()
                {
                    let _ = tx.send(PlayerEvent::VideoControl(control));
                }
                if e.event == 7 && !e.data.is_null() {
                    let end = &*(e.data as *const EndFile);
                    if active && end.reason == 0 {
                        active = false;
                        let _ = tx.send(PlayerEvent::Ended);
                    } else if end.reason == 4 {
                        active = false;
                        let _ = tx.send(PlayerEvent::Error(format!("播放失败 ({})", end.error)));
                    }
                }
            }
            ticks += 1;
            if ticks % 8 == 0 {
                let (mut p, mut d) = (0.0f64, 0.0f64);
                prop(h, c"time-pos".as_ptr(), 5, &mut p as *mut _ as *mut c_void);
                prop(h, c"duration".as_ptr(), 5, &mut d as *mut _ as *mut c_void);
                let mut paused: c_int = 0;
                prop(
                    h,
                    c"pause".as_ptr(),
                    3,
                    &mut paused as *mut _ as *mut c_void,
                );
                let _ = tx.send(PlayerEvent::Position(p, d, paused != 0));
            }
        }
        Ok(())
    }
}
