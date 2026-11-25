//! Scale native cross-monitor drags without changing the window's aspect ratio.

mod drag;

use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use std::{cell::Cell, rc::Rc};
use winit::platform::windows::{Color, WindowExtWindows};

type AppliedWindow = (winit::window::WindowId, bool, winit::dpi::PhysicalSize<u32>);

fn sync_native_window(ui: &crate::AppWindow, applied: &Cell<Option<AppliedWindow>>) {
    ui.window().with_winit_window(|native| {
        let scale = ui.window().scale_factor();
        let minimum = winit::dpi::PhysicalSize::new(
            (ui.get_minimum_window_width() * scale).ceil() as u32,
            (ui.get_minimum_window_height() * scale).ceil() as u32,
        );
        let state = (native.id(), ui.get_dark(), minimum);
        if applied.get() == Some(state) {
            return;
        }
        applied.set(Some(state));
        // Store physical constraints using the current content DPI. Winit may
        // still report the window-centre monitor while the cursor has crossed.
        native.set_min_inner_size(Some(minimum));
        let theme = ui.global::<crate::Theme>();
        let color = |c: slint::Color| Color::from_rgb(c.red(), c.green(), c.blue());
        native.set_theme(Some(if ui.get_dark() {
            winit::window::Theme::Dark
        } else {
            winit::window::Theme::Light
        }));
        native.set_title_background_color(Some(color(theme.get_bg())));
        native.set_title_text_color(color(theme.get_ink()));
        native.set_border_color(Some(color(theme.get_elevated())));
    });
}

pub(super) fn install(ui: &crate::AppWindow) {
    install_with_state(ui, Rc::new(drag::DragState::default()));
}

