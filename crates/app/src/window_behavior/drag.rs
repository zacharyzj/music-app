//! A drag always scales from its initial geometry, using the monitor under the cursor.

use slint::winit_030::winit;
use std::{cell::Cell, rc::Rc};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
        MonitorFromWindow,
    },
    UI::{
        HiDpi::{AdjustWindowRectExForDpi, GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GWL_EXSTYLE, GWL_STYLE, GetCursorPos, GetWindowLongPtrW, GetWindowRect, IsIconic,
            IsZoomed, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, SWP_HIDEWINDOW, SWP_NOMOVE, SWP_NOSIZE,
            WINDOWPOS, WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_MOVING, WM_NCDESTROY,
            WM_SIZING, WM_SYSCOMMAND, WM_WINDOWPOSCHANGING,
        },
    },
};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl From<RECT> for Rect {
    fn from(r: RECT) -> Self {
        Self {
            left: r.left,
            top: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
        }
    }
}

impl From<Rect> for RECT {
    fn from(r: Rect) -> Self {
        Self {
            left: r.left,
            top: r.top,
            right: r.left + r.width,
            bottom: r.top + r.height,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Monitor {
    pub bounds: Rect,
    pub work: Rect,
    pub dpi: u32,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    original: Rect,
    monitor: Monitor,
    grab_x: f64,
    grab_y: f64,
}

impl Drag {
    fn begin(original: Rect, monitor: Monitor, cursor: POINT) -> Self {
        Self {
            original,
            monitor,
            grab_x: (f64::from(cursor.x - original.left) / f64::from(original.width))
                .clamp(0.0, 1.0),
            grab_y: f64::from(cursor.y - original.top) * 96.0 / f64::from(monitor.dpi),
        }
    }

    fn placement(self, monitor: Monitor, cursor: POINT, minimum: (i32, i32)) -> Rect {
        // Compare long/short edges so rotating a monitor does not reshape the
        // window. Both axes (including size limits) must use one scale factor.
        // DPI still controls layout and the caption offset, not another resize.
        let edges = |r: Rect| {
            (
                f64::from(r.width.max(r.height)),
                f64::from(r.width.min(r.height)),
            )
        };
        let (source_long, source_short) = edges(self.monitor.bounds);
        let (target_long, target_short) = edges(monitor.bounds);
        let original_width = f64::from(self.original.width);
        let original_height = f64::from(self.original.height);
        let minimum_scale =
            (f64::from(minimum.0) / original_width).max(f64::from(minimum.1) / original_height);
        let fit_scale = (f64::from(monitor.work.width) / original_width)
            .min(f64::from(monitor.work.height) / original_height);
        let scale = (target_long / source_long)
            .min(target_short / source_short)
            .clamp(minimum_scale, fit_scale.max(minimum_scale));
        let width = (original_width * scale).round() as i32;
        let height = (original_height * scale).round() as i32;
        Rect {
            left: cursor.x - (self.grab_x * f64::from(width)).round() as i32,
            top: cursor.y - (self.grab_y * f64::from(monitor.dpi) / 96.0).round() as i32,
            width,
            height,
        }
    }
}

#[derive(Default)]
pub(super) struct DragState {
    hwnd: Cell<HWND>,
    drag: Cell<Option<Drag>>,
    placement: Cell<Option<Rect>>,
    pub dpi: Cell<Option<u32>>,
    // Retained after WM_EXITSIZEMOVE until the next native DPI notification, so
    // an already queued winit ScaleFactorChanged cannot undo the final size.
    pub dpi_during_drag: Cell<bool>,
    pub minimum: Cell<(i32, i32)>,
    #[cfg(test)]
    pub input: Cell<Option<(POINT, Monitor)>>,
}

impl DragState {
    pub fn attach(self: &Rc<Self>, window: &Window) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = handle.hwnd.get() as HWND;
        if self.hwnd.get() == hwnd {
            return;
        }
        let data = Rc::into_raw(self.clone());
        // SAFETY: Slint creates and accesses this HWND on its event-loop thread.
        // The subclass owns one Rc until WM_NCDESTROY (or installation failure).
        if unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, data as usize) } != 0 {
            self.hwnd.set(hwnd);
        } else {
            unsafe {
                drop(Rc::from_raw(data));
            }
        }
    }

