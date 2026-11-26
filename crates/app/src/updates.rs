use crate::AppWindow;
use anyhow::{Context, Result, bail};
use reqwest::{StatusCode, blocking::Client, header::ACCEPT};
use semver::Version;
use serde::Deserialize;
use slint::ComponentHandle;
use std::{env::consts, thread, time::Duration};

const CURRENT: &str = env!("CARGO_PKG_VERSION");
const API: &str = "https://api.github.com/repos/zacharyzj/music-app/releases";
const RELEASES: &str = "https://github.com/zacharyzj/music-app/releases";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    state: String,
}

struct Update {
    version: Version,
    url: String,
    direct: bool,
}

mod public_page;

fn stable_version(tag: &str) -> Option<Version> {
    let number = ["plugins-v", "player-v", "v"]
        .iter()
        .find_map(|prefix| tag.strip_prefix(prefix))
        .unwrap_or(tag);
    Version::parse(number).ok().filter(|v| v.pre.is_empty())
}

// The Windows packager also supports an unversioned filename. Only an APK
// without an ABI or an explicitly universal package can match multiple CPUs.
fn package_target<'a>(name: &'a str, version: &str) -> Option<(&'a str, &'static str)> {
    let name = name.strip_prefix("musicplayer-")?;
    let os = name.split(['-', '.']).next()?;
    let extensions: &[&str] = match os {
        "windows" => &[".zip", ".msi", ".exe"],
        "macos" => &[".dmg", ".pkg", ".zip"],
        "linux" => &[".appimage", ".tar.gz", ".tar.xz"],
        "android" => &[".apk"],
        _ => return None,
    };
    let stem = extensions.iter().find_map(|ext| name.strip_suffix(ext))?;
    let stem = stem
        .strip_suffix(version)
        .and_then(|s| s.strip_suffix('-'))
        .unwrap_or(stem);
    let arch = match stem.strip_prefix(os)?.strip_prefix('-') {
        Some("x64" | "amd64" | "x86_64") => "x86_64",
        Some("arm64" | "aarch64" | "arm64-v8a") => "aarch64",
        Some("x86" | "i686") => "x86",
        Some("armv7" | "armeabi-v7a") => "arm",
        Some("universal") if matches!(os, "macos" | "android") => "universal",
        None if stem == "android" => "universal",
        _ => return None,
    };
    Some((os, arch))
}

impl Release {
    fn player(&self, os: &str, arch: &str) -> Option<Update> {
        if self.draft || self.prerelease {
            return None;
        }
        let tag = self.tag_name.as_str();
        let version = stable_version(tag)?;
        let normalized = version.to_string().to_ascii_lowercase();
        let (rank, asset) = self
            .assets
            .iter()
            .filter(|asset| asset.state == "uploaded")
            .filter_map(|asset| {
                let name = asset.name.to_ascii_lowercase();
                let (asset_os, asset_arch) = package_target(&name, &normalized)?;
                let rank = match (asset_os == os, asset_arch) {
                    (true, cpu) if cpu == arch => 2,
                    (true, "universal") => 1,
                    _ => 0,
                };
                Some((rank, asset))
            })
            .max_by_key(|(rank, _)| *rank)?;
        let direct = rank > 0;
        // The tag and filename above are taken from an uploaded player asset
        // in this repository. Plugin-only releases never reach this point.
        let url = if direct {
            format!("{RELEASES}/download/{tag}/{}", asset.name)
        } else {
            format!("{RELEASES}/tag/{tag}")
        };
        Some(Update {
            version,
            url,
            direct,
        })
    }
}

fn latest(releases: &[Release], current: &Version, os: &str, arch: &str) -> Result<Option<Update>> {
    let mut players = releases
        .iter()
        .filter_map(|r| r.player(os, arch))
        .peekable();
    if players.peek().is_none() {
        bail!("GitHub 上暂时没有播放器正式版");
    }
    Ok(players
        .filter(|u| u.version.cmp_precedence(current).is_gt())
        .max_by(|a, b| {
            a.direct
                .cmp(&b.direct)
                .then_with(|| a.version.cmp_precedence(&b.version))
        }))
}

fn check() -> Result<Option<Update>> {
    let current = Version::parse(CURRENT)?;
    let client = Client::builder()
        .user_agent(concat!("music-app/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(12))
        .build()?;
    match api_releases(&client) {
        Ok(releases) => latest(&releases, &current, consts::OS, consts::ARCH),
        Err(api_error) => {
            public_page::check(&client, &current, consts::OS, consts::ARCH).map_err(|_| api_error)
        }
    }
}

fn api_releases(client: &Client) -> Result<Vec<Release>> {
    let mut releases = Vec::new();
    for page in 1.. {
        let response = client
            .get(API)
            .header(ACCEPT, "application/vnd.github+json")
            .query(&[("per_page", 50), ("page", page)])
            .send()
            .context("无法连接 GitHub，请检查网络后重试")?;
        if matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
        ) {
            bail!("GitHub 暂时限制了请求，请稍后重试");
        }
        let response = response
            .error_for_status()
            .context("GitHub 暂时无法提供发布列表")?;
        let batch: Vec<Release> = serde_json::from_reader(std::io::BufReader::new(response))
            .context("无法读取 GitHub 发布列表")?;
        let last = batch.len() < 50;
        releases.extend(batch);
        if last {
            break;
        }
    }
    Ok(releases)
}

fn platform_name() -> String {
    let os = match consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        "android" => "Android",
        os => os,
    };
    let arch = match consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "ARM64",
        arch => arch,
    };
    format!("{os} {arch}")
}

fn present(ui: &AppWindow, result: Result<Option<Update>>) {
    ui.set_update_busy(false);
    match result {
        Ok(Some(update)) => {
            if ui.get_update_url().as_str() != update.url {
                ui.set_show_update_notice(true);
            }
            let platform = platform_name();
            let status = if update.direct {
                format!("发现 {}，可下载 {platform} 版", update.version)
            } else {
                format!("发现 {}，此版本暂无 {platform} 安装包", update.version)
            };
            ui.set_update_status(status.into());
            ui.set_update_direct(update.direct);
            ui.set_update_url(update.url.into());
        }
        Ok(None) => {
            ui.set_update_status("当前已是最新正式版".into());
            ui.set_update_url("".into());
            ui.set_show_update_notice(false);
        }
        Err(e) => {
            let message = format!("检查更新失败 · {e}");
            if ui.get_update_url().is_empty() {
                ui.set_update_status(message.into());
            } else {
                ui.invoke_notify_row(message.into());
            }
        }
    }
}

pub(super) fn install(ui: &AppWindow) {
    ui.set_player_version(CURRENT.into());
    let window = ui.as_weak();
    ui.on_check_update(move || {
        let Some(ui) = window.upgrade() else { return };
        if ui.get_update_busy() {
            return;
        }
        ui.set_update_busy(true);
        let window = ui.as_weak();
        thread::spawn(move || {
            let result = check();
            let _ = window.upgrade_in_event_loop(move |ui| present(&ui, result));
        });
    });
    let window = ui.as_weak();
    ui.on_open_update(move || {
        let Some(ui) = window.upgrade() else { return };
        let url = ui.get_update_url();
        if url.is_empty() {
            return;
        }
        match webbrowser::open(&url) {
            Ok(()) => ui.set_show_update_notice(false),
            Err(_) => ui.invoke_notify_row("无法打开系统浏览器，请检查默认浏览器设置后重试".into()),
        }
    });
    ui.invoke_check_update();
}

#[cfg(test)]
mod tests;
