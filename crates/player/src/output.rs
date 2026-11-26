use super::Handle;
use super::audio::{AudioApi, OutputDevice, Outputs};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

pub(super) struct Routing {
    pub preferred: String,
    pub state: Option<Outputs>,
    applied: String,
    refresh: Instant,
    system_id: Option<String>,
    defaults: mpsc::Receiver<Option<String>>,
    stop: mpsc::Sender<()>,
}
impl Routing {
    pub fn new() -> Self {
        let (tx, defaults) = mpsc::channel();
        let (stop, wait) = mpsc::channel();
        std::thread::spawn(move || {
            let mut previous = None;
            loop {
                let current = default_endpoint();
                if current != previous {
                    previous = current.clone();
                    if tx.send(current).is_err() {
                        break;
                    }
                }
                if wait.recv_timeout(Duration::from_secs(2)) != Err(mpsc::RecvTimeoutError::Timeout)
                {
                    break;
                }
            }
        });
        Self {
            preferred: "auto".into(),
            state: None,
            applied: "auto".into(),
            refresh: Instant::now(),
            system_id: None,
            defaults,
            stop,
        }
    }
    pub fn select(&mut self, id: String) {
        self.preferred = id;
        self.state = None;
        self.refresh = Instant::now();
    }
    pub fn refresh(&mut self, api: AudioApi, h: Handle) -> anyhow::Result<Option<Outputs>> {
        for id in self.defaults.try_iter() {
            self.system_id = id;
            self.refresh = Instant::now();
        }
        if Instant::now() < self.refresh {
            return Ok(None);
        }
        self.refresh = Instant::now() + Duration::from_secs(2);
        let Some(list) = api
            .text(h, "audio-device-list")
            .and_then(|v| serde_json::from_str::<Vec<OutputDevice>>(&v).ok())
        else {
            return Ok(None);
        };
        let next = outputs(list, &self.preferred, self.system_id.as_deref());
        if self.applied != next.selected {
            api.command(h, &["set", "audio-device", &next.selected])?;
            self.applied = next.selected.clone();
        }
        if self.state.as_ref() == Some(&next) {
            return Ok(None);
        }
        self.state = Some(next.clone());
        Ok(Some(next))
    }
}
impl Drop for Routing {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

fn outputs(mut devices: Vec<OutputDevice>, preferred: &str, default: Option<&str>) -> Outputs {
    // Backend-only aliases (e.g. "openal") do not identify an individual device.
    devices.retain(|d| {
        d.name == "auto"
            || d.name
                .split_once('/')
                .is_some_and(|(_, id)| !id.is_empty() && id != "default")
    });
    let selected = devices
        .iter()
        .find(|d| d.name == preferred && preferred != "auto")
        .map_or("auto", |d| d.name.as_str())
        .to_owned();
    let active = if selected == "auto" {
        default
            .and_then(|id| {
                devices.iter().find(|d| {
                    d.name
                        .split_once('/')
                        .is_some_and(|(_, suffix)| suffix == id)
                })
            })
            .map(|d| d.name.clone())
            .or_else(|| {
                default
                    .filter(|id| id.starts_with("android/"))
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    } else {
        selected.clone()
    };
    Outputs {
        devices,
        active,
        selected,
    }
}

#[cfg(windows)]
fn default_endpoint() -> Option<String> {
    use windows::Win32::{
        Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender},
        System::Com::{
            CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
            CoUninitialize,
        },
    };
    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = (|| {
            let devices: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let id = devices
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
                .ok()?
                .GetId()
                .ok()?;
            let name = id.to_string().ok();
            CoTaskMemFree(Some(id.as_ptr().cast()));
            name.map(|s| s.strip_prefix("{0.0.0.00000000}.").unwrap_or(&s).to_owned())
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        result
    }
}

#[cfg(target_os = "macos")]
fn default_endpoint() -> Option<String> {
    use std::ffi::{CStr, c_void};
    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }
    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyData(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringGetCString(s: *const c_void, buf: *mut i8, size: isize, encoding: u32) -> u8;
        fn CFRelease(value: *const c_void);
    }
    fn read<T>(object: u32, selector: [u8; 4], value: &mut T) -> bool {
        let addr = Address {
            selector: u32::from_be_bytes(selector),
            scope: u32::from_be_bytes(*b"glob"),
            element: 0,
        };
        let mut size = std::mem::size_of::<T>() as u32;
        unsafe {
            AudioObjectGetPropertyData(
                object,
                &addr,
                0,
                std::ptr::null(),
                &mut size,
                std::ptr::from_mut(value).cast(),
            ) == 0
        }
    }
    let mut device = 0u32;
    let mut uid: *const c_void = std::ptr::null();
    if !read(1, *b"dOut", &mut device) || !read(device, *b"uid ", &mut uid) || uid.is_null() {
        return None;
    }
    let mut buffer = [0i8; 1024];
    unsafe {
        let ok = CFStringGetCString(uid, buffer.as_mut_ptr(), buffer.len() as isize, 0x08000100);
        CFRelease(uid);
        (ok != 0).then(|| {
            CStr::from_ptr(buffer.as_ptr())
                .to_string_lossy()
                .into_owned()
        })
    }
}

#[cfg(target_os = "linux")]
fn default_endpoint() -> Option<String> {
    use std::process::Command;
    let read = |bin: &str, args: &[&str]| -> Option<String> {
        let out = Command::new(bin).args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    read("pactl", &["get-default-sink"])
        .filter(|v| !v.is_empty())
        .or_else(|| {
            let props = read("wpctl", &["inspect", "@DEFAULT_AUDIO_SINK@"])?;
            props.lines().find_map(|line| {
                let (key, value) = line
                    .trim()
                    .trim_start_matches('*')
                    .trim()
                    .split_once(" = ")?;
                (key == "node.name").then(|| value.trim_matches('"').to_owned())
            })
        })
}

#[cfg(target_os = "android")]
#[path = "android_output.rs"]
mod android;
#[cfg(target_os = "android")]
use android::default_endpoint;

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "macos",
    target_os = "android"
)))]
fn default_endpoint() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switching_or_unplugging_uses_the_actual_device_id() {
        let list = || {
            ["auto", "wasapi/first", "wasapi/second"]
                .map(|s| OutputDevice {
                    name: s.into(),
                    description: "同名设备".into(),
                })
                .to_vec()
        };
        assert_eq!(
            outputs(list(), "auto", Some("first")).active,
            "wasapi/first"
        );
        assert_eq!(
            outputs(list(), "auto", Some("second")).active,
            "wasapi/second"
        );
        assert_eq!(
            outputs(list(), "wasapi/first", Some("second")).active,
            "wasapi/first"
        );
        let unplugged = outputs(list(), "wasapi/missing", Some("second"));
        assert_eq!(
            (unplugged.selected.as_str(), unplugged.active.as_str()),
            ("auto", "wasapi/second")
        );
        assert!(outputs(list(), "auto", None).active.is_empty());
    }
}