    fn cursor_monitor(&self) -> Option<(POINT, Monitor)> {
        #[cfg(test)]
        if let Some(input) = self.input.get() {
            return Some(input);
        }
        let mut point = POINT::default();
        // SAFETY: output points to a valid POINT; monitor handles are borrowed.
        unsafe {
            if GetCursorPos(&mut point) == 0 {
                return None;
            }
            monitor_info(MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST)).map(|m| (point, m))
        }
    }

    fn begin(&self, hwnd: HWND) {
        self.end();
        if !normal(hwnd) {
            return;
        }
        let Some((cursor, cursor_monitor)) = self.cursor_monitor() else {
            return;
        };
        let mut rect = RECT::default();
        // SAFETY: the subclass is called with a live HWND on its owner thread.
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
            return;
        }
        let rect = Rect::from(rect);
        if rect.width <= 0 || rect.height <= 0 {
            return;
        }
        #[cfg(test)]
        let monitor = if self.input.get().is_some() {
            Some(cursor_monitor)
        } else {
            None
        };
        #[cfg(not(test))]
        let monitor = None;
        let monitor = monitor
            .or_else(|| unsafe { monitor_info(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)) })
            .unwrap_or(cursor_monitor);
        self.drag.set(Some(Drag::begin(rect, monitor, cursor)));
    }

    fn moving(&self, hwnd: HWND) -> Option<Rect> {
        // Dragging a maximized caption can restore the window after ENTERSIZEMOVE.
        // Capture the restored geometry once it becomes a normal window.
        if self.drag.get().is_none() {
            self.begin(hwnd);
        }
        let drag = self.drag.get()?;
        if !normal(hwnd) {
            self.end();
            return None;
        }
        let (cursor, monitor) = self.cursor_monitor()?;
        let minimum = minimum_outer(hwnd, self.minimum.get(), monitor.dpi);
        let placement = drag.placement(monitor, cursor, minimum);
        self.dpi.set(Some(monitor.dpi));
        self.placement.set(Some(placement));
        Some(placement)
    }

    fn end(&self) {
        self.drag.set(None);
        self.placement.set(None);
    }
}

fn normal(hwnd: HWND) -> bool {
    // SAFETY: queries do not retain the borrowed HWND.
    unsafe { IsZoomed(hwnd) == 0 && IsIconic(hwnd) == 0 }
}

pub(super) unsafe fn monitor_info(handle: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // Use the same effective-DPI query as winit's MonitorHandle::scale_factor.
    // Winit establishes process per-monitor DPI awareness before window creation.
    let (mut x, mut y) = (0, 0);
    if unsafe { GetMonitorInfoW(handle, &mut info) } == 0
        || unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut x, &mut y) } < 0
        || x == 0
    {
        return None;
    }
    let bounds = Rect::from(info.rcMonitor);
    let work = Rect::from(info.rcWork);
    (bounds.width > 0 && bounds.height > 0 && work.width > 0 && work.height > 0).then_some(
        Monitor {
            bounds,
            work,
            dpi: x,
        },
    )
}

pub(super) fn minimum_outer(hwnd: HWND, logical: (i32, i32), dpi: u32) -> (i32, i32) {
    let scale = f64::from(dpi) / 96.0;
    let mut rect = RECT {
        right: (f64::from(logical.0) * scale).ceil() as i32,
        bottom: (f64::from(logical.1) * scale).ceil() as i32,
        ..Default::default()
    };
    // SAFETY: querying our window's styles and updating a stack-owned RECT.
    unsafe {
        AdjustWindowRectExForDpi(
            &mut rect,
            GetWindowLongPtrW(hwnd, GWL_STYLE) as u32,
            0,
            GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32,
            dpi,
        );
    }
    (rect.right - rect.left, rect.bottom - rect.top)
}

