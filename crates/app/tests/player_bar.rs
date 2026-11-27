mod support;
use music_app_ui::{AppWindow, FavoriteState, PlayerMode, Row};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{
    ComponentHandle, LogicalPosition, ModelRc, PhysicalSize, PlatformError, SharedString, Window,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use support::{click, test_clock};

fn chord(window: &Window, modifier: Option<Key>, key: impl Into<SharedString>, repeat: bool) {
    let text = key.into();
    if let Some(modifier) = modifier {
        window.dispatch_event(WindowEvent::KeyPressed {
            text: modifier.into(),
        });
    }
    window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    if repeat {
        window.dispatch_event(WindowEvent::KeyPressRepeated { text: text.clone() });
    }
    window.dispatch_event(WindowEvent::KeyReleased { text });
    if let Some(modifier) = modifier {
        window.dispatch_event(WindowEvent::KeyReleased {
            text: modifier.into(),
        });
    }
}

#[test]
fn player_stays_at_the_bottom_when_search_content_changes() -> anyhow::Result<()> {
    let _clock = test_clock()?;
    let ui = AppWindow::new()?;
    ui.show()?;
    let window = ui.window();
    ui.set_sources(ModelRc::from(
        ["本地音乐".into(), "24bit 无损音乐".into()].as_slice(),
    ));
    ui.set_source_index(1);
    ui.set_search_page(true);
    ui.set_anonymous_source(true);
    ui.set_can_filter(true);
    ui.set_online_sources(ModelRc::from(
        ["全部音乐源".into(), "24bit 无损音乐".into()].as_slice(),
    ));
    ui.set_heading("搜索结果 · 24bit 无损音乐".into());
    let result = Row {
        kind: "track".into(),
        title: "歌曲".into(),
        ..Default::default()
    };
    for (w, h, bar_height) in [(1240, 820, 110), (720, 480, 96), (800, 1000, 96)] {
        window.set_size(PhysicalSize::new(w, h));
        let check = |page| -> anyhow::Result<()> {
            let frame = window.take_snapshot()?;
            let pixel = |x, y| frame.as_slice()[(y * w + x) as usize];
            let sidebar = pixel(4, 4);
            let top = (4..h).find(|&y| pixel(4, y) != sidebar);
            assert_eq!(top, Some(h - bar_height), "{page} at {w}×{h}");
            let bar = pixel(4, h - bar_height);
            for x in [4, w - 5] {
                assert!(
                    pixel(x, h - 1) == bar,
                    "{page} leaves a gap below the player at {w}×{h}"
                );
            }
            Ok(())
        };
        ui.set_rows(ModelRc::default());
        ui.set_busy(true);
        check("loading search")?;
        ui.set_busy(false);
        check("empty search")?;
        ui.set_rows(ModelRc::from(std::slice::from_ref(&result)));
        check("one result")?;
        ui.set_show_lyrics(true);
        check("lyrics")?;
        ui.set_show_lyrics(false);
    }
    Ok(())
}

#[test]
fn shortcuts_clicks_and_tooltips_in_the_player_window() -> anyhow::Result<()> {
    let advance = test_clock()?;
    let ui = AppWindow::new()?;
    ui.show()?;
    let window = ui.window();
    window.set_size(PhysicalSize::new(720, 480));
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    let actions = Rc::new(RefCell::new(Vec::new()));
    let log = actions.clone();
    ui.on_action(move |action| log.borrow_mut().push(action.to_string()));
    for (modifier, key, action) in [
        (None, SharedString::from(Key::Space), "pause"),
        (Some(Key::Control), Key::LeftArrow.into(), "previous"),
        (Some(Key::Control), Key::RightArrow.into(), "next"),
        (Some(Key::Control), "S".into(), "shuffle"),
        (Some(Key::Control), "r".into(), "cycle-mode"),
        (None, Key::F7.into(), "pause"),
        (None, Key::F8.into(), "next"),
    ] {
        chord(window, modifier, key, true);
        assert_eq!(actions.take(), [action]);
    }

    let mut mode_icons = Vec::new();
    ui.set_current_favorite(FavoriteState::Liked);
    for (mode, name) in [
        (PlayerMode::Sequential, "顺序播放"),
        (PlayerMode::RepeatAll, "列表循环"),
        (PlayerMode::RepeatOne, "单曲循环"),
        (PlayerMode::Shuffle, "随机播放"),
        (PlayerMode::Heartbeat, "心动模式"),
    ] {
        ui.set_playback_mode(mode);
        assert_eq!(ui.get_playback_mode_name(), name);
        let frame = window.take_snapshot()?;
        let stride = frame.width() as usize;
        let background = frame.as_slice()[415 * stride + 228];
        let shape = |left| -> Vec<bool> {
            frame
                .as_slice()
                .chunks_exact(stride)
                .skip(406)
                .take(18)
                .flat_map(|row| {
                    row[left..left + 18]
                        .iter()
                        .map(|&pixel| pixel != background)
                })
                .collect()
        };
        let icon = shape(242);
        assert!(
            icon.iter().any(|&ink| ink) && !mode_icons.contains(&icon),
            "missing or duplicate icon for {name}"
        );
        mode_icons.push(icon);
        if mode == PlayerMode::Heartbeat {
            assert_ne!(
                shape(242),
                shape(424),
                "heartbeat and the favorite heart must have distinct shapes"
            );
        }
    }
    ui.set_playback_mode(PlayerMode::Sequential);
    click(window, 251., 415.);
    assert_eq!(actions.take(), ["cycle-mode"]);
    for state in [
        FavoriteState::NoTrack,
        FavoriteState::Unsupported,
        FavoriteState::Checking,
        FavoriteState::Adding,
        FavoriteState::Removing,
    ] {
        ui.set_current_favorite(state);
        click(window, 433., 415.);
        chord(window, Some(Key::Control), "d", false);
        assert!(actions.take().is_empty());
    }
    for state in [
        FavoriteState::Unliked,
        FavoriteState::Liked,
        FavoriteState::Retry,
    ] {
        ui.set_current_favorite(state);
        click(window, 433., 415.);
        chord(window, Some(Key::Control), "d", true);
        assert_eq!(actions.take(), ["favorite-current", "favorite-current"]);
    }
    ui.set_current_favorite(FavoriteState::Unliked);

    ui.set_show_downloads(true);
    chord(window, Some(Key::Control), "p", true);
    assert!(ui.get_show_queue() && !ui.get_show_downloads());
    window.take_snapshot()?;
    click(window, 477., 415.);
    assert!(
        !ui.get_show_queue(),
        "the queue icon must close the same panel"
    );
    chord(window, Some(Key::Control), "l", true);
    assert!(ui.get_show_lyrics() && !ui.get_video_page());
    click(window, 590., 431.);
    assert!(!ui.get_show_lyrics());
    ui.set_current_has_video(true);
    click(window, 590., 431.);
    assert!(ui.get_video_page());
    ui.set_show_settings(true);
    chord(window, Some(Key::Control), "l", true);
    assert!(ui.get_video_page() && !ui.get_show_settings());
    chord(window, Some(Key::Control), "l", true);
    assert!(!ui.get_video_page() && !ui.get_show_lyrics());
    ui.set_current_has_video(false);

    chord(window, Some(Key::Control), "m", true);
    assert_eq!(ui.get_volume_level(), 0.);
    chord(window, Some(Key::Control), "m", false);
    assert_eq!(ui.get_volume_level(), 70.);
    ui.invoke_set_player_volume(97.);
    chord(window, Some(Key::Control), Key::UpArrow, true);
    assert_eq!(ui.get_volume_level(), 100.);
    click(window, 630., 431.);
    assert_eq!(ui.get_volume_level(), 0.);
    chord(window, Some(Key::Control), "m", false);
    assert_eq!(ui.get_volume_level(), 100.);
    chord(window, Some(Key::Control), Key::DownArrow, true);
    assert_eq!(ui.get_volume_level(), 80.);

    let sought = Rc::new(Cell::new(-1.));
    let last_seek = sought.clone();
    ui.on_seek(move |position| last_seek.set(position));
    chord(window, Some(Key::Shift), Key::RightArrow, false);
    assert_eq!(sought.get(), -1.);
    ui.set_duration(60.);
    ui.set_position(58.);
    chord(window, Some(Key::Shift), Key::RightArrow, false);
    assert_eq!(sought.get(), 60.);
    ui.set_position(2.);
    chord(window, Some(Key::Shift), Key::LeftArrow, false);
    assert_eq!(sought.get(), 0.);
    let position = LogicalPosition::new(300., 456.);
    let button = PointerEventButton::Left;
    window.dispatch_event(WindowEvent::PointerPressed { position, button });
    assert!(ui.get_scrubbing());
    window.dispatch_event(WindowEvent::PointerReleased { position, button });
    assert!(!ui.get_scrubbing());
    assert!(
        (5.0..55.0).contains(&sought.get()),
        "the seek slider must accept clicks"
    );
    let track_pixel =
        || -> Result<_, PlatformError> { Ok(window.take_snapshot()?.as_slice()[456 * 720 + 350]) };
    ui.set_position(0.);
    let empty = track_pixel()?;
    ui.set_position(60.);
    assert_ne!(
        empty,
        track_pixel()?,
        "slider must keep following playback after a seek"
    );
    ui.set_position(0.);
    ui.set_duration(0.);

    for dark in [false, true] {
        ui.set_dark(dark);
        for (w, h, bar_height) in [(1240, 820, 110), (720, 480, 96)] {
            window.set_size(PhysicalSize::new(w, h));
            let frame = window.take_snapshot()?;
            let pixels = frame.as_slice();
            for x in [4, w - 5] {
                let y = h - bar_height / 2;
                let at = |y| pixels[(y * w + x) as usize];
                assert!(
                    at(y) == at(y - 1) && at(y) == at(y + 1),
                    "divider crosses the player bar"
                );
            }
        }
        // Real hit targets in the smallest supported layout, including both sliders.
        for (name, x, y) in [
            ("mode", 251., 415.),
            ("previous", 295., 415.),
            ("play", 342., 415.),
            ("next", 389., 415.),
            ("favorite", 433., 415.),
            ("queue", 477., 415.),
            ("lyrics", 590., 431.),
            ("mute", 630., 431.),
            ("volume", 695., 431.),
            ("seek", 300., 456.),
        ] {
            let position = LogicalPosition::new(x, y);
            window.dispatch_event(WindowEvent::PointerMoved { position });
            advance(200);
            let before = window.take_snapshot()?;
            advance(600);
            let hover = window.take_snapshot()?;
            assert!(
                before.as_bytes() != hover.as_bytes(),
                "missing tooltip on {name}"
            );
            if name == "mode"
                && !dark
                && let Some(path) = std::env::var_os("MUSIC_API_CAPTURE")
            {
                let bar_height = 96;
                let offset = ((hover.height() - bar_height) * hover.width() * 4) as usize;
                image::save_buffer(
                    path,
                    &hover.as_bytes()[offset..],
                    hover.width(),
                    bar_height,
                    image::ColorType::Rgba8,
                )?;
            }
            window.dispatch_event(WindowEvent::PointerExited);
            advance(600);
            // Re-entering starts a fresh delay and restores the original hover appearance.
            window.dispatch_event(WindowEvent::PointerMoved { position });
            advance(200);
            assert!(
                before.as_bytes() == window.take_snapshot()?.as_bytes(),
                "tooltip still visible on {name}"
            );
            window.dispatch_event(WindowEvent::PointerExited);
            advance(600);
        }
    }

    let query = Rc::new(RefCell::new(String::new()));
    let submitted = query.clone();
    ui.on_search(move |text, _| *submitted.borrow_mut() = text.to_string());
    click(window, 420., 32.);
    for key in ["a", " ", "b"] {
        chord(window, None, key, false);
    }
    chord(window, Some(Key::Control), Key::RightArrow, false);
    chord(window, Some(Key::Control), "p", false);
    chord(window, Some(Key::Control), "d", false);
    chord(window, None, Key::Return, false);
    assert_eq!(query.borrow().as_str(), "a b");
    assert!(!ui.get_show_queue() && actions.borrow().is_empty());

    click(window, 389., 415.);
    assert_eq!(actions.take(), ["next"]);
    chord(window, None, Key::Space, false);
    assert_eq!(
        actions.take(),
        ["pause"],
        "clicking the player must leave text editing"
    );
    click(window, 696., 431.);
    chord(window, Some(Key::Control), Key::RightArrow, false);
    assert_eq!(
        actions.take(),
        ["next"],
        "a focused volume slider must allow track shortcuts"
    );
    ui.set_show_login(true);
    chord(window, None, Key::Space, false);
    ui.set_show_login(false);
    ui.set_show_catalog(true);
    chord(window, Some(Key::Control), Key::RightArrow, false);
    assert!(actions.borrow().is_empty());
    Ok(())
}
