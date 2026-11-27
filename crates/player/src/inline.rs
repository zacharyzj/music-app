use super::{Handle, audio::AudioApi};
use anyhow::{Result, anyhow, ensure};
use libloading::Library;
use std::{
    ffi::{CString, c_char, c_int, c_void},
    ptr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle, Thread},
};

#[derive(Debug)]
pub struct InlineVideo {
    pub ticket: u64,
    pub url: String,
    pub referrer: Option<String>,
    pub speed: f64,
}

pub struct VideoFrame {
    pub ticket: u64,
    pub width: u32,
    pub height: u32,
    // libmpv's rgb0 format uses RGB byte order, with an unused fourth byte.
    pub pixels: Vec<u32>,
}

struct Surface {
    viewport: [i32; 2],
    picture: Option<[i32; 2]>,
    ticket: Option<u64>,
    frame: Option<VideoFrame>,
    thread: Option<Thread>,
}

impl Surface {
    fn render_size(&self) -> [i32; 2] {
        let [w, h] = self.picture.unwrap_or(self.viewport);
        let scale = (f64::from(self.viewport[0]) / f64::from(w))
            .min(f64::from(self.viewport[1]) / f64::from(h))
            .min(1.);
        [w, h].map(|n| (f64::from(n) * scale).round().max(1.) as i32)
    }

    fn wake(&self) {
        if let Some(t) = &self.thread {
            t.unpark();
        }
    }
}

#[derive(Clone)]
pub struct VideoFrames(Arc<Mutex<Surface>>);

impl Default for VideoFrames {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Surface {
            viewport: [640, 360],
            picture: None,
            ticket: None,
            frame: None,
            thread: None,
        })))
    }
}

impl VideoFrames {
    pub fn resize(&self, width: i32, height: i32) {
        if width < 1 || height < 1 {
            return;
        }
        let mut surface = self.0.lock().unwrap();
        if surface.viewport != [width, height] {
            surface.viewport = [width, height];
            surface.wake();
        }
    }

    fn set_picture_size(&self, size: [i32; 2]) {
        let mut surface = self.0.lock().unwrap();
        if surface.ticket.is_some() && surface.picture != Some(size) {
            surface.picture = Some(size);
            surface.wake();
        }
    }

    pub fn take(&self) -> Option<VideoFrame> {
        self.0.lock().unwrap().frame.take()
    }

    fn select(&self, ticket: Option<u64>) {
        let mut surface = self.0.lock().unwrap();
        surface.ticket = ticket;
        surface.picture = None;
        surface.frame = None;
        surface.wake();
    }
}

// Layout and parameter IDs from libmpv's mpv/render.h software render API.
#[repr(C)]
struct Param(c_int, *mut c_void);

type Wake = unsafe extern "C" fn(*mut c_void);
struct RenderContext {
    raw: Handle,
    update: unsafe extern "C" fn(Handle) -> u64,
    render: unsafe extern "C" fn(Handle, *mut Param) -> c_int,
    callback: unsafe extern "C" fn(Handle, Option<Wake>, *mut c_void),
    free: unsafe extern "C" fn(Handle),
    waker: Option<Box<Thread>>,
}
// The software context is moved once to its render thread, which owns all subsequent API calls.
unsafe impl Send for RenderContext {}

impl Drop for RenderContext {
    fn drop(&mut self) {
        unsafe {
            (self.callback)(self.raw, None, ptr::null_mut());
            (self.free)(self.raw);
        }
    }
}

unsafe extern "C" fn wake(data: *mut c_void) {
    unsafe {
        (&*data.cast::<Thread>()).unpark();
    }
}