const SUBCLASS_ID: usize = 0x4d555349; // MUSI, scoped to this subclass procedure.

unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: attach retains an Rc for this subclass until WM_NCDESTROY. Keep
    // another reference on this stack across potentially reentrant destruction.
    let state = unsafe {
        Rc::increment_strong_count(data as *const DragState);
        Rc::from_raw(data as *const DragState)
    };
    match message {
        WM_ENTERSIZEMOVE => state.begin(hwnd),
        WM_SIZING | WM_EXITSIZEMOVE => state.end(),
        WM_SYSCOMMAND
            if matches!(
                wparam as u32 & 0xfff0,
                SC_MAXIMIZE | SC_MINIMIZE | SC_RESTORE
            ) =>
        {
            state.end()
        }
        WM_MOVING if lparam != 0 => {
            if let Some(rect) = state.moving(hwnd) {
                // SAFETY: WM_MOVING lends a writable RECT for the duration of the call.
                unsafe {
                    *(lparam as *mut RECT) = rect.into();
                }
                return 1;
            }
        }
        WM_DPICHANGED => {
            let dragging = state.placement.get().is_some();
            state.dpi_during_drag.set(dragging);
            if !dragging {
                state.dpi.set(Some(wparam as u32 & 0xffff));
            }
        }
        WM_WINDOWPOSCHANGING if lparam != 0 => {
            // Let Windows apply its normal constraints first. Winit's DPI handler
            // also resizes/nudges the window by its centre; the cursor-based drag
            // rectangle must win so the two policies cannot fight each other.
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            if let Some(rect) = state.placement.get().filter(|_| normal(hwnd)) {
                // SAFETY: WM_WINDOWPOSCHANGING lends a writable WINDOWPOS.
                let pos = unsafe { &mut *(lparam as *mut WINDOWPOS) };
                if pos.flags & SWP_HIDEWINDOW == 0 {
                    pos.x = rect.left;
                    pos.y = rect.top;
                    pos.cx = rect.width;
                    pos.cy = rect.height;
                    pos.flags &= !(SWP_NOMOVE | SWP_NOSIZE);
                }
            }
            return result;
        }
        WM_NCDESTROY => {
            state.end();
            state.hwnd.set(std::ptr::null_mut());
            // SAFETY: remove before forwarding destruction; release exactly the
            // Rc retained by attach, without borrowing it across a reentrant call.
            unsafe {
                RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID);
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                drop(Rc::from_raw(data as *const DragState));
                return result;
            }
        }
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(width: i32, height: i32, dpi: u32) -> Monitor {
        let bounds = Rect {
            left: 0,
            top: 0,
            width,
            height,
        };
        Monitor {
            bounds,
            work: bounds,
            dpi,
        }
    }

    fn start() -> Drag {
        Drag::begin(
            Rect {
                left: 100,
                top: 100,
                width: 1000,
                height: 700,
            },
            monitor(1920, 1080, 96),
            POINT { x: 350, y: 120 },
        )
    }

    #[test]
    fn resolution_controls_occupancy_dpi_only_changes_layout() {
        for dpi in [96, 120, 144, 192] {
            let target =
                start().placement(monitor(3840, 2160, dpi), POINT { x: 2500, y: 80 }, (0, 0));
            assert_eq!((target.width, target.height), (2000, 1400));
            assert_eq!(target.left, 2000);
            assert_eq!(target.top, 80 - (20 * dpi / 96) as i32);
            let same_resolution =
                start().placement(monitor(1920, 1080, dpi), POINT { x: 350, y: 120 }, (0, 0));
            assert_eq!((same_resolution.width, same_resolution.height), (1000, 700));
        }
    }

    #[test]
    fn repeated_crossings_do_not_accumulate_rounding_or_use_last_target() {
        let drag = start();
        for _ in 0..100 {
            let other =
                drag.placement(monitor(2560, 1440, 144), POINT { x: -800, y: -600 }, (0, 0));
            assert_eq!((other.width, other.height), (1333, 933));
            assert_eq!((other.left, other.top), (-1133, -630));
            let original =
                drag.placement(monitor(1920, 1080, 96), POINT { x: 350, y: 120 }, (0, 0));
            assert_eq!(original, drag.original);
        }
    }

    #[test]
    fn rotating_a_monitor_preserves_window_size_and_aspect_ratio() {
        let target = start().placement(monitor(1080, 1920, 96), POINT { x: -400, y: 40 }, (0, 0));
        assert_eq!((target.width, target.height), (1000, 700));
        assert_eq!((target.left, target.top), (-650, 20));
    }

    #[test]
    fn landscape_4k_to_portrait_1440p_scales_both_axes_by_two_thirds() {
        let original = Rect {
            left: 100,
            top: 100,
            width: 1500,
            height: 1050,
        };
        let drag = Drag::begin(original, monitor(3840, 2160, 144), POINT { x: 475, y: 130 });
        for _ in 0..100 {
            let target = drag.placement(
                monitor(1440, 2560, 96),
                POINT { x: -720, y: 50 },
                (720, 480),
            );
            assert_eq!((target.width, target.height), (1000, 700));
            assert_eq!((target.left, target.top), (-970, 30));
            let restored = drag.placement(
                monitor(3840, 2160, 144),
                POINT { x: 475, y: 130 },
                (1080, 720),
            );
            assert_eq!(restored, original);
        }
        let portrait = Drag::begin(
            Rect {
                left: -1000,
                top: 100,
                width: 1000,
                height: 700,
            },
            monitor(1440, 2560, 96),
            POINT { x: -750, y: 120 },
        );
        let target = portrait.placement(
            monitor(3840, 2160, 144),
            POINT { x: 800, y: 80 },
            (1080, 720),
        );
        assert_eq!((target.width, target.height), (1500, 1050));
    }

    #[test]
    fn a_wide_window_fits_portrait_work_area_without_stretching() {
        let drag = Drag::begin(
            Rect {
                left: 100,
                top: 100,
                width: 2400,
                height: 1600,
            },
            monitor(3840, 2160, 144),
            POINT { x: 1300, y: 130 },
        );
        let target = drag.placement(
            monitor(1440, 2560, 96),
            POINT { x: -720, y: 50 },
            (720, 480),
        );
        assert_eq!((target.width, target.height), (1440, 960));
    }

    #[test]
    fn work_area_caps_size_without_violating_minimum() {
        let mut screen = monitor(1280, 720, 192);
        screen.work.height = 680;
        let target = start().placement(screen, POINT { x: 400, y: 40 }, (720, 480));
        assert_eq!((target.width, target.height), (720, 504));
        let large = Drag {
            original: Rect {
                width: 2200,
                height: 1400,
                ..start().original
            },
            ..start()
        };
        let target = large.placement(screen, POINT { x: 400, y: 40 }, (720, 480));
        assert_eq!((target.width, target.height), (1069, 680));
        let target = start().placement(screen, POINT { x: 400, y: 40 }, (1440, 960));
        assert_eq!((target.width, target.height), (1440, 1008));
    }

    #[test]
    fn next_drag_uses_manually_resized_window() {
        let drag = Drag::begin(
            Rect {
                left: 50,
                top: 60,
                width: 900,
                height: 600,
            },
            monitor(3840, 2160, 192),
            POINT { x: 500, y: 100 },
        );
        let target = drag.placement(monitor(1920, 1080, 96), POINT { x: 500, y: 100 }, (0, 0));
        assert_eq!((target.width, target.height), (450, 300));
        assert_eq!((target.left, target.top), (275, 80));
    }
}
