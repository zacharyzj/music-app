use anyhow::{Result, bail};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("plugin") => {
            let bytes = music_plugin_host::bundle(std::path::Path::new("plugins/netease-js"))?;
            let m = music_plugin_host::manifest(&bytes)?;
            music_plugin_host::Runtime::new().validate(&bytes)?;
            std::fs::create_dir_all("dist")?;
            std::fs::write("dist/netease.ypm.zip", &bytes)?;
            println!(
                "{} {}: dist/netease.ypm.zip ({} bytes)",
                m.name,
                m.version,
                bytes.len()
            );
        }
        Some("probe") => {
            let bytes = std::fs::read("dist/netease.ypm.zip")?;
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
            if args.get(2).is_some_and(|s| s == "--play") {
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
        )?,
        _ => bail!(
            "Usage: cargo xtask plugin | probe [query] [--play] | probe-account <app-data-dir> | play <file>"
        ),
    };
    Ok(())
}
fn playback(url: &str) -> Result<()> {
    use music_player::{Command, Player, PlayerEvent};
    use std::time::{Duration, Instant};
    let path = std::env::var_os("MUSIC_API_MPV")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "dist/native/mpv-2.dll".into());
    let player = Player::start(Some(&path));
    player.commands.send(Command::Volume(10.0))?;
    player.commands.send(Command::Open(url.into()))?;
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut paused = false;
    let mut success = false;
    while Instant::now() < deadline {
        match player.events.recv_timeout(Duration::from_secs(2)) {
            Ok(PlayerEvent::Error(e)) => {
                let _ = player.commands.send(Command::Quit);
                bail!("{e}")
            }
            Ok(PlayerEvent::Position(p, d)) if p > 0.2 && d > 0.0 => {
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