struct Renderer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Renderer {
    fn start(lib: &Library, h: Handle, frames: &VideoFrames) -> Result<Self> {
        let mut context = unsafe {
            let create = lib
                .get::<unsafe extern "C" fn(*mut Handle, Handle, *mut Param) -> c_int>(
                    b"mpv_render_context_create",
                )?;
            let update = *lib.get(b"mpv_render_context_update")?;
            let render = *lib.get(b"mpv_render_context_render")?;
            let callback = *lib.get(b"mpv_render_context_set_update_callback")?;
            let free = *lib.get(b"mpv_render_context_free")?;
            let mut raw = ptr::null_mut();
            let mut params = [
                Param(1, c"sw".as_ptr().cast_mut().cast()),
                Param(0, ptr::null_mut()),
            ];
            ensure!(
                create(&mut raw, h, params.as_mut_ptr()) >= 0,
                "当前 libmpv 不支持内嵌视频"
            );
            RenderContext {
                raw,
                update,
                render,
                callback,
                free,
                waker: None,
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let target = frames.clone();
        let thread = thread::Builder::new()
            .name("mpv-inline-render".into())
            .spawn(move || {
                context.run(&target, &stopping);
            })?;
        frames.0.lock().unwrap().thread = Some(thread.thread().clone());
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl RenderContext {
    fn run(&mut self, frames: &VideoFrames, stopping: &AtomicBool) {
        let waker = self.waker.insert(Box::new(thread::current()));
        unsafe {
            (self.callback)(self.raw, Some(wake), ptr::from_mut(&mut **waker).cast());
        }
        let mut previous = None;
        while !stopping.load(Ordering::Acquire) {
            let changed = unsafe { (self.update)(self.raw) } & 1 != 0;
            let (ticket, mut size) = {
                let s = frames.0.lock().unwrap();
                (s.ticket, s.render_size())
            };
            if let Some(ticket) = ticket
                && (changed || previous != Some((ticket, size)))
            {
                let mut stride = size[0] as usize * 4;
                let mut pixels = vec![0u32; size[0] as usize * size[1] as usize];
                let mut block: c_int = 0;
                let mut params = [
                    Param(17, size.as_mut_ptr().cast()),
                    Param(18, c"rgb0".as_ptr().cast_mut().cast()),
                    Param(19, ptr::from_mut(&mut stride).cast()),
                    Param(20, pixels.as_mut_ptr().cast()),
                    Param(12, ptr::from_mut(&mut block).cast()),
                    Param(0, ptr::null_mut()),
                ];
                if unsafe { (self.render)(self.raw, params.as_mut_ptr()) } >= 0 {
                    let mut s = frames.0.lock().unwrap();
                    if s.ticket == Some(ticket) && s.render_size() == size {
                        s.frame = Some(VideoFrame {
                            ticket,
                            width: size[0] as u32,
                            height: size[1] as u32,
                            pixels,
                        });
                    }
                }
            }
            previous = ticket.map(|t| (t, size));
            thread::park();
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

pub(super) struct InlineOutput<'a> {
    lib: &'a Library,
    h: Handle,
    api: AudioApi,
    frames: VideoFrames,
    renderer: Option<Renderer>,
    wanted: Option<InlineVideo>,
    loading: bool,
    previous_options: Option<(String, String)>,
}

impl<'a> InlineOutput<'a> {
    pub fn new(lib: &'a Library, h: Handle, api: AudioApi, frames: VideoFrames) -> Self {
        Self {
            lib,
            h,
            api,
            frames,
            renderer: None,
            wanted: None,
            loading: false,
            previous_options: None,
        }
    }

    pub fn request(&mut self, request: InlineVideo) {
        self.close();
        self.wanted = Some(request);
    }

    // video-add runs on the audio file's timeline, so pause, seek, gain and the queue stay in sync.
    pub fn start_if_needed(&mut self) -> Result<()> {
        let Some(v) = &self.wanted else { return Ok(()) };
        if self.previous_options.is_some() {
            return Ok(());
        }
        if self.renderer.is_none() {
            self.renderer = Some(Renderer::start(self.lib, self.h, &self.frames)?);
        }
        self.previous_options = Some((
            self.api.text(self.h, "speed").unwrap_or_else(|| "1".into()),
            self.api.text(self.h, "referrer").unwrap_or_default(),
        ));
        self.api.command(self.h, &["set", "vid", "no"])?;
        self.api.command(self.h, &["set", "vo", "libmpv"])?;
        if let Some(page) = &v.referrer {
            self.api
                .command(self.h, &["set", "file-local-options/referrer", page])?;
        }
        let url = CString::new(v.url.as_str())?;
        let title = CString::new(format!("music-inline-{}", v.ticket))?;
        let args = [
            c"video-add".as_ptr(),
            url.as_ptr(),
            c"auto".as_ptr(),
            title.as_ptr(),
            ptr::null(),
        ];
        let submit = unsafe {
            self.lib
                .get::<unsafe extern "C" fn(Handle, u64, *const *const c_char) -> c_int>(
                    b"mpv_command_async",
                )?
        };
        ensure!(
            unsafe { submit(self.h, v.ticket, args.as_ptr()) } >= 0,
            "无法打开视频画面"
        );
        self.loading = true;
        Ok(())
    }

    fn track(&self, ticket: u64) -> Option<String> {
        let title = format!("music-inline-{ticket}");
        let n = self
            .api
            .text(self.h, "track-list/count")?
            .parse::<usize>()
            .ok()?;
        (0..n).find_map(|i| {
            (self
                .api
                .text(self.h, &format!("track-list/{i}/title"))
                .as_deref()
                == Some(&title)
                && self
                    .api
                    .text(self.h, &format!("track-list/{i}/type"))
                    .as_deref()
                    == Some("video")
                && self
                    .api
                    .text(self.h, &format!("track-list/{i}/external"))
                    .as_deref()
                    == Some("yes"))
            .then(|| self.api.text(self.h, &format!("track-list/{i}/id")))
            .flatten()
        })
    }

    pub fn complete(&mut self, ticket: u64, status: c_int) -> Result<()> {
        if ticket == 0 {
            return Ok(());
        }
        let track = self.track(ticket);
        let Some(v) = self.wanted.as_ref().filter(|v| v.ticket == ticket) else {
            if let Some(id) = track {
                let _ = self.api.command(self.h, &["video-remove", &id]);
            }
            return Ok(());
        };
        self.loading = false;
        ensure!(status >= 0, "视频轨道加载失败（{status}）");
        let id = track.ok_or_else(|| anyhow!("该来源未返回可播放的视频轨道"))?;
        self.api.command(self.h, &["set", "vid", &id])?;
        self.api
            .command(self.h, &["set", "speed", &v.speed.to_string()])?;
        self.frames.select(Some(ticket));
        self.update_picture_size();
        Ok(())
    }

    pub fn update_picture_size(&self) {
        if self.wanted.is_none() || self.loading {
            return;
        }
        let read = |name| self.api.text(self.h, name)?.parse::<i32>().ok();
        if let (Some(w), Some(h)) = (read("video-out-params/dw"), read("video-out-params/dh"))
            && w > 0
            && h > 0
        {
            let rotated = matches!(read("video-out-params/rotate"), Some(90 | 270));
            self.frames
                .set_picture_size(if rotated { [h, w] } else { [w, h] });
        }
    }

    pub fn speed(&mut self, speed: f64) {
        if let Some(v) = &mut self.wanted {
            v.speed = speed;
            if !self.loading && self.previous_options.is_some() {
                let _ = self
                    .api
                    .command(self.h, &["set", "speed", &speed.to_string()]);
            }
        }
    }

    pub fn ticket(&self) -> Option<u64> {
        self.wanted.as_ref().map(|v| v.ticket)
    }

    pub fn close(&mut self) {
        let Some(v) = self.wanted.take() else { return };
        self.frames.select(None);
        if self.loading {
            if let Ok(abort) = unsafe {
                self.lib
                    .get::<unsafe extern "C" fn(Handle, u64)>(b"mpv_abort_async_command")
            } {
                unsafe { abort(self.h, v.ticket) };
            }
            self.loading = false;
        }
        if let Some((speed, referrer)) = self.previous_options.take() {
            let _ = self.api.command(self.h, &["set", "vid", "no"]);
            if let Some(id) = self.track(v.ticket) {
                let _ = self.api.command(self.h, &["video-remove", &id]);
            }
            let _ = self.api.command(self.h, &["set", "vo", "gpu-next,gpu"]);
            let _ = self.api.command(self.h, &["set", "speed", &speed]);
            let _ = self
                .api
                .command(self.h, &["set", "file-local-options/referrer", &referrer]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_fits_the_physical_viewport_at_its_native_aspect_and_resolution() {
        let frames = VideoFrames::default();
        frames.select(Some(1));
        frames.resize(2400, 1400);
        frames.set_picture_size([1920, 1080]);
        assert_eq!(frames.0.lock().unwrap().render_size(), [1920, 1080]);
        frames.resize(800, 1200);
        frames.set_picture_size([1080, 1920]);
        assert_eq!(frames.0.lock().unwrap().render_size(), [675, 1200]);
    }
}
