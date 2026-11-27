use super::*;

#[cfg(all(test, feature = "ui-smoke"))]
mod tests;
use music_player::{InlineVideo, VideoControl, VideoQuality};
use slint::{Rgb8Pixel, SharedPixelBuffer};

const SPEEDS: [f64; 7] = [0.5, 0.75, 1., 1.25, 1.5, 1.75, 2.];

struct Stream {
    url: String,
    referrer: Option<String>,
    qualities: Vec<VideoQuality>,
}

#[derive(Default)]
pub(super) struct VideoPage {
    generation: Option<u64>,
    ticket: u64,
    stream: Option<Stream>,
}

impl VideoPage {
    pub fn error(&self, ui: &AppWindow, ticket: u64, message: String) {
        if self.generation.is_some() && self.ticket == ticket && ui.get_video_page() {
            ui.set_video_error(format!("视频暂时无法播放 · {message}").into());
        }
    }

    fn attach(&mut self, ui: &AppWindow, player: &mpsc::Sender<Command>, prefs: PlaybackSettings) {
        let Some(stream) = &self.stream else { return };
        let choice = VideoQuality::preferred(&stream.qualities, prefs.video_height);
        let index = choice
            .and_then(|q| stream.qualities.iter().position(|v| v.height == q.height))
            .map_or(0, |i| i as i32 + 1);
        ui.set_video_quality_index(index);
        ui.set_video_speed_index(
            SPEEDS
                .iter()
                .position(|v| *v == prefs.video_speed)
                .unwrap_or(2) as i32,
        );
        ui.set_video_error("".into());
        self.ticket += 1;
        let video = InlineVideo {
            ticket: self.ticket,
            url: choice.map_or(&stream.url, |q| &q.url).clone(),
            referrer: stream.referrer.clone(),
            speed: prefs.video_speed,
        };
        if player.send(Command::ShowInline(video)).is_err() {
            self.error(ui, self.ticket, "播放引擎已关闭".into());
        }
    }
}

impl UiState {
    pub(super) fn sync_video_page(
        &mut self,
        ui: &AppWindow,
        jobs: &mpsc::Sender<Job>,
        player: &mpsc::Sender<Command>,
    ) {
        let target = self.loaded.as_ref().filter(|_| ui.get_video_page());
        let generation = target.map(|_| self.play_generation);
        let page = &mut self.video_page;
        if generation == page.generation {
            return;
        }
        if page.generation.is_some() {
            let _ = player.send(Command::HideInline);
        }
        page.generation = generation;
        page.ticket += 1;
        page.stream = None;
        ui.set_video_error("".into());
        ui.set_video_frame(slint::Image::default());
        ui.set_video_qualities(["自动".into()].into());
        ui.set_video_quality_index(0);
        if let Some(key) = target
            && jobs
                .send(Job {
                    source: key.source.clone(),
                    generation: self.play_generation,
                    request: Request::new(
                        "resolve",
                        json!({"id":key.id,"video":true,"inline":page.ticket}),
                    ),
                })
                .is_err()
        {
            page.error(ui, page.ticket, "音乐源服务不可用".into());
        }
    }

    pub(super) fn finish_video_page(
        &mut self,
        ui: &AppWindow,
        reply: Reply,
        player: &mpsc::Sender<Command>,
        prefs: PlaybackSettings,
    ) {
        let page = &mut self.video_page;
        if !ui.get_video_page()
            || page.generation != Some(self.play_generation)
            || reply.generation != self.play_generation
            || reply.args["inline"].as_u64() != Some(page.ticket)
            || !self
                .loaded
                .as_ref()
                .is_some_and(|key| key.source == reply.source && reply.args["id"] == key.id)
        {
            return;
        }
        let result = reply.value.and_then(|v| {
            let url = v["url"].as_str().ok_or("音乐源未提供视频地址")?.to_owned();
            let mut qualities: Vec<VideoQuality> =
                serde_json::from_value(v["qualities"].clone()).unwrap_or_default();
            qualities.sort_by_key(|q| std::cmp::Reverse(q.height));
            qualities.dedup_by_key(|q| q.height);
            Ok(Stream {
                url,
                referrer: v["referrer"].as_str().map(str::to_owned),
                qualities,
            })
        });
        match result {
            Ok(stream) => {
                ui.set_video_qualities(playlist_import::labels(
                    std::iter::once("自动".to_owned())
                        .chain(stream.qualities.iter().map(|q| format!("{}p", q.height))),
                ));
                page.stream = Some(stream);
                page.attach(ui, player, prefs);
            }
            Err(message) => page.error(ui, page.ticket, message),
        }
    }
}

pub(super) fn install(
    ui: &AppWindow,
    state: Rc<RefCell<UiState>>,
    player: Rc<Player>,
    settings: Rc<Cell<PlaybackSettings>>,
) {
    let window = ui.as_weak();
    let frames = player.frames.clone();
    let state_for_frames = state.clone();
    ui.on_video_tick(move |w, h| {
        let Some(ui) = window.upgrade() else { return };
        let scale = ui.window().scale_factor();
        let [w, h] = [w, h].map(|n| (n as f32 * scale).round() as i32);
        frames.resize(w, h);
        let Some(frame) = frames.take() else { return };
        let s = state_for_frames.borrow();
        if !ui.get_video_page()
            || s.video_page.generation != Some(s.play_generation)
            || frame.ticket != s.video_page.ticket
        {
            return;
        }
        let mut image = SharedPixelBuffer::<Rgb8Pixel>::new(frame.width, frame.height);
        for (dst, src) in image.make_mut_slice().iter_mut().zip(frame.pixels) {
            let [r, g, b, _] = src.to_ne_bytes();
            *dst = Rgb8Pixel::new(r, g, b);
        }
        ui.set_video_frame(slint::Image::from_rgb8(image));
    });
    let window = ui.as_weak();
    ui.on_video_option(move |option, index| {
        let Some(ui) = window.upgrade() else { return };
        let mut s = state.borrow_mut();
        if !ui.get_video_page() || s.video_page.generation != Some(s.play_generation) {
            return;
        }
        let page = &mut s.video_page;
        match option.as_str() {
            "speed" => {
                if let Some(&speed) = SPEEDS.get(index as usize) {
                    PlaybackSettings::apply_video_control(
                        &settings,
                        &ui,
                        VideoControl::Speed(speed),
                    );
                    ui.set_video_speed_index(index);
                    let _ = player.commands.send(Command::InlineSpeed(speed));
                }
            }
            "quality" => {
                if let Some(stream) = &page.stream {
                    let height = if index == 0 {
                        0
                    } else if let Some(q) = stream.qualities.get(index.saturating_sub(1) as usize) {
                        q.height
                    } else {
                        return;
                    };
                    PlaybackSettings::apply_video_control(
                        &settings,
                        &ui,
                        VideoControl::Height(height),
                    );
                    page.attach(&ui, &player.commands, settings.get());
                }
            }
            "retry" => {
                let _ = player.commands.send(Command::HideInline);
                page.generation = None;
            }
            _ => {}
        }
    });
}
