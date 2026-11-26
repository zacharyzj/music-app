use crate::package::{MAX_PACKAGE, Package, media_allowed};
use anyhow::{Context, Result, ensure};
use reqwest::{Url, blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::{Read, Write},
    time::Duration,
};
use tempfile::NamedTempFile;

pub const DEFAULT_URL: &str =
    "https://github.com/zacharyzj/music-app/releases/latest/download/plugins.json";
const INDEX_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default)]
    pub media: Vec<String>,
}
#[derive(Serialize, Deserialize)]
pub struct Catalog {
    pub schema: u8,
    pub plugins: Vec<Entry>,
}

pub fn checksum(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn parse_index(bytes: &[u8], base: &Url) -> Result<Vec<Entry>> {
    let mut index: Catalog = serde_json::from_slice(bytes).context("无法读取插件索引 JSON")?;
    ensure!(
        index.schema == 1 && index.plugins.len() <= 256,
        "不支持的插件索引版本或插件数量"
    );
    let mut ids = HashSet::new();
    for item in &mut index.plugins {
        ensure!(
            !item.id.is_empty()
                && !item.name.is_empty()
                && !item.version.is_empty()
                && ids.insert(item.id.clone()),
            "索引中的插件信息缺失或 ID 重复"
        );
        ensure!(
            item.sha256.len() == 64 && item.sha256.bytes().all(|c| c.is_ascii_hexdigit()),
            "插件索引缺少有效 SHA-256"
        );
        let url = base.join(&item.url)?;
        ensure!(media_allowed(url.as_str(), &[]), "索引含有不支持的插件地址");
        item.url = url.to_string();
    }
    Ok(index.plugins)
}

fn fetch(url: &Url, limit: usize) -> Result<Vec<u8>> {
    ensure!(media_allowed(url.as_str(), &[]), "请输入公开的 HTTPS 地址");
    let client = Client::builder()
        .timeout(Duration::from_secs(45))
        .user_agent(concat!("music-app/", env!("CARGO_PKG_VERSION")))
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 8 || !media_allowed(attempt.url().as_str(), &[]) {
                attempt.error("插件地址重定向无效")
            } else {
                attempt.follow()
            }
        }))
        .build()?;
    let response = client
        .get(url.clone())
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(reqwest::Error::without_url)?;
    ensure!(
        response.content_length().is_none_or(|n| n <= limit as u64),
        "在线插件或索引过大"
    );
    let mut data = Vec::new();
    response.take(limit as u64 + 1).read_to_end(&mut data)?;
    ensure!(data.len() <= limit, "在线插件或索引过大");
    Ok(data)
}

fn checked_package(bytes: &[u8], entry: Option<&Entry>) -> Result<NamedTempFile> {
    if let Some(e) = entry {
        ensure!(
            checksum(bytes).eq_ignore_ascii_case(&e.sha256),
            "插件 SHA-256 校验失败，请刷新列表"
        );
    }
    let manifest = Package::read(bytes)?.manifest;
    if let Some(e) = entry {
        ensure!(
            e.id == manifest.id
                && e.version == manifest.version
                && e.network == manifest.network
                && e.media == manifest.media,
            "插件身份、版本或权限与索引不符"
        );
    }
    let mut file = NamedTempFile::new()?;
    file.write_all(bytes)?;
    file.flush()?;
    Ok(file)
}

pub enum Fetched {
    List(Vec<Entry>),
    Package(NamedTempFile),
}
pub fn load(address: &str, entry: Option<&Entry>) -> Result<Fetched> {
    let url =
        Url::parse(entry.map_or(address.trim(), |e| &e.url)).context("请输入完整 HTTPS 地址")?;
    if entry.is_some() || url.path().to_ascii_lowercase().ends_with(".zip") {
        let bytes = fetch(&url, MAX_PACKAGE)?;
        Ok(Fetched::Package(checked_package(&bytes, entry)?))
    } else {
        Ok(Fetched::List(parse_index(
            &fetch(&url, INDEX_LIMIT)?,
            &url,
        )?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_index_checks_digest_identity_permissions_and_destinations() {
        let file = crate::bundle(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/bilibili-js"
        )))
        .unwrap();
        let m = crate::manifest(&file).unwrap();
        let base = Url::parse("https://github.com/example/music/releases/download/v1/plugins.json")
            .unwrap();
        let index = Catalog {
            schema: 1,
            plugins: vec![Entry {
                id: m.id,
                name: m.name,
                version: m.version,
                description: String::new(),
                url: "bilibili.ypm.zip".into(),
                sha256: checksum(&file),
                network: m.network,
                media: m.media,
            }],
        };
        let entries = parse_index(&serde_json::to_vec(&index).unwrap(), &base).unwrap();
        let mut entry = entries[0].clone();
        assert_eq!(
            entry.url,
            "https://github.com/example/music/releases/download/v1/bilibili.ypm.zip"
        );
        assert!(checked_package(&file, Some(&entry)).is_ok());
        assert!(checked_package(b"wrong", Some(&entry)).is_err());
        entry.network.clear();
        assert!(checked_package(&file, Some(&entry)).is_err());
        entry.url = "file:///invalid".into();
        assert!(
            parse_index(
                &serde_json::to_vec(&Catalog {
                    schema: 1,
                    plugins: vec![entry]
                })
                .unwrap(),
                &base
            )
            .is_err()
        );
    }
}
