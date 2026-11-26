use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{self, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{LogicalPosition, PlatformError, Window};
use std::{cell::Cell, rc::Rc, time::Duration};

struct TestPlatform {
    window: Rc<MinimalSoftwareWindow>,
    time: Rc<Cell<Duration>>,
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.time.get()
    }
}

pub fn test_clock() -> anyhow::Result<impl Fn(u64)> {
    let time = Rc::new(Cell::new(Duration::ZERO));
    platform::set_platform(Box::new(TestPlatform {
        window: MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer),
        time: time.clone(),
    }))?;
    Ok(move |ms| {
        time.set(time.get() + Duration::from_millis(ms));
        platform::update_timers_and_animations();
    })
}

pub fn click(window: &Window, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    let button = PointerEventButton::Left;
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed { position, button });
    window.dispatch_event(WindowEvent::PointerReleased { position, button });
}
