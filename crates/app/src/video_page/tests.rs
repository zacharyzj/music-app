use super::*;
use anyhow::{bail, ensure};
use std::io::Write;

fn track(source: &str) -> Item {
    Item {
        kind: "track".into(),
        key: ResourceKey {
            source: source.into(),
            id: "same".into(),
        },
        title: "视频测试".into(),
        ..Default::default()
    }
}

fn state() -> Result<UiState> {
    let plugin = serde_json::from_value(
        json!({"id":"video-source","name":"视频源","version":"1","abi":2,
        "capabilities":["video"],"network":[],"media":["media.example"]}),
    )?;
    Ok(UiState::new(
        Queue::default(),
        Default::default(),
        vec![(plugin, true)],
    ))
}

fn settings() -> PlaybackSettings {
    PlaybackSettings {
        volume: 30.,
        muted: false,
        video_speed: 1.,
        video_height: 720,
    }
}

fn reply(job: Job, value: std::result::Result<Value, String>) -> Reply {
    Reply {
        source: job.source,
        op: job.request.op,
        args: job.request.args,
        value,
        generation: job.generation,
    }
}

#[test]
fn the_playing_track_routes_the_page_and_late_video_results_are_ignored() -> Result<()> {
    let _clock = crate::test_ui::test_clock()?;
    let ui = AppWindow::new()?;
    let mut s = state()?;
    let (jobs, rx) = mpsc::channel();
    let (commands, engine) = mpsc::channel();
    let video = track("video-source");
    s.queue.items.push(video.clone());
    let saved = json!(s.queue);
    ui.set_show_lyrics(true);
    play(
        &ui,
        &mut s,
        &jobs,
        &commands,
        video.clone(),
        PlayKind::Audio,
    );
    assert!(ui.get_video_page());
    assert_eq!(s.source, "local");
    assert!(matches!(engine.try_recv()?, Command::Stop));
    let normal = rx.try_recv()?;
    assert_eq!(normal.request.op, "resolve");
    assert!(normal.request.args["video"].is_null());
    s.stream_started(&ui, video.key.clone());
    s.playing = false;
    ui.set_playing(false);
    s.sync_video_page(&ui, &jobs, &commands);
    let inline = rx
        .try_iter()
        .find(|j| j.request.args["inline"].is_u64())
        .unwrap();
    assert_eq!(inline.source, video.key.source);
    assert_eq!(inline.request.args["id"], "same");
    assert_eq!(inline.request.args["video"], true);
    ui.set_show_lyrics(false);
    s.sync_video_page(&ui, &jobs, &commands);
    assert!(matches!(engine.try_recv()?, Command::HideInline));
    ui.set_show_lyrics(true);
    s.sync_video_page(&ui, &jobs, &commands);
    s.finish_video_page(
        &ui,
        reply(inline, Err("stale".into())),
        &commands,
        settings(),
    );
    assert!(ui.get_video_error().is_empty() && engine.try_recv().is_err());
    let inline = rx.try_recv()?;
    s.finish_video_page(
        &ui,
        reply(
            inline,
            Ok(
                json!({"url":"auto","referrer":"https://media.example/","qualities":[
        {"height":480,"url":"sd"},{"height":720,"url":"hd"},{"height":1080,"url":"fullhd"}]}),
            ),
        ),
        &commands,
        settings(),
    );
    let Command::ShowInline(v) = engine.try_recv()? else {
        bail!("expected an inline video track")
    };
    assert_eq!(v.url, "hd");
    assert_eq!(
        ui.get_video_qualities().iter().collect::<Vec<_>>(),
        ["自动", "1080p", "720p", "480p"]
    );
    assert!(!s.playing);
    assert_eq!(json!(s.queue), saved);
    s.queue.index = Some(0);
    s.playback_ended(&ui, &jobs, &commands);
    assert!(ui.get_video_page() && ui.get_video_ended());
    play(
        &ui,
        &mut s,
        &jobs,
        &commands,
        track("local"),
        PlayKind::Audio,
    );
    assert!(ui.get_show_lyrics() && !ui.get_video_page() && !ui.get_video_ended());
    Ok(())
}

fn media(dir: &Path) -> Result<(String, String)> {
    let wav = dir.join("silent.wav");
    let audio_bytes = 8000u32 * 2 * 10;
    let mut audio = std::fs::File::create(&wav)?;
    audio.write_all(b"RIFF")?;
    audio.write_all(&(36 + audio_bytes).to_le_bytes())?;
    audio.write_all(b"WAVEfmt ")?;
    for chunk in [
        &16u32.to_le_bytes()[..],
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &8000u32.to_le_bytes(),
        &16000u32.to_le_bytes(),
        &2u16.to_le_bytes(),
        &16u16.to_le_bytes(),
    ] {
        audio.write_all(chunk)?;
    }
    audio.write_all(b"data")?;
    audio.write_all(&audio_bytes.to_le_bytes())?;
    audio.write_all(&vec![0; audio_bytes as usize])?;
    let y4m = dir.join("picture.y4m");
    let mut video = std::fs::File::create(&y4m)?;
    video.write_all(b"YUV4MPEG2 W64 H36 F25:1 Ip A1:1 C420jpeg\n")?;
    for i in 0..250 {
        video.write_all(b"FRAME\n")?;
        video.write_all(&[65 + (i % 100) as u8; 64 * 36])?;
        video.write_all(&[96; 32 * 18])?;
        video.write_all(&[180; 32 * 18])?;
    }
    Ok((
        wav.to_string_lossy().into_owned(),
        y4m.to_string_lossy().into_owned(),
    ))
}

