use aes::cipher::{BlockEncryptMut, KeyInit, block_padding::Pkcs7};
use music_core::*;
use music_plugin_sdk::host;
use serde_json::{Value, json};
const MANIFEST_BYTES: &[u8] = include_bytes!("../manifest.json");
#[used]
#[cfg_attr(target_arch = "wasm32", unsafe(link_section = "music_manifest"))]
static MANIFEST: [u8; MANIFEST_BYTES.len()] = *include_bytes!("../manifest.json");
#[unsafe(no_mangle)]
/// # Safety
/// The host must supply a readable guest buffer allocated by music_alloc.
pub unsafe extern "C" fn music_call(ptr: u32, len: u32) -> u64 {
    unsafe { music_plugin_sdk::dispatch(ptr, len, handle) }
}
fn err(code: &str, s: impl Into<String>) -> SourceError {
    SourceError::new(code, s)
}
fn get(k: &str) -> Result<Value, SourceError> {
    host(json!({"op":"get","key":k}))
}
fn set(k: &str, v: Value) -> Result<(), SourceError> {
    host(json!({"op":"set","key":k,"value":v}))?;
    Ok(())
}
fn now() -> u64 {
    host(json!({"op":"time"}))
        .ok()
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}
fn strval(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_else(|| {
        if v.is_number() {
            v.to_string()
        } else {
            String::new()
        }
    })
}
fn text(v: &Value, k: &str) -> String {
    strval(&v[k])
}
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
fn checked(v: &Value) -> Result<String, SourceError> {
    let s = strval(v);
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        Err(err("unavailable", "无效资源编号"))
    } else {
        Ok(s)
    }
}
pub fn encrypt(path: &str, text: &str) -> String {
    let digest = format!(
        "{:x}",
        md5::compute(format!("nobody{path}use{text}md5forencrypt"))
    );
    let input = format!("{path}-36cd479b6b5-{text}-36cd479b6b5-{digest}");
    hex::encode_upper(
        ecb::Encryptor::<aes::Aes128>::new(b"e82ckenh8dichen8".into())
            .encrypt_padded_vec_mut::<Pkcs7>(input.as_bytes()),
    )
}
fn api_with(
    path: &str,
    mut data: Value,
    mut cookies: Value,
    raw: bool,
) -> Result<(Value, Value), SourceError> {
    let mut device = get("device")?;
    if !device.is_string() {
        device = json!(format!(
            "{:x}",
            md5::compute(format!("{}{:?}", now(), host(json!({"op":"random"}))))
        ));
        set("device", device.clone())?;
    }
    let mut header = json!({"os":"pc","appver":"3.1.11","deviceId":device,"versioncode":"140","osver":"Microsoft-Windows-10-Professional-build-19045-64bit","channel":"netease","resolution":"1920x1080","__csrf":"","requestId":format!("{}_1",now())});
    if let Some(j) = cookies.as_object() {
        for (k, v) in j {
            header[k] = v.clone();
        }
    }
    if !cookies.is_object() {
        cookies = json!({});
    }
    data["header"] = header.clone();
    let params = encrypt(path, &data.to_string());
    let body = form_urlencoded::Serializer::new(String::new())
        .append_pair("params", &params)
        .finish();
    let cookie = header
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| format!("{k}={}", strval(v)))
        .collect::<Vec<_>>()
        .join("; ");
    let response = host(
        json!({"op":"http","url":format!("https://interfacepc.music.163.com{}",path.replacen("/api/","/eapi/",1)),"headers":{"Content-Type":"application/x-www-form-urlencoded","Referer":"https://music.163.com/","User-Agent":"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/124.0.0.0 Safari/537.36","Cookie":cookie},"body":body}),
    )?;
    if response["status"] != 200 {
        return Err(err(
            "network",
            format!("网易云 HTTP {}", response["status"]),
        ));
    }
    let result: Value = serde_json::from_str(response["body"].as_str().unwrap_or(""))
        .map_err(|_| err("protocol", "网易云返回了无效响应"))?;
    let code = result["code"].as_i64().unwrap_or(200);
    if !raw && code != 200 {
        return Err(err(
            if [301, 302, 401].contains(&code) {
                "signIn"
            } else if [-462, 405].contains(&code) {
                "verification"
            } else {
                "failed"
            },
            format!("网易云请求失败 {code}：{}", text(&result, "message")),
        ));
    }
    for c in array(&response["cookies"]) {
        if let Some((k, v)) = c
            .as_str()
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .split_once('=')
        {
            cookies[k] = json!(v);
        }
    }
    Ok((result, cookies))
}
fn api(path: &str, data: Value) -> Result<Value, SourceError> {
    let (j, c) = api_with(path, data, get("cookies")?, false)?;
    set("cookies", c)?;
    Ok(j)
}
fn account() -> Result<Value, SourceError> {
    let v = get("account")?;
    if v["id"].is_null() {
        Err(err("signIn", "请先扫码登录"))
    } else {
        Ok(v)
    }
}
fn item(x: &Value, kind: &str) -> Item {
    let album = if x["al"].is_object() {
        &x["al"]
    } else {
        &x["album"]
    };
    let artists = if x["ar"].is_array() {
        &x["ar"]
    } else {
        &x["artists"]
    };
    let artist = array(artists)
        .iter()
        .map(|a| text(a, "name"))
        .collect::<Vec<_>>()
        .join(" / ");
    let cover = [
        &x["picUrl"],
        &x["coverImgUrl"],
        &x["img1v1Url"],
        &album["picUrl"],
    ]
    .into_iter()
    .find_map(Value::as_str)
    .unwrap_or("")
    .replacen("http://", "https://", 1);
    Item {
        key: ResourceKey {
            source: "netease".into(),
            id: strval(&x["id"]),
        },
        kind: kind.into(),
        title: text(x, "name"),
        subtitle: if artist.is_empty() {
            text(&x["creator"], "nickname")
        } else {
            artist.clone()
        },
        cover,
        duration_ms: x["dt"].as_u64().or(x["duration"].as_u64()).unwrap_or(0),
        album: text(album, "name"),
        artist,
        unavailable: x["privilege"]["st"].as_i64().is_some_and(|n| n < 0)
            || x["noCopyrightRcmd"].is_object(),
    }
}
fn page(title: &str, values: &Value, kind: &str, offset: u64, more: bool) -> Value {
    json!(Page {
        title: title.into(),
        items: array(values).iter().map(|v| item(v, kind)).collect(),
        offset,
        more
    })
}
fn details(ids: &[Value]) -> Result<Vec<Item>, SourceError> {
    let mut all = Vec::new();
    for batch in ids.chunks(500) {
        let c = batch
            .iter()
            .map(|x| json!({"id":strval(x)}))
            .collect::<Vec<_>>();
        let j = api(
            "/api/v3/song/detail",
            json!({"c":serde_json::to_string(&c).unwrap()}),
        )?;
        let items: Vec<_> = array(&j["songs"])
            .iter()
            .map(|v| item(v, "track"))
            .collect();
        for id in batch {
            let id = strval(id);
            all.push(
                items
                    .iter()
                    .find(|t| t.key.id == id)
                    .cloned()
                    .unwrap_or(Item {
                        key: ResourceKey {
                            source: "netease".into(),
                            id,
                        },
                        title: "歌曲暂不可用".into(),
                        kind: "track".into(),
                        unavailable: true,
                        ..Default::default()
                    }),
            );
        }
    }
    Ok(all)
}
pub fn handle(r: Request) -> SourceResult {
    let a = r.args;
    let offset = a["offset"].as_u64().unwrap_or(0);
    let id = || checked(&a["id"]);
    match r.op.as_str() {
        "search" => {
            let query = a["query"].as_str().unwrap_or("").trim();
            let (kind, typ, key, count) = match a["kind"].as_str().unwrap_or("track") {
                "album" => ("album", 10, "albums", "albumCount"),
                "artist" => ("artist", 100, "artists", "artistCount"),
                "playlist" => ("playlist", 1000, "playlists", "playlistCount"),
                _ => ("track", 1, "songs", "songCount"),
            };
            if query.is_empty() {
                return Ok(page("搜索", &json!([]), kind, offset, false));
            }
            let j = api(
                "/api/cloudsearch/pc",
                json!({"s":query,"type":typ,"limit":50,"offset":offset,"total":true}),
            )?;
            Ok(page(
                "搜索结果",
                &j["result"][key],
                kind,
                offset,
                j["result"][count].as_u64().unwrap_or(0) > offset + 50,
            ))
        }
        "suggest" => {
            let j = api("/api/search/suggest/web", json!({"s":a["query"]}))?;
            Ok(json!(
                array(&j["result"]["songs"])
                    .iter()
                    .map(|x| text(x, "name"))
                    .collect::<Vec<_>>()
            ))
        }
        "home" => {
            let j = api(
                "/api/personalized/playlist",
                json!({"limit":24,"total":true,"n":1000}),
            )?;
            Ok(page("为你推荐", &j["result"], "playlist", 0, false))
        }
        "browse" => {
            let p = a["page"].as_str().unwrap_or("playlists");
            let (path, data, key, kind, title) = match p {
                "charts" => ("/api/toplist", json!({}), "list", "playlist", "排行榜"),
                "daily" => {
                    account()?;
                    (
                        "/api/v3/discovery/recommend/songs",
                        json!({}),
                        "dailySongs",
                        "track",
                        "每日推荐",
                    )
                }
                "new-albums" => (
                    "/api/discovery/newAlbum",
                    json!({}),
                    "albums",
                    "album",
                    "最新专辑",
                ),
                "cloud" => {
                    account()?;
                    (
                        "/api/v1/cloud/get",
                        json!({"limit":50,"offset":offset}),
                        "data",
                        "track",
                        "我的云盘",
                    )
                }
                _ => (
                    "/api/playlist/list",
                    json!({"cat":a["category"].as_str().unwrap_or("全部"),"order":"hot","limit":50,"offset":offset,"total":true}),
                    "playlists",
                    "playlist",
                    "热门歌单",
                ),
            };
            let j = api(path, data)?;
            if p == "cloud" {
                let items = array(&j["data"])
                    .iter()
                    .map(|v| {
                        let mut t = item(&v["simpleSong"], "track");
                        t.key.id = strval(&v["songId"]);
                        t.title = text(v, "songName");
                        t.subtitle = text(v, "artist");
                        t.unavailable = t.key.id.is_empty();
                        t
                    })
                    .collect();
                return Ok(json!(Page {
                    title: title.into(),
                    items,
                    offset,
                    more: j["hasMore"] == true || j["count"].as_u64().unwrap_or(0) > offset + 50
                }));
            }
            let values = if p == "daily" {
                &j["data"][key]
            } else {
                &j[key]
            };
            Ok(page(title, values, kind, offset, j["more"] == true))
        }
        "album" => {
            let j = api(&format!("/api/v1/album/{}", id()?), json!({}))?;
            Ok(page(
                &text(&j["album"], "name"),
                &j["songs"],
                "track",
                0,
                false,
            ))
        }
        "artist" => {
            let key = id()?;
            let j = api(&format!("/api/v1/artist/{key}"), json!({}))?;
            let albums = api(
                &format!("/api/artist/albums/{key}"),
                json!({"limit":50,"offset":offset,"total":true}),
            )?;
            let mut p: Page = serde_json::from_value(page(
                &text(&j["artist"], "name"),
                &j["hotSongs"],
                "track",
                offset,
                albums["more"] == true,
            ))
            .unwrap();
            p.items
                .extend(array(&albums["hotAlbums"]).iter().map(|v| item(v, "album")));
            Ok(json!(p))
        }
        "playlist" => {
            let j = api(
                "/api/v6/playlist/detail",
                json!({"id":id()?,"n":1000,"s":0}),
            )?;
            let p = &j["playlist"];
            if !p.is_object() {
                return Err(err("unavailable", "歌单不存在或无权访问"));
            }
            let ids: Vec<_> = array(&p["trackIds"])
                .iter()
                .map(|x| x["id"].clone())
                .collect();
            let start = (offset as usize).min(ids.len());
            let end = (start + 50).min(ids.len());
            let items = if ids.is_empty() {
                array(&p["tracks"])
                    .iter()
                    .map(|v| item(v, "track"))
                    .collect()
            } else {
                details(&ids[start..end])?
            };
            Ok(json!(Page {
                title: text(p, "name"),
                items,
                offset,
                more: end < ids.len()
            }))
        }
        "playlists" => {
            let user = account()?;
            let j = api(
                "/api/user/playlist",
                json!({"uid":user["id"],"limit":50,"offset":offset,"includeVideo":false}),
            )?;
            Ok(page(
                "我的歌单",
                &j["playlist"],
                "playlist",
                offset,
                j["more"] == true,
            ))
        }
        "liked" => {
            let user = account()?;
            let j = api("/api/song/like/get", json!({"uid":user["id"]}))?;
            let ids = array(&j["ids"]);
            let start = (offset as usize).min(ids.len());
            let end = (start + 50).min(ids.len());
            Ok(json!(Page {
                title: "喜欢的歌曲".into(),
                items: details(&ids[start..end])?,
                offset,
                more: end < ids.len()
            }))
        }
        "like" => {
            account()?;
            api(
                "/api/radio/like",
                json!({"alg":"itembased","trackId":id()?,"like":a["on"],"time":"3"}),
            )?;
            Ok(Value::Null)
        }
        "savedAlbums" => {
            account()?;
            let j = api(
                "/api/album/sublist",
                json!({"limit":50,"offset":offset,"total":true}),
            )?;
            Ok(page(
                "收藏专辑",
                &j["data"],
                "album",
                offset,
                j["hasMore"] == true || array(&j["data"]).len() == 50,
            ))
        }
        "saveAlbum" => {
            account()?;
            api(
                if a["on"] == true {
                    "/api/album/sub"
                } else {
                    "/api/album/unsub"
                },
                json!({"id":id()?}),
            )?;
            Ok(Value::Null)
        }
        "recent" => {
            account()?;
            let j = api("/api/play-record/song/list", json!({"limit":50}))?;
            let tracks: Vec<_> = array(&j["data"]["list"])
                .iter()
                .map(|x| x["data"].clone())
                .collect();
            Ok(page("最近播放", &json!(tracks), "track", 0, false))
        }
        "stream" => {
            let qualities = ["hires", "lossless", "high", "standard"];
            let setting = get("quality")?;
            let selected = setting
                .as_str()
                .filter(|v| qualities.contains(v))
                .unwrap_or(a["quality"].as_str().unwrap_or("high"));
            let start = qualities.iter().position(|v| *v == selected).unwrap_or(2);
            for q in &qualities[start..] {
                let level = if *q == "high" { "exhigh" } else { q };
                let j = api(
                    "/api/song/enhance/player/url/v1",
                    json!({"ids":format!("[{}]",id()?),"level":level,"encodeType":"flac"}),
                )?;
                let s = &j["data"][0];
                if s["url"].is_string()
                    && !s["url"].as_str().unwrap().is_empty()
                    && (s["freeTrialInfo"].is_null() || s["freeTrialInfo"] == "null")
                {
                    return Ok(
                        json!({"url":text(s,"url").replacen("http://","https://",1),"quality":level}),
                    );
                }
            }
            Err(err(
                "unavailable",
                "没有可用完整音频，请检查账号、会员或地区权限",
            ))
        }
        "lyrics" => {
            let j = api(
                "/api/song/lyric",
                json!({"id":id()?,"tv":-1,"lv":-1,"rv":-1,"kv":-1,"_nmclfl":1}),
            )?;
            Ok(json!({"lrc":j["lrc"]["lyric"],"translation":j["tlyric"]["lyric"]}))
        }
        "settings" => {
            if let Some(q) = a["quality"].as_str() {
                if !["follow", "standard", "high", "lossless", "hires"].contains(&q) {
                    return Err(err("setup", "无效音质"));
                }
                set("quality", json!(q))?;
            }
            get("quality")
        }
        "account" => {
            if get("cookies")?["MUSIC_U"].is_null() {
                return Ok(Value::Null);
            }
            match api("/api/w/nuser/account/get", json!({})) {
                Ok(j) => {
                    let a = json!({"id":j["profile"]["userId"],"name":j["profile"]["nickname"]});
                    set("account", a.clone())?;
                    Ok(a)
                }
                Err(e) if e.code == "signIn" => {
                    set("cookies", Value::Null)?;
                    set("account", Value::Null)?;
                    Ok(Value::Null)
                }
                Err(e) => Err(e),
            }
        }
        "loginStart" => {
            let (j, _) = api_with(
                "/api/login/qrcode/unikey",
                json!({"type":3}),
                json!({}),
                false,
            )?;
            let key = text(&j, "unikey");
            if key.is_empty() {
                return Err(err("failed", "未取得二维码"));
            }
            set("login", json!({"key":key,"started":now()}))?;
            Ok(json!({"key":key,"qr":format!("https://music.163.com/login?codekey={key}")}))
        }
        "loginPoll" => {
            let pending = get("login")?;
            if pending["key"] != a["key"]
                || now().saturating_sub(pending["started"].as_u64().unwrap_or(0)) > 120000
            {
                return Ok(json!({"status":"expired"}));
            }
            let (j, c) = api_with(
                "/api/login/qrcode/client/login",
                json!({"key":a["key"],"type":3}),
                json!({}),
                true,
            )?;
            if j["code"] == 803 {
                let (u, c) = api_with("/api/w/nuser/account/get", json!({}), c, false)?;
                if u["profile"]["userId"].is_null() || c["MUSIC_U"].is_null() {
                    return Err(err("signIn", "无法确认登录账号"));
                }
                set("cookies", c)?;
                set(
                    "account",
                    json!({"id":u["profile"]["userId"],"name":u["profile"]["nickname"]}),
                )?;
                set("login", Value::Null)?;
            }
            Ok(
                json!({"status":match j["code"].as_i64(){Some(800)=>"expired",Some(801)=>"waiting",Some(802)=>"scanned",Some(803)=>"success",_=>"failed"}}),
            )
        }
        "loginCancel" => {
            set("login", Value::Null)?;
            Ok(Value::Null)
        }
        "signOut" => {
            for k in ["cookies", "account", "login"] {
                set(k, Value::Null)?;
            }
            Ok(Value::Null)
        }
        _ => Err(err("unsupported", format!("未支持 {}", r.op))),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encryption_vector() {
        assert_eq!(
            encrypt("/api/test", "{}"),
            "4DC723619A991588865191FD2F319BAD89A665F3FF2B75B10678911D4E9910E4690E5F68E0095E53773148F1312770C2786A0951D5B32887C8AE362CEF057841F3A83A42DA35956C149F335E924490D4"
        );
    }
    #[test]
    fn identifiers() {
        assert!(checked(&json!("../x")).is_err());
        assert_eq!(checked(&json!(123)).unwrap(), "123");
    }
}
