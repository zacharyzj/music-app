use anyhow::{Context, Result};
use jni::{
    JNIEnv, JavaVM,
    objects::{JByteArray, JObject, JString, JValue},
};
use std::{
    cell::RefCell,
    sync::{Mutex, mpsc},
};

thread_local! {
    pub(super) static ACTIVITY: RefCell<Option<slint::android::AndroidApp>> = const { RefCell::new(None) };
}

static EVENTS: Mutex<Option<mpsc::Sender<(String, String)>>> = Mutex::new(None);

pub(super) fn events() -> mpsc::Receiver<(String, String)> {
    let (tx, rx) = mpsc::channel();
    *EVENTS.lock().unwrap() = Some(tx);
    rx
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_zacharyzj_musicapp_PlayerActivity_event(
    mut env: JNIEnv,
    _class: jni::objects::JClass,
    operation: JString,
    value: JString,
) {
    let (Ok(op), Ok(value)) = (env.get_string(&operation), env.get_string(&value)) else {
        return;
    };
    if let Ok(sender) = EVENTS.lock()
        && let Some(tx) = sender.as_ref()
    {
        let _ = tx.send((op.into(), value.into()));
    }
}

fn with_activity<T>(
    f: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> jni::errors::Result<T>,
) -> Result<T> {
    ACTIVITY.with_borrow(|app| {
        let app = app.as_ref().context("Android activity unavailable")?;
        // Both references are borrowed from the AndroidApp on its native event thread.
        let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast())? };
        let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
        let mut env = vm.attach_current_thread()?;
        let result = env.with_local_frame(12, |env| f(env, &activity));
        if result.is_err() {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        Ok(result?)
    })
}

pub(super) fn storage_key() -> Result<[u8; 32]> {
    let bytes = with_activity(|env, activity| {
        let array = JByteArray::from(
            env.call_method(activity, "pluginStorageKey", "()[B", &[])?
                .l()?,
        );
        env.convert_byte_array(array)
    })?;
    bytes
        .try_into()
        .ok()
        .context("Android Keystore returned an invalid key")
}

pub(super) fn cache_dir() -> Result<std::path::PathBuf> {
    let path = with_activity(|env, activity| {
        let dir = env
            .call_method(activity, "getCacheDir", "()Ljava/io/File;", &[])?
            .l()?;
        let path = JString::from(
            env.call_method(dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?
                .l()?,
        );
        Ok(String::from(env.get_string(&path)?))
    })?;
    Ok(path.into())
}

pub(super) fn pick_files(plugin: bool) -> Result<()> {
    with_activity(|env, activity| {
        env.call_method(
            activity,
            "pickFiles",
            "(Z)V",
            &[JValue::Bool(plugin.into())],
        )?;
        Ok(())
    })
}

pub(super) fn playback(
    playing: bool,
    title: &str,
    artist: &str,
    seconds: f32,
    duration: f32,
) -> Result<()> {
    with_activity(|env, activity| {
        let title = env.new_string(title)?;
        let artist = env.new_string(artist)?;
        env.call_method(
            activity,
            "playback",
            "(ZLjava/lang/String;Ljava/lang/String;JJ)V",
            &[
                JValue::Bool(playing.into()),
                JValue::Object(&title),
                JValue::Object(&artist),
                JValue::Long((seconds * 1000.) as i64),
                JValue::Long((duration * 1000.) as i64),
            ],
        )?;
        Ok(())
    })
}