#[test]
#[ignore = "requires an installed libmpv supplied in MUSIC_API_MPV"]
fn inline_frames_reach_slint_and_follow_the_original_audio_clock() -> Result<()> {
    let advance = crate::test_ui::test_clock()?;
    let ui = AppWindow::new()?;
    ui.show()?;
    ui.window().set_size(slint::PhysicalSize::new(1000, 680));
    let dir = tempfile::tempdir()?;
    let (audio, video) = media(dir.path())?;
    let player = Rc::new(Player::start(Some(
        std::env::var_os("MUSIC_API_MPV")
            .as_deref()
            .map(Path::new)
            .expect("set MUSIC_API_MPV"),
    )));
    let s = Rc::new(RefCell::new(state()?));
    let prefs = Rc::new(Cell::new(settings()));
    install(&ui, s.clone(), player.clone(), prefs.clone());
    let pc = &player.commands;
    pc.send(Command::Volume(0.))?;
    pc.send(Command::Open {
        generation: 0,
        start: Default::default(),
        url: audio,
        referrer: None,
        video: None,
        gains: Default::default(),
    })?;
    let wait = |predicate: &dyn Fn(f64, bool) -> bool| -> Result<()> {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            match player.events.recv_timeout(Duration::from_millis(500)) {
                Ok(PlayerEvent::Position(c))
                    if c.duration > 9. && predicate(c.start.position, c.start.paused) =>
                {
                    return Ok(());
                }
                Ok(
                    PlayerEvent::Error(e)
                    | PlayerEvent::InlineError(_, e)
                    | PlayerEvent::TrackFailed(_, e),
                ) => bail!("{e}"),
                _ => {}
            }
        }
        bail!("timed out waiting for the audio clock")
    };
    wait(&|p, _| p > 0.)?;
    pc.send(Command::Pause(true))?;
    pc.send(Command::Seek(2.))?;
    wait(&|p, paused| paused && (p - 2.).abs() < 0.12)?;
    let key = track("video-source").key;
    s.borrow_mut().loaded = Some(key);
    ui.set_current_has_video(true);
    ui.set_show_lyrics(true);
    let (jobs, rx) = mpsc::channel();
    s.borrow_mut().sync_video_page(&ui, &jobs, pc);
    let requested = rx.recv_timeout(Duration::from_secs(1))?;
    s.borrow_mut().finish_video_page(
        &ui,
        reply(
            requested,
            Ok(json!({"url":video,"referrer":"https://media.example/",
        "qualities":[{"height":720,"url":video}]})),
        ),
        pc,
        prefs.get(),
    );
    let until = Instant::now() + Duration::from_secs(10);
    let get_pixel = || -> Result<[u8; 3]> {
        loop {
            ui.invoke_video_tick(320, 180);
            if let Some(image) = ui.get_video_frame().to_rgb8()
                && (image.width(), image.height()) == (64, 36)
            {
                let pixel = image.as_slice()
                    [(image.height() / 2 * image.width() + image.width() / 2) as usize];
                if pixel.r > pixel.b.saturating_add(20) {
                    return Ok([pixel.r, pixel.g, pixel.b]);
                }
            }
            for e in player.events.try_iter() {
                if let PlayerEvent::InlineError(_, err)
                | PlayerEvent::Error(err)
                | PlayerEvent::TrackFailed(_, err) = e
                {
                    bail!("{err}");
                }
            }
            ensure!(
                Instant::now() < until,
                "no decoded video frame reached the Slint page"
            );
            std::thread::sleep(Duration::from_millis(16));
        }
    };
    let paused_pixel = get_pixel()?;
    wait(&|p, paused| paused && (p - 2.).abs() < 0.12)?;
    // Seek while paused and check the displayed frame changes with the same audio clock.
    pc.send(Command::Seek(4.))?;
    wait(&|p, paused| paused && (p - 4.).abs() < 0.12)?;
    for _ in 0..40 {
        if get_pixel()? != paused_pixel {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_ne!(get_pixel()?, paused_pixel);
    ui.invoke_video_option("speed".into(), 3);
    assert_eq!(prefs.get().video_speed, 1.25);
    ui.invoke_video_option("quality".into(), 0);
    assert_eq!(prefs.get().video_height, 0);
    ui.invoke_video_option("quality".into(), 1);
    assert_eq!(prefs.get().video_height, 720);
    pc.send(Command::Pause(false))?;
    wait(&|p, paused| !paused && p > 4.2)?;
    advance(30);
    assert_eq!(ui.get_video_quality_index(), 1);
    let shot = ui.window().take_snapshot()?;
    assert!(
        shot.as_slice()
            .iter()
            .filter(|v| v.r > v.b.saturating_add(30))
            .count()
            > 3000
    );
    if let Some(path) = std::env::var_os("MUSIC_VIDEO_PAGE_CAPTURE") {
        image::save_buffer(
            path,
            shot.as_bytes(),
            shot.width(),
            shot.height(),
            image::ColorType::Rgba8,
        )?;
    }
    ui.set_show_lyrics(false);
    s.borrow_mut().sync_video_page(&ui, &jobs, pc);
    assert_eq!(ui.get_video_frame().size().width, 0);
    wait(&|p, paused| !paused && p > 4.7)?;
    // Exit may follow a seek and pause before the next UI timer tick.
    pc.send(Command::Pause(true))?;
    pc.send(Command::Seek(6.375))?;
    let checkpoint = player.clock().expect("playback clock at exit");
    assert!(
        checkpoint.start.paused && (checkpoint.start.position - 6.375).abs() < 0.1,
        "{checkpoint:?}"
    );
    pc.send(Command::Quit)?;
    Ok(())
}
