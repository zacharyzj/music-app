use super::{Asset, RELEASES, Release, Update, latest, stable_version};
use anyhow::{Context, Result};
use reqwest::{
    blocking::Client,
    header::{ACCEPT, ACCEPT_LANGUAGE},
};
use semver::Version;

// The public release list and its attachment fragments are available to
// anonymous browsers even when the REST API's shared IP quota is exhausted.
fn tags(html: &str) -> impl Iterator<Item = (Version, String)> + '_ {
    html.split("data-release-anchor=\"release-")
        .skip(1)
        .filter_map(|section| {
            let (tag, section) = section.split_once('"')?;
            let header = section
                .split("</section>")
                .next()?
                .split("markdown-body")
                .next()?;
            if header
                .split(['<', '>'])
                .any(|text| text.trim().eq_ignore_ascii_case("Pre-release"))
            {
                return None;
            }
            Some((stable_version(tag)?, tag.to_owned()))
        })
}

fn attachments(tag: String, html: &str) -> Release {
    let path = format!("href=\"/zacharyzj/music-app/releases/download/{tag}/");
    let assets = html
        .split(&path)
        .skip(1)
        .filter_map(|s| s.split_once('"'))
        .map(|(name, _)| Asset {
            name: name.into(),
            state: "uploaded".into(),
        })
        .collect();
    Release {
        tag_name: tag,
        draft: false,
        prerelease: false,
        assets,
    }
}

fn read(client: &Client, url: String) -> Result<String> {
    Ok(client
        .get(url)
        .header(ACCEPT, "text/html")
        .header(ACCEPT_LANGUAGE, "en")
        .send()?
        .error_for_status()?
        .text()?)
}

pub(super) fn check(
    client: &Client,
    current: &Version,
    os: &str,
    arch: &str,
) -> Result<Option<Update>> {
    let mut published = Vec::new();
    for page in 1.. {
        let html = read(client, format!("{RELEASES}?page={page}"))?;
        published.extend(tags(&html));
        if !html.contains("rel=\"next\"") {
            break;
        }
    }
    published.sort_unstable_by(|(a, _), (b, _)| b.cmp_precedence(a));
    let mut releases = Vec::new();
    for (_, tag) in published {
        let html = read(client, format!("{RELEASES}/expanded_assets/{tag}"))?;
        let release = attachments(tag, &html);
        let enough = release
            .player(os, arch)
            .is_some_and(|u| u.direct || u.version.cmp_precedence(current).is_le());
        releases.push(release);
        if enough {
            break;
        }
    }
    latest(&releases, current, os, arch).context("无法从 GitHub 发布页确认播放器版本")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_stable_headings_and_only_the_tags_own_downloads() {
        let page = r#"<section data-release-anchor="release-plugins-v0.2.12"><div>Latest</div><div class="markdown-body"><p>Pre-release</p></div></section>
            <section data-release-anchor="release-v2.0.0"><span>Pre-release</span></section>
            <section data-release-anchor="release-v2.1.0-rc.1"></section>"#;
        let tags: Vec<_> = tags(page).collect();
        assert_eq!(tags.len(), 1);
        let assets = r#"<a href="/zacharyzj/music-app/releases/download/plugins-v0.2.12/netease.ypm.zip">plugin</a>
            <a href="/zacharyzj/music-app/releases/download/plugins-v0.2.12/MusicPlayer-windows-x64-0.2.12.zip">player</a>
            <a href="/other/music-app/releases/download/plugins-v0.2.12/MusicPlayer-macos-arm64.dmg">other</a>"#;
        let release = attachments(tags[0].1.clone(), assets);
        assert!(release.player("windows", "x86_64").unwrap().direct);
        assert!(!release.player("macos", "aarch64").unwrap().direct);
    }
}
