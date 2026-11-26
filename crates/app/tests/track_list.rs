mod support;
use music_app_ui::{AppWindow, Row};
use slint::platform::WindowEvent;
use slint::{ComponentHandle, LogicalPosition, Model, PhysicalSize, VecModel};
use std::{cell::RefCell, rc::Rc};
use support::{click, test_clock};

#[test]
fn scrolled_track_actions_and_tooltips() -> anyhow::Result<()> {
    let advance = test_clock()?;
    let ui = AppWindow::new()?;
    ui.show()?;
    let window = ui.window();
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    ui.set_can_download(true);
    ui.set_page_account_actions(true);
    let tracks = Rc::new(VecModel::from(
        (1..=100)
            .map(|i| Row {
                resource: format!("source:{i}").into(),
                kind: "track".into(),
                title: format!("歌曲 {i}").into(),
                account_actions: true,
                show_download: true,
                download_label: "下载".into(),
                download_hint: "下载到本地音乐库".into(),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    ));
    ui.set_rows(tracks.clone().into());
    let events = Rc::new(RefCell::new(Vec::new()));
    let log = events.clone();
    ui.on_row_action(move |i, action| log.borrow_mut().push((i, action.to_string())));

    for (w, h, y, buttons, gap) in [
        (1240, 820, 210., [951., 1001., 1069., 1138., 1188.], 1027.),
        (720, 480, 196., [505., 541., 594., 650., 681.], 554.),
    ] {
        window.set_size(PhysicalSize::new(w, h));
        window.take_snapshot()?;
        let viewport = LogicalPosition::new((w - 330) as f32, 242.);
        for delta_y in [10000., -4970.] {
            window.dispatch_event(WindowEvent::PointerScrolled {
                position: viewport,
                delta_x: 0.,
                delta_y,
            });
            advance(400);
        }
        window.take_snapshot()?;
        let index = 71;
        let mut row = tracks.row_data(index).unwrap();
        row.liked = false;
        row.download_blocked = false;
        tracks.set_row_data(index, row.clone());
        ui.set_local_playlist(false);
        advance(400);
        window.take_snapshot()?;
        let check = |x, expected: Option<&str>| {
            click(window, x, y);
            // ScrollView defers pointer dispatch while it distinguishes a click from a drag.
            advance(160);
            let expected: Vec<_> = expected
                .into_iter()
                .map(|name| (index as i32, name.to_owned()))
                .collect();
            assert_eq!(events.take(), expected, "click at {x}, {y} in width {w}");
        };
        let title_x = if w == 1240 { 370. } else { 310. };
        check(title_x, None);
        click(window, title_x, y + 70.);
        advance(160);
        assert!(
            events.take().is_empty(),
            "clicks on different tracks must not start playback"
        );
        check(title_x, None);
        check(title_x, Some("open"));
        for (x, action) in buttons.into_iter().zip([
            Some("open"),
            Some("add"),
            Some("download"),
            Some("like"),
            None,
        ]) {
            check(x, action);
        }
        check(gap, None);
        row.liked = true;
        row.download_blocked = true;
        row.download_hint = "音乐源未开放这首歌的下载".into();
        tracks.set_row_data(index, row);
        window.take_snapshot()?;
        check(buttons[2], None);
        check(buttons[3], Some("unlike"));
        check(buttons[4], Some("unlike"));
        ui.set_local_playlist(true);
        window.take_snapshot()?;
        check(buttons[4], Some("remove"));
        let neighbor = tracks.row_data(index + 1).unwrap();
        let mut album = neighbor.clone();
        album.kind = "album".into();
        tracks.set_row_data(index + 1, album);
        window.take_snapshot()?;
        click(window, title_x, y + 70.);
        advance(160);
        assert_eq!(events.take(), [(index as i32 + 1, "open".to_owned())]);
        tracks.set_row_data(index + 1, neighbor);

        for x in buttons {
            window.dispatch_event(WindowEvent::PointerExited);
            advance(600);
            let position = LogicalPosition::new(x, y);
            window.dispatch_event(WindowEvent::PointerMoved { position });
            advance(200);
            let before = window.take_snapshot()?;
            advance(600);
            let hover = window.take_snapshot()?;
            assert!(
                before.as_bytes() != hover.as_bytes(),
                "missing tooltip at {x} in width {w}"
            );
            if w == 1240
                && x == buttons[1]
                && let Some(path) = std::env::var_os("MUSIC_ROW_HOVER_CAPTURE")
            {
                image::save_buffer(
                    path,
                    hover.as_bytes(),
                    hover.width(),
                    hover.height(),
                    image::ColorType::Rgba8,
                )?;
            }
            window.dispatch_event(WindowEvent::PointerExited);
            advance(600);
            window.dispatch_event(WindowEvent::PointerMoved { position });
            advance(200);
            assert!(
                before.as_bytes() == window.take_snapshot()?.as_bytes(),
                "tooltip still visible at {x} in width {w}"
            );
        }
        let mut video = tracks.row_data(index).unwrap();
        video.show_video = true;
        tracks.set_row_data(index, video.clone());
        ui.set_page_has_video(true);
        window.take_snapshot()?;
        check(buttons[0], Some("video"));
        video.disabled = true;
        tracks.set_row_data(index, video.clone());
        window.take_snapshot()?;
        check(buttons[0], None);
        video.disabled = false;
        video.show_video = false;
        tracks.set_row_data(index, video);
        ui.set_page_has_video(false);
    }
    window.dispatch_event(WindowEvent::PointerExited);
    advance(800);
    // The feedback stays above the player and gives each new action its full display time.
    let notice_area = || -> anyhow::Result<Vec<_>> {
        let frame = window.take_snapshot()?;
        Ok(frame
            .as_slice()
            .chunks_exact(frame.width() as usize)
            .skip(316)
            .take(60)
            .flat_map(|line| line[160..560].iter().copied())
            .collect())
    };
    let resting = notice_area()?;
    ui.invoke_notify_row("已加入播放队列".into());
    assert!(
        resting != notice_area()?,
        "no visible feedback after adding a track"
    );
    advance(3000);
    ui.invoke_notify_row("已取消喜欢".into());
    advance(1800);
    assert!(
        resting != notice_area()?,
        "the second action's feedback expired too soon"
    );
    advance(2300);
    assert!(resting == notice_area()?, "feedback should dismiss itself");
    ui.set_queue(tracks.clone().into());
    ui.set_show_queue(true);
    let logged = events.clone();
    ui.on_queue_action(move |i, action| logged.borrow_mut().push((i, action.to_string())));
    window.take_snapshot()?;
    for (x, action) in [
        (535., None),
        (535., Some("play")),
        (634., Some("play")),
        (673., Some("remove")),
    ] {
        click(window, x, 130.);
        advance(160);
        let expected: Vec<_> = action.into_iter().map(|a| (0, a.to_owned())).collect();
        assert_eq!(events.take(), expected, "queue click at {x}");
    }
    let mut video = tracks.row_data(0).unwrap();
    video.show_video = true;
    tracks.set_row_data(0, video);
    window.take_snapshot()?;
    click(window, 634., 130.);
    advance(160);
    assert_eq!(events.take(), [(0, "video".into())]);
    Ok(())
}
