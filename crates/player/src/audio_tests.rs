use super::*;

#[test]
fn gains_keep_peak_headroom_and_modes_keep_album_gain_fixed() {
    let g = Gains::from_stream(
        &serde_json::json!({"gain":12.,"peak":1.,"albumGain":-5.,"albumPeak":0.8}),
    );
    assert_eq!((g.track, g.album), (Some(-1.), Some(-5.)));
    assert_eq!(Gains::from_stream(&serde_json::json!({})).track, None);
    assert_eq!(safe_gain(6., None), Some(0.));
    let mut l = Loudness {
        mode: Normalization::Album("a".into()),
        shared: Some(("a".into(), -7.)),
        ..Default::default()
    };
    assert_eq!(l.selected(g), Some(-7.));
    l.mode = Normalization::Album("b".into());
    assert_eq!(l.selected(g), Some(-5.));
    l.mode = Normalization::Track;
    assert_eq!(l.selected(g), Some(-1.));
    l.mode = Normalization::Off;
    assert_eq!(l.selected(g), Some(0.));
}

fn tone(path: &Path, amplitude: f64) {
    use std::io::Write;
    let mut f = std::fs::File::create(path).unwrap();
    let samples = 48_000u32;
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + 2 * samples).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    for bytes in [
        16u32.to_le_bytes().as_slice(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &samples.to_le_bytes(),
        &(2 * samples).to_le_bytes(),
        &2u16.to_le_bytes(),
        &16u16.to_le_bytes(),
    ] {
        f.write_all(bytes).unwrap();
    }
    f.write_all(b"data").unwrap();
    f.write_all(&(2 * samples).to_le_bytes()).unwrap();
    let pcm = (0..samples)
        .flat_map(|i| {
            ((amplitude * (f64::from(i) * std::f64::consts::TAU * 440. / f64::from(samples)).sin())
                as i16)
                .to_le_bytes()
        })
        .collect::<Vec<_>>();
    f.write_all(&pcm).unwrap();
}

#[test]
#[ignore = "requires an installed libmpv supplied in MUSIC_API_MPV"]
fn native_loudness_and_eq() -> anyhow::Result<()> {
    let lib = unsafe {
        libloading::Library::new(std::env::var_os("MUSIC_API_MPV").expect("set MUSIC_API_MPV"))?
    };
    let api = unsafe {
        AudioApi {
            create: *lib.get(b"mpv_create")?,
            init: *lib.get(b"mpv_initialize")?,
            set: *lib.get(b"mpv_set_option_string")?,
            cmd: *lib.get(b"mpv_command")?,
            destroy: *lib.get(b"mpv_terminate_destroy")?,
            get: *lib.get(b"mpv_get_property_string")?,
            free: *lib.get(b"mpv_free")?,
        }
    };
    let h = unsafe { (api.create)() };
    assert!(!h.is_null());
    let _owner = OwnedMpv {
        handle: h,
        destroy: api.destroy,
    };
    unsafe {
        (api.set)(h, c"ao".as_ptr(), c"null".as_ptr());
        (api.set)(h, c"config".as_ptr(), c"no".as_ptr());
        (api.set)(h, c"vid".as_ptr(), c"no".as_ptr());
        assert!((api.init)(h) >= 0);
    }
    let dir = tempfile::tempdir()?;
    let loud = dir.path().join("loud.wav");
    let quiet = dir.path().join("quiet.wav");
    tone(&loud, 16000.);
    tone(&quiet, 2000.);
    let mut fx = Loudness {
        mode: Normalization::Track,
        ..Default::default()
    };
    let measure = |fx: &mut Loudness, path: &Path| -> anyhow::Result<f64> {
        fx.open(api, h, path.to_str().unwrap(), None, Gains::default())?;
        let end = Instant::now() + Duration::from_secs(8);
        while fx.waiting() {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(15));
            fx.poll(api, h)?;
        }
        Ok(api.number(h, "volume-gain").unwrap())
    };
    let a = measure(&mut fx, &loud)?;
    let b = measure(&mut fx, &quiet)?;
    assert!(
        (b - a - 18.06).abs() < 0.4,
        "measured track gains {a} and {b}"
    );
    fx.stop();
    fx.set_mode(api, h, Normalization::Album("album".into()))?;
    let album = measure(&mut fx, &loud)?;
    assert!((album - a).abs() < 0.01 && (measure(&mut fx, &quiet)? - album).abs() < 0.01);
    fx.set_mode(api, h, Normalization::Off)?;
    assert_eq!(api.number(h, "volume-gain"), Some(0.));
    let volume = api.number(h, "volume");
    let flat = Equalizer::default();
    let rock = Equalizer::PRESETS[2];
    let custom = Equalizer([2, 1, -3]);
    set_eq(api, h, flat, rock)?;
    set_eq(api, h, rock, custom)?;
    let af = api.text(h, "af").unwrap();
    assert_eq!(af.matches("music-eq").count(), 1, "{af}");
    assert!(af.contains("g=-3"), "{af}");
    set_eq(api, h, custom, flat)?;
    assert!(!api.text(h, "af").unwrap_or_default().contains("music-eq"));
    assert_eq!(api.number(h, "volume"), volume);
    if std::env::var_os("MUSIC_API_TEST_OUTPUTS").is_some() {
        let mut route = crate::output::Routing::new();
        let until = Instant::now() + Duration::from_secs(5);
        let devices = loop {
            if let Some(out) = route.refresh(api, h)?
                && !out.active.is_empty()
            {
                break out;
            }
            assert!(
                Instant::now() < until,
                "default audio endpoint was not resolved"
            );
            std::thread::sleep(Duration::from_millis(30));
        };
        let hardware: Vec<_> = devices
            .devices
            .into_iter()
            .filter(|d| d.name != "auto")
            .take(2)
            .collect();
        assert_eq!(
            hardware.len(),
            2,
            "this hardware check requires two outputs"
        );
        for device in hardware {
            route.select(device.name.clone());
            let selected = route.refresh(api, h)?.unwrap();
            assert_eq!(selected.active, device.name);
            assert_eq!(
                api.text(h, "audio-device").as_deref(),
                Some(device.name.as_str())
            );
        }
        route.select("auto".into());
        assert_eq!(route.refresh(api, h)?.unwrap().active, devices.active);
    }
    Ok(())
}
