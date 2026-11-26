use jni::{
    JNIEnv, JavaVM,
    objects::{JObject, JString, JValue},
};

pub(super) fn default_endpoint() -> Option<String> {
    let context = ndk_context::android_context();
    // The Activity owns the global reference for the lifetime of this playback thread.
    let vm = unsafe { JavaVM::from_raw(context.vm().cast()).ok()? };
    let activity = unsafe { JObject::from_raw(context.context().cast()) };
    let mut env = vm.attach_current_thread().ok()?;
    let result = env.with_local_frame(32, |env| media_route(env, &activity));
    if result.is_err() {
        let _ = env.exception_clear();
    }
    result.ok().flatten()
}

fn media_route(
    env: &mut JNIEnv<'_>,
    activity: &JObject<'_>,
) -> jni::errors::Result<Option<String>> {
    let sdk = env
        .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?
        .i()?;
    if sdk < 33 {
        return Ok(None);
    }
    let service = env.new_string("audio")?;
    let manager = env
        .call_method(
            activity,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[JValue::Object(&service)],
        )?
        .l()?;
    let builder = env.new_object("android/media/AudioAttributes$Builder", "()V", &[])?;
    let builder = env
        .call_method(
            builder,
            "setUsage",
            "(I)Landroid/media/AudioAttributes$Builder;",
            &[JValue::Int(1)],
        )?
        .l()?; // USAGE_MEDIA
    let attrs = env
        .call_method(builder, "build", "()Landroid/media/AudioAttributes;", &[])?
        .l()?;
    let devices = env
        .call_method(
            manager,
            "getAudioDevicesForAttributes",
            "(Landroid/media/AudioAttributes;)Ljava/util/List;",
            &[JValue::Object(&attrs)],
        )?
        .l()?;
    if env.call_method(&devices, "size", "()I", &[])?.i()? != 1 {
        return Ok(None);
    }
    let device = env
        .call_method(devices, "get", "(I)Ljava/lang/Object;", &[JValue::Int(0)])?
        .l()?;
    let kind = env.call_method(&device, "getType", "()I", &[])?.i()?;
    let address = JString::from(
        env.call_method(&device, "getAddress", "()Ljava/lang/String;", &[])?
            .l()?,
    );
    let address: String = env.get_string(&address)?.into();
    let name = env
        .call_method(&device, "getProductName", "()Ljava/lang/CharSequence;", &[])?
        .l()?;
    let name = JString::from(
        env.call_method(name, "toString", "()Ljava/lang/String;", &[])?
            .l()?,
    );
    let name: String = env.get_string(&name)?.into();
    Ok(Some(format!("android/{kind}/{address}/{name}")))
}
