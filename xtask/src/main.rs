use anyhow::{Result, bail};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("plugin") => {
            let output = std::path::Path::new(args.get(1).map_or("dist", String::as_str));
            std::fs::create_dir_all(output)?;
            use music_plugin_host::catalog::{Catalog, Entry, checksum};
            let base = std::env::var("MUSIC_PLUGIN_RELEASE_BASE").unwrap_or_else(|_| {
                format!(
                    "https://github.com/zacharyzj/music-app/releases/download/plugins-v{}",
                    env!("CARGO_PKG_VERSION")
                )
            });
            let mut plugins = Vec::new();
            for directory in [
                "plugins/netease-js",
                "plugins/incompetech-js",
                "plugins/audius-js",
                "plugins/bilibili-js",
                "plugins/24bit-js",
            ] {
                let bytes = music_plugin_host::bundle(std::path::Path::new(directory))?;
                let m = music_plugin_host::manifest(&bytes)?;
                music_plugin_host::Runtime::new().validate(&bytes)?;
                let path = output.join(format!("{}.ypm.zip", m.id));
                std::fs::write(&path, &bytes)?;
                println!(
                    "{} {}: {} ({} bytes)",
                    m.name,
                    m.version,
                    path.display(),
                    bytes.len()
                );
                let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(
                    std::path::Path::new(directory).join("plugin.json"),
                )?)?;
                plugins.push(Entry {
                    url: format!("{base}/{}.ypm.zip", m.id),
                    sha256: checksum(&bytes),
                    id: m.id,
                    name: m.name,
                    version: m.version,
                    description: raw["description"].as_str().unwrap_or_default().into(),
                    network: m.network,
                    media: m.media,
                });
            }
            std::fs::write(
                output.join("plugins.json"),
                serde_json::to_vec_pretty(&Catalog { schema: 1, plugins })?,
            )?;
        }
        Some("probe") => {
            let source = args
                .windows(2)
                .find(|a| a[0] == "--source")
                .map_or("netease", |a| a[1].as_str());
            anyhow::ensure!(
                !source.is_empty()
                    && source
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
                "Invalid source ID"
            );
            let bytes = std::fs::read(format!("dist/{source}.ypm.zip"))?;
            let root = std::env::temp_dir().join("music-api-probe");
            let rt = music_plugin_host::Runtime::new();
            let query = args.get(1).map(String::as_str).unwrap_or("晴天");
            let p = rt.call(
                &bytes,
                &root,
                music_core::Request::new("search", serde_json::json!({"query":query})),
            )?;
            println!(
                "Search: {} results; more={}",
                p["items"].as_array().map_or(0, Vec::len),
                p["more"]
            );
            if args.iter().any(|s| s == "--play") {
                let id = &p["items"][0]["key"]["id"];
                let stream = rt.call(
                    &bytes,
                    &root,
                    music_core::Request::new(
                        "stream",
                        serde_json::json!({"id":id,"quality":"standard"}),
                    ),
                )?;
                println!("Stream resolved: quality={}", stream["quality"]);
                playback(
                    stream["url"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Missing stream URL"))?,
                    stream["referrer"].as_str(),
                )?;
            }
        }
        Some("probe-account") => {
            let root = std::path::Path::new(
                args.get(1)
                    .ok_or_else(|| anyhow::anyhow!("Missing application data directory"))?,
            );
            // Work on a temporary encrypted copy. Never update the running app's session.
            let scratch = tempfile::tempdir()?;
            std::fs::copy(
                root.join("plugin-data/netease.enc"),
                scratch.path().join("netease.enc"),
            )?;
            let bytes = std::fs::read("dist/netease.ypm.zip")?;
            let rt = music_plugin_host::Runtime::new();
            if args.iter().any(|arg| arg == "--heartbeat") {
                use music_core::{Page, Request};
                use serde_json::json;
                let call = |op, args| rt.call(&bytes, scratch.path(), Request::new(op, args));
                let liked = call("liked", json!({}))?;
                let playlist = liked["heartbeatPlaylistId"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Favorite playlist unavailable"))?;
                let page: Page = serde_json::from_value(liked.clone())?;
                let mut current = page
                    .items
                    .into_iter()
                    .find(|t| !t.unavailable)
                    .ok_or_else(|| anyhow::anyhow!("No playable favorite"))?
                    .key
                    .id;
                let mut recent = vec![current.clone()];
                for batch in 1..=2 {
                    let page: Page = serde_json::from_value(call(
                        "heartbeat",
                        json!({"playlistId":playlist, "currentId":current, "recent":recent}),
                    )?)?;
                    let next = page
                        .items
                        .first()
                        .ok_or_else(|| anyhow::anyhow!("Empty heartbeat batch"))?;
                    let ids: Vec<_> = page.items.iter().map(|t| &t.key.id).collect();
                    let membership = call("isLiked", json!({"ids":ids}))?;
                    anyhow::ensure!(
                        page.items.iter().all(|t| membership[&t.key.id].as_bool()
                            == Some(!t.heartbeat_recommendation)),
                        "Recommendation markers disagree with favorite membership"
                    );
                    let recommended = page
                        .items
                        .iter()
                        .filter(|t| t.heartbeat_recommendation)
                        .count();
                    println!(
                        "Heartbeat batch {batch}: {} favorites, {recommended} recommendations",
                        page.items.len() - recommended
                    );
                    current = next.key.id.clone();
                    recent.extend(ids.into_iter().cloned());
                }
                return Ok(());
            }
            let mut failed = false;
            for page in ["daily", "cloud"] {
                let started = std::time::Instant::now();
                match rt.call(
                    &bytes,
                    scratch.path(),
                    music_core::Request::new("browse", serde_json::json!({"page":page})),
                ) {
                    Ok(p) => {
                        println!(
                            "{page}: {} items, navigation_links={}, elapsed={:.2}s",
                            p["items"].as_array().map_or(0, Vec::len),
                            p["links"].as_array().map_or(0, Vec::len),
                            started.elapsed().as_secs_f64()
                        );
                        if page == "cloud"
                            && let Some(next) = p["links"].as_array().and_then(|links| {
                                links.iter().find(|link| link["title"] == "下一页")
                            })
                        {
                            let page = rt.call(
                                &bytes,
                                scratch.path(),
                                music_core::Request::new("browse", next["args"].clone()),
                            )?;
                            println!(
                                "cloud-next: {} items, navigation_links={}",
                                page["items"].as_array().map_or(0, Vec::len),
                                page["links"].as_array().map_or(0, Vec::len)
                            );
                        }
                    }
                    Err(e) => {
                        failed = true;
                        eprintln!(
                            "{page}: {e:#}, elapsed={:.2}s",
                            started.elapsed().as_secs_f64()
                        );
                    }
                }
            }
            anyhow::ensure!(!failed, "Account browse probe failed");
        }
        Some("play") => playback(
            args.get(1)
                .ok_or_else(|| anyhow::anyhow!("Missing file path"))?,
            None,
        )?,
        _ => bail!(
            "Usage: cargo xtask plugin [output-dir] | probe [query] [--source id] [--play] | probe-account <app-data-dir> | play <file>"
        ),
    };
    Ok(())
}
fn playback(url: &str, referrer: Option<&str>) -> Result<()> {
    use music_player::{Command, Player, PlayerEvent};
    use std::time::{Duration, Instant};
    let path = std::env::var_os("MUSIC_API_MPV")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "dist/native/mpv-2.dll".into());
    let player = Player::start(Some(&path));
    player.commands.send(Command::Volume(10.0))?;
    player.commands.send(Command::Open {
        url: url.into(),
        referrer: referrer.map(str::to_owned),
        video: None,
        gains: Default::default(),
    })?;
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut paused = false;
    let mut success = false;
    while Instant::now() < deadline {
        match player.events.recv_timeout(Duration::from_secs(2)) {
            Ok(PlayerEvent::Error(e)) => {
                let _ = player.commands.send(Command::Quit);
                bail!("{e}")
            }
            Ok(PlayerEvent::Position(p, d, _)) if p > 0.2 && d > 0.0 => {
                if !paused {
                    player.commands.send(Command::Pause(true))?;
                    player.commands.send(Command::Seek(1.0))?;
                    player.commands.send(Command::Pause(false))?;
                    paused = true;
                } else if p > 1.1 {
                    println!(
                        "Playback verified: position={p:.2}, duration={d:.2}, pause/seek/resume sent"
                    );
                    success = true;
                    break;
                }
            }
            _ => {}
        }
    }
    let _ = player.commands.send(Command::Stop);
    let _ = player.commands.send(Command::Quit);
    anyhow::ensure!(success, "Playback did not advance within 25 seconds");
    Ok(())
}
