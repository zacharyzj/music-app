use super::*;

fn release(tag: &str, assets: &[&str]) -> Release {
    Release {
        tag_name: tag.into(),
        draft: false,
        prerelease: false,
        assets: assets
            .iter()
            .map(|name| Asset {
                name: (*name).into(),
                state: "uploaded".into(),
            })
            .collect(),
    }
}

#[test]
fn compares_player_versions_independently_of_plugins_and_release_order() -> Result<()> {
    let mut draft = release("v2.0.0", &["MusicPlayer-windows-x64.zip"]);
    draft.draft = true;
    let mut preview = release("v3.0.0", &["MusicPlayer-windows-x64.zip"]);
    preview.prerelease = true;
    let mut pending = release("v4.0.0", &["MusicPlayer-windows-x64.zip"]);
    pending.assets[0].state = "starter".into();
    let releases = [
        release("plugins-v9.0.0", &["plugins.json", "netease.ypm.zip"]),
        release("plugins-v0.2.9", &["MusicPlayer-windows-x64-0.2.9.zip"]),
        release(
            "plugins-v0.2.12",
            &["netease.ypm.zip", "MusicPlayer-windows-x64-0.2.12.zip"],
        ),
        release("v0.2.11", &["MusicPlayer-windows-x64.zip"]),
        release("v4.0.1-rc.1", &["MusicPlayer-windows-x64.zip"]),
        release(
            "v4.0.2",
            &[
                "MusicPlayer-windows-x64-0.2.12.zip",
                "MusicPlayer-windows-x64-symbols.zip",
                "MusicPlayer-android.debug.apk",
            ],
        ),
        release("v0.2.12+another", &["MusicPlayer-linux-x64.AppImage"]),
        draft,
        preview,
        pending,
    ];
    let update = latest(&releases, &Version::parse("0.2.9")?, "windows", "x86_64")?.unwrap();
    assert!(update.direct);
    assert_eq!(
        update.url,
        format!("{RELEASES}/download/plugins-v0.2.12/MusicPlayer-windows-x64-0.2.12.zip")
    );
    for current in ["0.2.12", "0.2.12+local", "1.0.0"] {
        assert!(latest(&releases, &Version::parse(current)?, "windows", "x86_64")?.is_none());
    }
    assert!(latest(&releases[..1], &Version::new(0, 0, 0), "windows", "x86_64").is_err());
    Ok(())
}

#[test]
fn selects_an_uploaded_package_for_the_os_and_cpu_or_the_real_release_page() -> Result<()> {
    let releases = [
        release(
            "v1.1.0",
            &[
                "MusicPlayer-android-1.1.0.apk",
                "MusicPlayer-macos-universal.dmg",
                "MusicPlayer-macos-x64-1.1.0.dmg",
                "MusicPlayer-android-arm64-v8a-1.1.0.apk",
                "MusicPlayer-linux-aarch64.tar.gz",
                "MusicPlayer-windows-arm64.zip",
            ],
        ),
        release("v1.2.0", &["MusicPlayer-windows-x64.zip"]),
    ];
    let old = Version::parse("1.0.0")?;
    for (os, arch, expected) in [
        ("windows", "x86_64", "MusicPlayer-windows-x64.zip"),
        ("windows", "aarch64", "MusicPlayer-windows-arm64.zip"),
        ("macos", "aarch64", "MusicPlayer-macos-universal.dmg"),
        ("macos", "x86_64", "MusicPlayer-macos-x64-1.1.0.dmg"),
        ("linux", "aarch64", "MusicPlayer-linux-aarch64.tar.gz"),
        (
            "android",
            "aarch64",
            "MusicPlayer-android-arm64-v8a-1.1.0.apk",
        ),
        ("android", "x86_64", "MusicPlayer-android-1.1.0.apk"),
    ] {
        let update = latest(&releases, &old, os, arch)?.unwrap();
        assert!(
            update.direct && update.url.ends_with(&format!("/{expected}")),
            "{}",
            update.url
        );
    }
    let fallback = latest(&releases, &old, "linux", "x86_64")?.unwrap();
    assert!(!fallback.direct);
    assert_eq!(fallback.url, format!("{RELEASES}/tag/v1.2.0"));
    Ok(())
}

#[cfg(feature = "ui-smoke")]
#[test]
fn banner_can_be_dismissed_and_the_settings_keep_the_download_and_retry_controls() -> Result<()> {
    use crate::test_ui::{click, test_clock};
    use slint::PhysicalSize;
    use std::{cell::Cell, rc::Rc};

    let _clock = test_clock()?;
    let ui = AppWindow::new()?;
    ui.show()?;
    ui.window().set_size(PhysicalSize::new(720, 480));
    let opened = Rc::new(Cell::new(0));
    let count = opened.clone();
    ui.on_open_update(move || count.set(count.get() + 1));
    let checks = Rc::new(Cell::new(0));
    let count = checks.clone();
    ui.on_check_update(move || count.set(count.get() + 1));

    let new = || {
        release("plugins-v0.2.12", &["MusicPlayer-windows-x64-0.2.12.zip"])
            .player("windows", "x86_64")
    };
    present(&ui, Ok(new()));
    assert!(ui.get_show_update_notice() && ui.get_update_direct());
    click(ui.window(), 620., 87.);
    assert_eq!(opened.get(), 1);
    click(ui.window(), 678., 87.);
    assert!(!ui.get_show_update_notice());
    present(&ui, Ok(new()));
    assert!(!ui.get_show_update_notice());
    present(&ui, Err(anyhow::anyhow!("离线")));
    assert!(
        ui.get_update_url()
            .ends_with("MusicPlayer-windows-x64-0.2.12.zip")
    );
    ui.set_show_settings(true);
    click(ui.window(), 625., 172.);
    click(ui.window(), 527., 172.);
    assert_eq!((opened.get(), checks.get()), (2, 1));
    present(&ui, Ok(None));
    assert!(ui.get_update_url().is_empty() && !ui.get_show_update_notice());
    present(&ui, Err(anyhow::anyhow!("离线")));
    assert!(ui.get_update_status().contains("离线") && !ui.get_show_update_notice());
    Ok(())
}
