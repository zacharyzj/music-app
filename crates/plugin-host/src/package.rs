use anyhow::{Result, ensure};
use music_core::{Manifest, Navigation, Setting};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Write},
    path::Path,
};

pub(crate) const MAX_PACKAGE: usize = 32 * 1024 * 1024;
#[derive(Clone)]
pub struct Package {
    pub manifest: Manifest,
    pub main: String,
    pub files: BTreeMap<String, Vec<u8>>,
}
pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', ':', '\0'])
        && !name.starts_with('/')
        && name
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
}
impl Package {
    pub fn read(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= MAX_PACKAGE, "插件包超过 32 MiB");
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
        ensure!(zip.len() <= 256, "插件文件过多");
        let mut files = BTreeMap::new();
        let mut total = 0usize;
        for i in 0..zip.len() {
            let f = zip.by_index(i)?;
            if f.is_dir() {
                continue;
            }
            let name = f.name().to_owned();
            ensure!(safe_name(&name), "插件路径不安全");
            ensure!(
                f.unix_mode().is_none_or(|m| m & 0o170000 != 0o120000),
                "插件不允许符号链接"
            );
            ensure!(f.size() <= 4 * 1024 * 1024, "单个插件文件过大");
            let mut data = vec![];
            f.take(4 * 1024 * 1024 + 1).read_to_end(&mut data)?;
            total += data.len();
            ensure!(
                data.len() <= 4 * 1024 * 1024 && total <= MAX_PACKAGE,
                "插件解压大小超过限制"
            );
            ensure!(files.insert(name, data).is_none(), "重复插件文件");
        }
        let manifests: Vec<_> = files
            .keys()
            .filter(|n| *n == "plugin.json" || n.ends_with("/plugin.json"))
            .cloned()
            .collect();
        ensure!(manifests.len() == 1, "插件必须包含唯一 plugin.json");
        let prefix = manifests[0].strip_suffix("plugin.json").unwrap();
        if !prefix.is_empty() {
            files = files
                .into_iter()
                .filter_map(|(name, bytes)| {
                    name.strip_prefix(prefix).map(|s| (s.to_owned(), bytes))
                })
                .collect();
        }
        let raw: Value = serde_json::from_slice(&files["plugin.json"])?;
        let id = raw["id"].as_str().unwrap_or("");
        ensure!(
            !id.is_empty()
                && id != "local"
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b)),
            "插件 ID 无效"
        );
        let main = raw["main"].as_str().unwrap_or("index.js").to_owned();
        ensure!(
            safe_name(&main) && files.contains_key(&main),
            "插件入口不存在"
        );
        let network = strings(&raw["permissions"]["network"]);
        let media = strings(&raw["permissions"]["media"]);
        let mut navigation = vec![Navigation {
            title: "为你推荐".into(),
            op: "home".into(),
            args: json!({}),
        }];
        if let Some(nav) = raw["navigation"].as_array() {
            for n in nav {
                if let Some(page) = n["page"].as_str() {
                    navigation.push(Navigation {
                        title: n["title"].as_str().unwrap_or(page).into(),
                        op: "browse".into(),
                        args: json!({"page":page}),
                    });
                } else if let Some(op) = n["builtin"].as_str()
                    && op != "home"
                {
                    navigation.push(Navigation {
                        title: match op {
                            "liked" => "喜欢的歌曲",
                            "savedAlbums" => "收藏专辑",
                            "playlists" => "我的歌单",
                            "recent" => "最近播放",
                            _ => op,
                        }
                        .into(),
                        op: op.into(),
                        args: json!({}),
                    });
                }
            }
        }
        let settings = raw["settings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| {
                Some(Setting {
                    key: s["key"].as_str()?.into(),
                    label: s["label"].as_str().unwrap_or("").into(),
                    default: s["default"].as_str().unwrap_or("").into(),
                    options: s["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|o| o.as_str().or(o["value"].as_str()).map(str::to_owned))
                        .collect(),
                })
            })
            .collect();
        let manifest = Manifest {
            id: id.into(),
            name: raw["name"].as_str().unwrap_or(id).into(),
            version: raw["version"].as_str().unwrap_or("0").into(),
            abi: 2,
            capabilities: {
                let mut caps = vec!["settings".into()];
                caps.extend(strings(&raw["capabilities"]).into_iter().filter(|c| {
                    matches!(
                        c.as_str(),
                        "download"
                            | "anonymous"
                            | "tracksOnly"
                            | "readOnly"
                            | "video"
                            | "heartbeat"
                    )
                }));
                caps
            },
            search_page_size: serde_json::from_value(raw["searchPageSize"].clone())?,
            network,
            media,
            navigation,
            settings,
        };
        Ok(Self {
            manifest,
            main,
            files,
        })
    }
}
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}
pub fn manifest(bytes: &[u8]) -> Result<Manifest> {
    Ok(Package::read(bytes)?.manifest)
}
pub fn bundle(dir: &Path) -> Result<Vec<u8>> {
    fn add(root: &Path, dir: &Path, out: &mut zip::ZipWriter<Cursor<Vec<u8>>>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            ensure!(!kind.is_symlink(), "插件不允许符号链接");
            if kind.is_dir() {
                add(root, &entry.path(), out)?;
            } else if kind.is_file() {
                let path = entry.path();
                let name = path
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                ensure!(safe_name(&name), "插件路径无效");
                out.start_file(
                    name,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )?;
                out.write_all(&std::fs::read(path)?)?;
            }
        }
        Ok(())
    }
    let mut out = zip::ZipWriter::new(Cursor::new(vec![]));
    add(dir, dir, &mut out)?;
    let bytes = out.finish()?.into_inner();
    Package::read(&bytes)?;
    Ok(bytes)
}
pub fn install(file: &Path, dir: &Path) -> Result<Manifest> {
    install_with(file, dir, |_| {})
}

pub fn install_with(
    file: &Path,
    dir: &Path,
    before_replace: impl FnOnce(&Manifest),
) -> Result<Manifest> {
    let bytes = if file.is_dir() {
        bundle(file)?
    } else {
        std::fs::read(file)?
    };
    let m = manifest(&bytes)?;
    before_replace(&m);
    std::fs::create_dir_all(dir)?;
    let target = dir.join(format!("{}.ypm.zip", m.id));
    if target.exists() {
        std::fs::copy(&target, dir.join(format!("{}.previous", m.id)))?;
    }
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(&target)?;
    Ok(m)
}
pub fn allowed(url: &str, patterns: &[String]) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some_and(|p| p != 443)
    {
        return false;
    }
    let Some(host) = u.host_str() else {
        return false;
    };
    patterns.iter().any(|p| {
        p == host
            || p.strip_prefix("*.")
                .is_some_and(|s| host.ends_with(&format!(".{s}")))
    })
}
pub fn media_allowed(url: &str, patterns: &[String]) -> bool {
    // YPM manifests normally only declare API hosts. Media URLs still require
    // HTTPS and cannot point to local/private literal IPs or localhost.
    if !patterns.is_empty() {
        return allowed(url, patterns);
    }
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some_and(|p| p != 443)
    {
        return false;
    }
    matches!(u.host(), Some(url::Host::Domain(h)) if h.contains('.') && !h.ends_with(".localhost") && !h.ends_with(".local"))
}