fn install_with_state(ui: &crate::AppWindow, drag: Rc<drag::DragState>) {
    let applied = Rc::new(Cell::new(None));
    let weak = ui.as_weak();
    let theme_state = applied.clone();
    ui.on_native_theme_changed(move || {
        if let Some(ui) = weak.upgrade() {
            theme_state.set(None);
            sync_native_window(&ui, &theme_state);
        }
    });
    let weak = ui.as_weak();
    ui.window().on_winit_window_event(move |window, event| {
        if let Some(ui) = weak.upgrade() {
            drag.minimum.set((
                ui.get_minimum_window_width().ceil() as i32,
                ui.get_minimum_window_height().ceil() as i32,
            ));
            window.with_winit_window(|native| drag.attach(native));
            if let Some(dpi) = drag.dpi.get() {
                let scale_factor = dpi as f32 / 96.0;
                if (window.scale_factor() - scale_factor).abs() > f32::EPSILON {
                    // Update layout and input coordinates together, even when
                    // two equal-resolution monitors only differ in scaling.
                    let _ = window.dispatch_event_with_result(
                        slint::platform::WindowEvent::ScaleFactorChanged { scale_factor },
                    );
                    window.with_winit_window(|native| {
                        let size = native.inner_size();
                        let _ = window.dispatch_event_with_result(
                            slint::platform::WindowEvent::Resized {
                                size: slint::PhysicalSize::new(size.width, size.height)
                                    .to_logical(scale_factor),
                            },
                        );
                    });
                }
            }
            sync_native_window(&ui, &applied);
        }
        if let winit::event::WindowEvent::Resized(size) = event {
            // winit can deliver the pre-correction resize AFTER the resize requested
            // by InnerSizeWriter. Applying it would desync Slint's cached size and
            // make the next DPI change reuse that stale size (winit 0.30 on Windows).
            if window
                .with_winit_window(|native| native.inner_size() != *size)
                .unwrap_or(false)
            {
                return EventResult::PreventDefault;
            }
        }
        let winit::event::WindowEvent::ScaleFactorChanged {
            inner_size_writer, ..
        } = event
        else {
            return EventResult::Propagate;
        };

        if drag.dpi.get().is_none() {
            return EventResult::Propagate;
        }
        if drag.dpi_during_drag.get() {
            window.with_winit_window(|native| {
                // A late, buffered DPI event must not resize again after mouse-up.
                let _ = inner_size_writer
                    .clone()
                    .request_inner_size(native.inner_size());
            });
        }
        // The native hook's latest DPI wins over queued/older notifications and
        // Windows' window-centre DPI while dragging. All other events propagate.
        EventResult::PreventDefault
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Result, ensure};
    use slint::winit_030::{WinitWindowAccessor, winit};
    use slint::{ComponentHandle, Timer, TimerMode};
    use std::{cell::RefCell, rc::Rc, time::Duration};
    use windows_sys::Win32::{
        Foundation::{POINT, RECT},
        Graphics::Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DwmGetWindowAttribute,
        },
        UI::WindowsAndMessaging::{
            GetWindowRect, MINMAXINFO, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowPos,
            WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_MOVING,
            WM_SIZING,
        },
    };
    use winit::{
        platform::windows::{EventLoopBuilderExtWindows, MonitorHandleExtWindows},
        raw_window_handle::{HasWindowHandle, RawWindowHandle},
    };

    #[test]
    fn resize_dpi_and_native_theme_regression() -> Result<()> {
        let mut builder = winit::event_loop::EventLoop::with_user_event();
        builder.with_any_thread(true);
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .with_winit_event_loop_builder(builder)
            .with_winit_window_attributes_hook(|a| a.with_visible(false).with_active(false))
            .select()?;
        // A real, hidden application window: no account, audio, or desktop input.
        let ui = crate::AppWindow::new()?;
        let drag = Rc::new(drag::DragState::default());
        install_with_state(&ui, drag.clone());
        ui.set_sources(
            Rc::new(slint::VecModel::from(vec![
                "本地音乐".into(),
                "网易云音乐".into(),
            ]))
            .into(),
        );
        ui.set_nav(
            Rc::new(slint::VecModel::from(
                crate::local_nav()
                    .into_iter()
                    .map(|entry| crate::Nav {
                        title: entry.title.into(),
                    })
                    .collect::<Vec<_>>(),
            ))
            .into(),
        );
        let outcome = Rc::new(RefCell::new(None));
        let result = outcome.clone();
        let weak = ui.as_weak();
        let timer = Timer::default();
        let mut step = 0;
        let mut baseline = None;
        let mut suggested = RECT::default();
        let mut expected_native_scale: Option<f64> = None;
        let mut original_outer: Option<drag::Rect> = None;
        let mut expected_outer: Option<drag::Rect> = None;
        let mut source_monitor: Option<drag::Monitor> = None;
        enum Action {
            Dpi(u32),
            Resize(u32, u32),
            Theme(bool),
            Burst,
            View(&'static str),
            BeginDrag,
            Move(i32, i32, u32),
            DragDpi(u32, bool),
            Sizing,
            EndDrag,
            HardwareBegin,
            HardwareMove(usize),
        }
        let actions = [
            Action::Resize(720, 480),
            Action::Dpi(96),
            Action::Dpi(144),
            Action::Dpi(192),
            Action::Resize(960, 620),
            Action::Theme(false),
            Action::Dpi(120),
            Action::Dpi(96),
            Action::Dpi(144),
            Action::Resize(1240, 820),
            Action::Theme(true),
            Action::Burst,
            Action::Resize(720, 480),
            Action::View("tracks"),
            Action::View("cards"),
            Action::View("settings"),
            Action::Theme(false),
            Action::View("lyrics"),
            Action::Dpi(96),
            Action::Resize(900, 600),
            Action::BeginDrag,
            Action::Move(3840, 2160, 192),
            Action::DragDpi(192, false),
            Action::Move(2560, 1440, 144),
            Action::DragDpi(144, false),
            Action::Move(1920, 1080, 96),
            Action::DragDpi(96, false),
            Action::Move(1440, 2560, 96),
            Action::Move(1080, 1920, 96),
            Action::Move(1920, 1080, 96),
            Action::Move(3840, 2160, 96),
            Action::Move(1920, 1080, 96),
            Action::Move(3840, 2160, 192),
            Action::DragDpi(192, true),
            Action::Dpi(96),
            Action::Resize(1000, 680),
            Action::BeginDrag,
            Action::Move(1920, 1080, 120),
            Action::Move(1920, 1080, 96),
            Action::EndDrag,
            Action::BeginDrag,
            Action::Sizing,
            Action::Resize(800, 520),
            Action::EndDrag,
            Action::HardwareBegin,
            Action::HardwareMove(0),
            Action::HardwareMove(1),
            Action::HardwareMove(0),
            Action::EndDrag,
        ];
        timer.start(TimerMode::Repeated, Duration::from_millis(120), move || {
            let check = (|| -> Result<bool> {
                let ui = weak.upgrade().context("test window was destroyed")?;
                ui.window()
                    .with_winit_window(|native| -> Result<bool> {
                        ensure!(
                            !native.is_visible().unwrap_or(false),
                            "test window must stay hidden"
                        );
                        let (expected, zoom) = *baseline.get_or_insert_with(|| {
                            (native.inner_size(), ui.window().scale_factor())
                        });
                        ensure!(expected.width > 0 && expected.height > 0, "empty window");
                        ensure!(native.is_resizable(), "window resizing is disabled");
                        {
                            let RawWindowHandle::Win32(handle) = native.window_handle()?.as_raw()
                            else {
                                anyhow::bail!("expected Windows handle");
                            };
                            let mut limits = MINMAXINFO::default();
                            // SAFETY: same-thread query of our live, hidden test window.
                            unsafe {
                                SendMessageW(
                                    handle.hwnd.get() as _,
                                    WM_GETMINMAXINFO,
                                    0,
                                    (&mut limits as *mut MINMAXINFO) as isize,
                                );
                            }
                            let border = native.outer_size().width - native.inner_size().width;
                            let caption = native.outer_size().height - native.inner_size().height;
                            ensure!(
                                limits.ptMinTrackSize.x
                                    <= (720.0 * zoom).ceil() as i32 + border as i32
                                    && limits.ptMinTrackSize.y
                                        <= (480.0 * zoom).ceil() as i32 + caption as i32,
                                "minimum track size is too large: {}x{} at {zoom}",
                                limits.ptMinTrackSize.x,
                                limits.ptMinTrackSize.y
                            );
                        }
                        ensure!(
                            native.inner_size() == expected,
                            "step {step}: client size changed from {expected:?} to {:?}",
                            native.inner_size()
                        );
                        ensure!(
                            ui.window().size()
                                == slint::PhysicalSize::new(expected.width, expected.height),
                            "step {step}: Slint's cached size is out of sync with Windows"
                        );
                        ensure!(
                            ui.window().scale_factor() == zoom,
                            "step {step}: UI zoom changed from {zoom} to {}",
                            ui.window().scale_factor()
                        );
                        if let Some(scale) = expected_native_scale {
                            ensure!(
                                (native.scale_factor() - scale).abs() < 0.001,
                                "native DPI event was not processed"
                            );
                        }
                        let snapshot = ui.window().take_snapshot()?;
                        ensure!(
                            snapshot.width() == expected.width
                                && snapshot.height() == expected.height,
                            "step {step}: renderer size is out of sync with Windows"
                        );
                        let RawWindowHandle::Win32(handle) = native.window_handle()?.as_raw()
                        else {
                            anyhow::bail!("expected Windows handle");
                        };
                        let hwnd = handle.hwnd.get() as _;
                        if let Some(expected_rect) = expected_outer {
                            let mut actual = RECT::default();
                            // SAFETY: same-thread query of our live hidden test window.
                            ensure!(unsafe { GetWindowRect(hwnd, &mut actual) } != 0, "GetWindowRect failed");
                            ensure!(drag::Rect::from(actual) == expected_rect,
                                "step {step}: drag rectangle changed: {:?}, expected {expected_rect:?}", drag::Rect::from(actual));
                            let original = original_outer.context("missing original drag size")?;
                            let actual = drag::Rect::from(actual);
                            // Independent aspect-ratio check, allowing only the
                            // half-pixel rounding of each physical dimension.
                            let aspect_error = (i64::from(actual.width) * i64::from(original.height)
                                - i64::from(actual.height) * i64::from(original.width)).abs();
                            ensure!(aspect_error <= (i64::from(original.width) + i64::from(original.height) + 1) / 2,
                                "step {step}: native window aspect ratio changed");
                        }
                        let theme = ui.global::<crate::Theme>();
                        for (attribute, expected_color) in [
                            (DWMWA_CAPTION_COLOR, theme.get_bg()),
                            (DWMWA_TEXT_COLOR, theme.get_ink()),
                            (DWMWA_BORDER_COLOR, theme.get_elevated()),
                        ] {
                            let mut actual = 0_u32;
                            // SAFETY: same-thread query; the output buffer matches COLORREF.
                            let hr = unsafe {
                                DwmGetWindowAttribute(
                                    handle.hwnd.get() as _,
                                    attribute as _,
                                    (&mut actual as *mut u32).cast(),
                                    size_of::<u32>() as _,
                                )
                            };
                            // Exact caption colors are a Windows 11 feature. On older Windows,
                            // the native dark/light theme remains the supported fallback.
                            if hr >= 0 {
                                let expected_color = u32::from(expected_color.red())
                                    | (u32::from(expected_color.green()) << 8)
                                    | (u32::from(expected_color.blue()) << 16);
                                ensure!(
                                    actual == expected_color,
                                    "step {step}: native title-bar theme differs from the UI"
                                );
                            }
                        }
                        if let Some(directory) = std::env::var_os("MUSIC_API_WINDOW_PREVIEW_DIR") {
                            let path = std::path::PathBuf::from(directory)
                                .join(format!("window-{step:02}.png"));
                            image::save_buffer(
                                path,
                                snapshot.as_bytes(),
                                snapshot.width(),
                                snapshot.height(),
                                image::ColorType::Rgba8,
                            )?;
                        }
                        match actions.get(step) {
                            Some(Action::Dpi(dpi)) => {
                                let RawWindowHandle::Win32(handle) =
                                    native.window_handle()?.as_raw()
                                else {
                                    anyhow::bail!("expected Windows handle");
                                };
                                let hwnd = handle.hwnd.get() as _;
                                let new_zoom = *dpi as f32 / 96.0;
                                let size = native.inner_size().to_logical::<f64>(native.scale_factor())
                                    .to_physical::<u32>(f64::from(new_zoom));
                                baseline = Some((size, new_zoom));
                                expected_outer = None;
                                // SAFETY: this test owns the live HWND on this thread. SendMessageW
                                // borrows the valid RECT only for this synchronous call.
                                unsafe {
                                    ensure!(
                                        GetWindowRect(hwnd, &mut suggested) != 0,
                                        "GetWindowRect failed"
                                    );
                                    let value = *dpi as usize;
                                    SendMessageW(
                                        hwnd,
                                        WM_DPICHANGED,
                                        value | (value << 16),
                                        (&suggested as *const RECT) as isize,
                                    );
                                }
                                expected_native_scale = Some(f64::from(*dpi) / 96.0);
                            }
                            Some(Action::Resize(width, height)) => {
                                expected_outer = None;
                                let size = winit::dpi::LogicalSize::new(*width, *height)
                                    .to_physical(f64::from(zoom));
                                let _ = native.request_inner_size(size);
                                baseline = Some((size, zoom));
                            }
                            Some(Action::Theme(dark)) => ui.set_dark(*dark),
                            Some(Action::Burst) => {
                                // Several sizes queued in one event-loop iteration reproduce
                                // a fast edge drag with coalesced/late resize notifications.
                                for width in [1100, 840, 1200, 760, 900] {
                                    let size = winit::dpi::LogicalSize::new(width, 600)
                                        .to_physical(f64::from(zoom));
                                    let _ = native.request_inner_size(size);
                                    baseline = Some((size, zoom));
                                }
                            }
                            Some(Action::View(view)) => {
                                ui.set_show_settings(*view == "settings");
                                ui.set_show_lyrics(*view == "lyrics");
                                ui.set_cards(*view == "cards");
                                ui.set_rows(
                                    std::rc::Rc::new(slint::VecModel::from(vec![
                                        crate::Row {
                                            title: "测试歌曲 · A Quiet Evening".into(),
                                            subtitle: "独立音乐人".into(),
                                            album: "我的专辑".into(),
                                            duration: "04:12".into(),
                                            ..Default::default()
                                        };
                                        12
                                    ]))
                                    .into(),
                                );
                                ui.set_now_title("A Quiet Evening".into());
                                ui.set_now_artist("独立音乐人".into());
                                ui.set_lyric("让旋律慢慢流淌".into());
                                ui.set_lyrics("第一行歌词\n第二行歌词\n第三行歌词".into());
                            }
                            Some(Action::BeginDrag | Action::HardwareBegin) => {
                                let screen = drag::Rect { left: 0, top: 0, width: 1920, height: 1080 };
                                let source = if matches!(actions.get(step), Some(Action::HardwareBegin)) {
                                    let monitor = native.current_monitor().context("no current monitor")?;
                                    let monitor = unsafe { drag::monitor_info(monitor.hmonitor() as _) }.context("no monitor metrics")?;
                                    let mut rect = RECT::default();
                                    unsafe {
                                        GetWindowRect(hwnd, &mut rect);
                                        let value = monitor.dpi as usize;
                                        SendMessageW(hwnd, WM_DPICHANGED, value | (value << 16), (&rect as *const RECT) as isize);
                                    }
                                    baseline = Some((native.inner_size(), monitor.dpi as f32 / 96.0));
                                    expected_native_scale = None;
                                    eprintln!("real monitor drag: source {monitor:?}");
                                    monitor
                                } else { drag::Monitor { bounds: screen, work: screen, dpi: 96 } };
                                let mut rect = RECT::default();
                                ensure!(unsafe { GetWindowRect(hwnd, &mut rect) } != 0, "GetWindowRect failed");
                                let rect = drag::Rect::from(rect);
                                drag.input.set(Some((POINT { x: rect.left + rect.width / 2, y: rect.top + 20 },
                                    source)));
                                unsafe { SendMessageW(hwnd, WM_ENTERSIZEMOVE, 0, 0); }
                                original_outer = Some(rect);
                                source_monitor = Some(source);
                            }
                            Some(action @ (Action::Move(..) | Action::HardwareMove(..))) => {
                                let (target, cursor) = match action {
                                    Action::Move(width, height, dpi) => {
                                        let screen = drag::Rect { left: 0, top: 0, width: *width, height: *height };
                                        (drag::Monitor { bounds: screen, work: screen, dpi: *dpi }, POINT { x: 900, y: 150 })
                                    }
                                    Action::HardwareMove(index) => {
                                        let monitors = native.available_monitors().collect::<Vec<_>>();
                                        let monitor = monitors.get(*index).or(monitors.first()).context("no monitors")?;
                                        let monitor = unsafe { drag::monitor_info(monitor.hmonitor() as _) }.context("no monitor metrics")?;
                                        eprintln!("real monitor drag: target {monitor:?}");
                                        (monitor, POINT { x: monitor.work.left + monitor.work.width / 2, y: monitor.work.top + 100 })
                                    }
                                    _ => unreachable!(),
                                };
                                drag.input.set(Some((cursor, target)));
                                let original = original_outer.context("drag was not started")?;
                                let source = source_monitor.context("missing source monitor")?;
                                let minimum = drag::minimum_outer(hwnd, (720, 480), target.dpi);
                                let resolution_scale = (f64::from(target.bounds.width.max(target.bounds.height))
                                    / f64::from(source.bounds.width.max(source.bounds.height)))
                                    .min(f64::from(target.bounds.width.min(target.bounds.height))
                                        / f64::from(source.bounds.width.min(source.bounds.height)));
                                let lower = (f64::from(minimum.0) / f64::from(original.width))
                                    .max(f64::from(minimum.1) / f64::from(original.height));
                                let upper = (f64::from(target.work.width) / f64::from(original.width))
                                    .min(f64::from(target.work.height) / f64::from(original.height));
                                let scale = resolution_scale.clamp(lower, upper.max(lower));
                                let width = (f64::from(original.width) * scale).round() as i32;
                                let height = (f64::from(original.height) * scale).round() as i32;
                                let expected = drag::Rect {
                                    left: cursor.x - (f64::from(original.width / 2) / f64::from(original.width) * f64::from(width)).round() as i32,
                                    top: cursor.y - (20.0 * f64::from(target.dpi) / f64::from(source.dpi)).round() as i32,
                                    width, height,
                                };
                                let mut proposed = RECT::from(original);
                                unsafe {
                                    ensure!(SendMessageW(hwnd, WM_MOVING, 0, (&mut proposed as *mut RECT) as isize) != 0,
                                        "WM_MOVING was not handled");
                                    ensure!(drag::Rect::from(proposed) == expected, "WM_MOVING used the wrong screen or baseline");
                                    ensure!(SetWindowPos(hwnd, std::ptr::null_mut(), proposed.left, proposed.top,
                                        proposed.right - proposed.left, proposed.bottom - proposed.top, SWP_NOACTIVATE | SWP_NOZORDER) != 0,
                                        "SetWindowPos failed");
                                }
                                expected_outer = Some(expected);
                                baseline = Some((native.inner_size(), target.dpi as f32 / 96.0));
                                expected_native_scale = None;
                            }
                            Some(Action::DragDpi(dpi, end)) => {
                                let mut rect = RECT::default();
                                unsafe {
                                    ensure!(GetWindowRect(hwnd, &mut rect) != 0, "GetWindowRect failed");
                                    let value = *dpi as usize;
                                    SendMessageW(hwnd, WM_DPICHANGED, value | (value << 16), (&rect as *const RECT) as isize);
                                    if *end { SendMessageW(hwnd, WM_EXITSIZEMOVE, 0, 0); }
                                }
                                expected_native_scale = Some(f64::from(*dpi) / 96.0);
                            }
                            Some(Action::Sizing) => {
                                let mut rect = RECT::default();
                                unsafe {
                                    GetWindowRect(hwnd, &mut rect);
                                    SendMessageW(hwnd, WM_SIZING, 1, (&mut rect as *mut RECT) as isize);
                                }
                                expected_outer = None;
                            }
                            Some(Action::EndDrag) => {
                                unsafe { SendMessageW(hwnd, WM_EXITSIZEMOVE, 0, 0); }
                            }
                            None => return Ok(true),
                        }
                        step += 1;
                        Ok(false)
                    })
                    .context("native window was not created")?
            })();
            match check {
                Ok(false) => {}
                done => {
                    *result.borrow_mut() = Some(done.map(|_| ()));
                    let _ = slint::quit_event_loop();
                }
            }
        });
        slint::run_event_loop_until_quit()?;
        timer.stop();
        outcome
            .borrow_mut()
            .take()
            .context("DPI regression did not finish")?
    }
}
