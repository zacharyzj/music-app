use super::*;
fn upstream() -> Package {
    Package::read(
        &bundle(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/netease-js"
        )))
        .unwrap(),
    )
    .unwrap()
}
fn session(package: Package, dir: &Path) -> Session {
    Session::new(
        package,
        Storage::with_key(dir.join("test.enc"), [3; 32]).unwrap(),
        "test".into(),
        dir.into(),
    )
    .unwrap()
}
fn module(source: &str) -> Package {
    let mut p = upstream();
    p.files.clear();
    p.files
        .insert("index.js".into(), source.as_bytes().to_vec());
    p.main = "index.js".into();
    p
}
#[test]
fn upstream_search_and_large_account_pages() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(upstream(), dir.path());
    s.host
        .borrow_mut()
        .storage
        .set("account", json!({"id":"1"}))
        .unwrap();
    s.host
        .borrow_mut()
        .storage
        .set("cookies", json!({"MUSIC_U":"fixture"}))
        .unwrap();
    let songs:Vec<_>=(0..50).map(|i|json!({"id":i+1,"name":format!("测试{i}"),"ar":[{"name":"歌手"}],"al":{"name":"专辑","picUrl":"https://p1.music.126.net/test.jpg"},"dt":240000,"metadata":"音乐元数据".repeat(600)})).collect();
    let cloud: Vec<_> = songs
        .iter()
        .enumerate()
        .map(|(i, t)| json!({"songId":1000+i,"cover":1099511000,"songName":t["name"],"artist":"歌手","simpleSong":t}))
        .collect();
    s.host.borrow_mut().fixture = Some(json!({
        "cloudsearch/pc":{"code":200,"result":{"songs":songs,"songCount":120}},
        "v3/discovery/recommend/songs":{"code":200,"data":{"dailySongs":songs}},
        "v1/cloud/get":{"code":200,"data":cloud,"hasMore":true,"count":120}
    }));
    let p = s
        .call(Request::new(
            "search",
            json!({"query":"test","kind":"track"}),
        ))
        .unwrap();
    assert_eq!(p["items"].as_array().unwrap().len(), 50);
    assert_eq!(p["more"], true);
    let p = s
        .call(Request::new("browse", json!({"page":"daily"})))
        .unwrap();
    assert_eq!(p["items"].as_array().unwrap().len(), 50);
    let p = s
        .call(Request::new("browse", json!({"page":"cloud"})))
        .unwrap();
    assert_eq!(p["items"][0]["key"]["id"], "1000");
    assert_eq!(p["items"][0]["cover"], "https://p1.music.126.net/test.jpg");
    assert_eq!(p["links"][0]["args"]["page"], "cloud:50");
    let p = s
        .call(Request::new("browse", json!({"page":"cloud:50"})))
        .unwrap();
    assert_eq!(p["links"][0]["args"]["page"], "cloud:0");
    assert_eq!(p["links"][1]["args"]["page"], "cloud:100");
}
#[test]
fn netease_liked_tracks_sort_by_add_time_before_paging() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(upstream(), dir.path());
    let entries: Vec<_> = (1..=53)
        .map(|i| match i {
            1 => json!({"id":i}),
            2 => json!({"id":i, "at":0}),
            3 | 4 => json!({"id":i, "at":10000}),
            _ => json!({"id":i, "at":i}),
        })
        .collect();
    // Details come back in a different order and one recording is no longer available.
    let songs: Vec<_> = (1..=53)
        .filter(|&id| id != 52)
        .map(|id| json!({"id":id}))
        .collect();
    {
        let mut host = s.host.borrow_mut();
        host.storage.set("account", json!({"id":7})).unwrap();
        host.storage
            .set("deviceId", json!("fixture-device"))
            .unwrap();
        host.storage
            .set("cookies", json!({"MUSIC_U":"fixture"}))
            .unwrap();
        host.fixture = Some(json!({
            "user/playlist":{"responses":[
                {"playlist":[{"id":8,"userId":7,"specialType":0},{"id":9,"userId":6,"specialType":5}],"more":true},
                {"playlist":[{"id":10,"userId":7,"specialType":5}],"more":false}
            ]},
            "v6/playlist/detail":{"playlist":{"id":10,"trackCount":53,"trackIds":entries}},
            "v3/song/detail":{"songs":songs}
        }));
    }
    let page_ids = |v: &Value| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["key"]["id"].as_str().unwrap().parse::<i32>().unwrap())
            .collect::<Vec<_>>()
    };
    let credentials = std::fs::read(dir.path().join("test.enc")).unwrap();
    let first = s.call(Request::new("liked", json!({}))).unwrap();
    let order: Vec<_> = [3, 4]
        .into_iter()
        .chain((5..=53).rev())
        .chain([1, 2])
        .collect();
    assert_eq!(page_ids(&first), order[..50]);
    assert_eq!(first["items"][3]["unavailable"], true);
    assert_eq!(first["more"], true);
    assert!(
        s.host.borrow().fixture.as_ref().unwrap()["user/playlist"]["responses"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    s.host.borrow_mut().fixture = Some(json!({"v3/song/detail":{"songs":songs}}));
    let next = s.call(Request::new("liked", json!({"offset":50}))).unwrap();
    assert_eq!(page_ids(&next), order[50..]);
    assert_eq!(next["more"], false);
    assert_eq!(
        std::fs::read(dir.path().join("test.enc")).unwrap(),
        credentials,
        "paging without Set-Cookie must not rewrite stored credentials"
    );

    // The other navigation entry for this playlist agrees. An ordinary playlist keeps its manual order.
    for (special_type, expected) in [(5, vec![12, 11]), (0, vec![11, 12])] {
        s.host.borrow_mut().fixture = Some(json!({"v6/playlist/detail":{"playlist":{
            "id":10,"userId":7,"specialType":special_type,"trackIds":[{"id":11,"at":1},{"id":12,"at":2}],
            "tracks":[{"id":12},{"id":11}]
        }}}));
        let playlist = s
            .call(Request::new("playlist", json!({"id":"10"})))
            .unwrap();
        assert_eq!(page_ids(&playlist), expected);
        assert_eq!(
            playlist["heartbeatPlaylistId"].as_str(),
            (special_type == 5).then_some("10")
        );
    }

    // A successful edit invalidates the old page order. Missing playlist metadata falls back to membership.
    s.host.borrow_mut().fixture = Some(
        json!({"radio/like":{"code":200},"user/playlist":{"playlist":[]},
        "song/like/get":{"ids":[9,7,8]},"v3/song/detail":{"songs":songs}}),
    );
    s.call(Request::new("like", json!({"id":"3","on":false})))
        .unwrap();
    let fallback = s.call(Request::new("liked", json!({"offset":1}))).unwrap();
    assert_eq!(page_ids(&fallback), [7, 8]);
    assert_eq!(fallback["title"], "喜欢的歌曲 · 网易云返回顺序");
    assert!(fallback["heartbeatPlaylistId"].is_null());
    assert_eq!(
        s.call(Request::new("isLiked", json!({"ids":["9","3"]})))
            .unwrap(),
        json!({"9":true,"3":false})
    );
    for (entries, expected, title) in [
        (
            json!([{"id":8,"at":4},{"id":7,"at":6}]),
            vec![7, 8, 9],
            "喜欢的歌曲",
        ),
        (json!([]), vec![9, 7, 8], "喜欢的歌曲 · 网易云返回顺序"),
    ] {
        s.host.borrow_mut().fixture = Some(json!({
            "user/playlist":{"playlist":[{"id":10,"userId":7,"specialType":5}]},
            "v6/playlist/detail":{"playlist":{"trackCount":3,"trackIds":entries}},
            "v3/song/detail":{"songs":songs}
        }));
        let page = s.call(Request::new("liked", json!({}))).unwrap();
        assert_eq!(page_ids(&page), expected);
        assert_eq!(page["title"], title);
    }
}

#[test]
fn netease_heartbeat_mixes_owned_favorites_with_playable_new_tracks() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(upstream(), dir.path());
    let mix = |id: &str| {
        Request::new(
            "heartbeat",
            json!({"playlistId":id,"currentId":"1","recent":["1","91"]}),
        )
    };
    assert!(s.call(mix("10")).unwrap_err().to_string().contains("登录"));
    {
        let mut host = s.host.borrow_mut();
        host.storage.set("account", json!({"id":"7"})).unwrap();
        host.storage
            .set("cookies", json!({"MUSIC_U":"fixture"}))
            .unwrap();
        host.fixture = Some(json!({
            "user/playlist":{"playlist":[{"id":9,"userId":8,"specialType":5},{"id":10,"userId":7,"specialType":5}]},
            "v6/playlist/detail":{"playlist":{"trackIds":[{"id":1},{"id":2},{"id":3}],"trackCount":3}},
            "v3/song/detail":{"songs":[{"id":1,"name":"种子"},{"id":2,"name":"收藏甲"},{"id":3,"name":"收藏乙"},{"id":94,"name":"补全推荐"}]},
            "playmode/intelligence/list":{"data":[{"id":2},{"id":91},{"id":92,"songInfo":{"id":92,"name":"已下架","privilege":{"st":-1}}},
                {"id":93,"songInfo":{"id":93,"name":"新推荐"}},{"id":93},{"id":94},{"id":"bad"}]}
        }));
    }
    let liked = s.call(Request::new("liked", json!({}))).unwrap();
    assert_eq!(liked["heartbeatPlaylistId"], "10");
    assert!(
        s.call(mix("9"))
            .unwrap_err()
            .to_string()
            .contains("当前账号")
    );
    let batch = s.call(mix("10")).unwrap();
    let items = batch["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(items[0]["key"]["id"], "93");
    assert_eq!(items[2]["key"]["id"], "94");
    assert_eq!(items[2]["title"], "补全推荐");
    let favorites: std::collections::HashSet<_> = [
        items[1]["key"]["id"].as_str().unwrap(),
        items[3]["key"]["id"].as_str().unwrap(),
    ]
    .into();
    assert_eq!(favorites, ["2", "3"].into());
    for (i, item) in items.iter().enumerate() {
        assert_eq!(item["heartbeat_recommendation"], i % 2 == 0);
        assert_eq!(item["key"]["source"], "netease");
    }
    s.host.borrow_mut().fixture.as_mut().unwrap()["playmode/intelligence/list"] =
        json!({"data":[]});
    let empty = s
        .call(Request::new(
            "heartbeat",
            json!({"playlistId":"10","currentId":"93","recent":["1","2","3"]}),
        ))
        .unwrap();
    assert!(empty["items"].as_array().unwrap().is_empty());
}

#[test]
fn trial_fallback_and_lyrics() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(upstream(), dir.path());
    s.host.borrow_mut().fixture = Some(
        json!({"song/enhance/player/url/v1":{"responses":[{"code":200,"data":[{"url":"https://m7.music.126.net/trial","freeTrialInfo":{}}]},{"code":200,"data":[{"url":"https://m7.music.126.net/full","freeTrialInfo":null,"level":"standard","gain":-5.5,"peak":0.84,"albumGain":-4.0,"albumPeak":0.9}]}]},"song/lyric":{"code":200,"lrc":{"lyric":"[00:01.00]歌词"}}}),
    );
    let p = s
        .call(Request::new("stream", json!({"id":"7","quality":"high"})))
        .unwrap();
    assert_eq!(p["url"], "https://m7.music.126.net/full");
    assert_eq!(p["quality"], "standard");
    assert_eq!(
        (
            p["gain"].as_f64(),
            p["albumGain"].as_f64(),
            p["peak"].as_f64(),
            p["albumPeak"].as_f64()
        ),
        (Some(-5.5), Some(-4.), Some(0.84), Some(0.9))
    );
    assert_eq!(
        s.call(Request::new("lyrics", json!({"id":"7"}))).unwrap()["lrc"],
        "[00:01.00]歌词"
    );
}
#[test]
fn login_prompt_cancel_preserves_legacy_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.enc");
    let mut storage = Storage::with_key(path.clone(), [3; 32]).unwrap();
    storage.set("device", json!("old-device")).unwrap();
    storage.set("account", json!({"id":1})).unwrap();
    storage
        .set("cookies", json!({"MUSIC_U":"original"}))
        .unwrap();
    let mut s = Session::new(upstream(), storage, "test".into(), dir.path().into()).unwrap();
    s.host.borrow_mut().fixture =
        Some(json!({"login/qrcode/unikey":{"code":200,"unikey":"fixture-key"}}));
    let start = s.call(Request::new("loginStart", json!({}))).unwrap();
    assert!(start["qr"].as_str().unwrap().contains("fixture-key"));
    drop(s);
    let storage = Storage::with_key(path, [3; 32]).unwrap();
    assert_eq!(storage.get("cookies")["MUSIC_U"], "original");
    assert_eq!(storage.get("deviceId"), "old-device");
}
#[test]
fn modules_timers_and_persistent_context() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = module(
        "import { name } from './helper.js';let n=0;export async function home(){await new Promise(r=>setTimeout(r,0));return {title:name+(++n),sections:[]}};",
    );
    p.files
        .insert("helper.js".into(), b"export const name='hello';".to_vec());
    let mut s = session(p, dir.path());
    assert_eq!(
        s.call(Request::new("home", json!({}))).unwrap()["title"],
        "hello1"
    );
    assert_eq!(
        s.call(Request::new("home", json!({}))).unwrap()["title"],
        "hello2"
    );
}
#[test]
fn rejects_module_escape_and_network_escape() {
    let dir = tempfile::tempdir().unwrap();
    let p = module("import '../outside.js';export function home(){}");
    assert!(
        Session::new(
            p,
            Storage::with_key(dir.path().join("bad.enc"), [3; 32]).unwrap(),
            "bad".into(),
            dir.path().into()
        )
        .is_err()
    );
    let mut s = session(
        module("export async function home(){await fetch('https://example.com');return {}}"),
        dir.path(),
    );
    assert!(
        s.call(Request::new("home", json!({})))
            .unwrap_err()
            .to_string()
            .contains("无权")
    );
    assert!(!allowed(
        "https://interfacepc.music.163.com.evil/a",
        &["interfacepc.music.163.com".into()]
    ));
    assert!(!media_allowed("file:///secret", &[]));
}
#[test]
fn infinite_js_is_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(module("export function home(){}"), dir.path());
    s.deadline.set(Instant::now() + Duration::from_millis(30));
    let start = Instant::now();
    assert!(s.eval::<()>("while(true){}").is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
}
#[test]
fn rejects_zip_traversal_and_duplicate_manifest() {
    assert!(!package::safe_name("../index.js"));
    assert!(!package::safe_name("C:/index.js"));
    assert!(!package::safe_name("dir\\file.js"));
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(vec![]));
    zip.start_file("../plugin.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"{}").unwrap();
    assert!(Package::read(&zip.finish().unwrap().into_inner()).is_err());
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(vec![]));
    for name in ["plugin.json", "nested/plugin.json"] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"{}").unwrap();
    }
    assert!(Package::read(&zip.finish().unwrap().into_inner()).is_err());
}

fn start_qr_login(s: &mut Session) -> Value {
    s.eval::<()>("let qrClock=Date.now(); Date.now=()=>qrClock;")
        .unwrap();
    s.call(Request::new("loginStart", json!({}))).unwrap()
}

fn poll_qr_after(s: &mut Session, start: &Value, millis: u32) -> Result<Value> {
    // Let the previous poll register its timer before moving the fixture clock.
    while s.engine.is_job_pending() {
        s.pump()?;
    }
    s.eval::<()>(&format!("qrClock += {millis}"))?;
    loop {
        let state = s.call(Request::new("loginPoll", json!({"key":start["key"]})))?;
        if state["status"] != "waiting" || !s.engine.is_job_pending() {
            return Ok(state);
        }
    }
}

#[test]
fn upstream_login_rotates_qr_and_commits_verified_account() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(upstream(), dir.path());
    s.host.borrow_mut().fixture = Some(json!({
        "login/qrcode/unikey":{"responses":[{"code":200,"unikey":"first-key"},{"code":200,"unikey":"second-key"}]},
        "login/qrcode/client/login":{"responses":[{"code":802},{"code":800},{"code":803,"_cookies":["MUSIC_U=verified; Path=/; HttpOnly"]}]},
        "w/nuser/account/get":{"code":200,"profile":{"userId":42,"nickname":"fixture-account"}}
    }));
    let start = start_qr_login(&mut s);
    let waiting = poll_qr_after(&mut s, &start, 2000).unwrap();
    assert_eq!(waiting["status"], "waiting");
    assert!(waiting["message"].as_str().unwrap().contains("已扫码"));
    let rotated = poll_qr_after(&mut s, &start, 2000).unwrap();
    assert!(rotated["qr"].as_str().unwrap().contains("second-key"));
    let success = poll_qr_after(&mut s, &start, 2000).unwrap();
    assert_eq!(success["status"], "success");
    assert_eq!(success["account"]["id"], "42");
    assert_eq!(
        s.host.borrow().storage.get("cookies")["MUSIC_U"],
        "verified"
    );
    assert_eq!(s.host.borrow().storage.get("account")["id"], "42");
}

#[test]
fn netease_download_checks_the_account_and_download_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(upstream(), dir.path());
    let request = || Request::new("download", json!({"id":"7"}));
    assert!(s.call(request()).unwrap_err().to_string().contains("登录"));
    {
        let mut host = s.host.borrow_mut();
        host.storage.set("account", json!({"id":"1"})).unwrap();
        host.storage
            .set("cookies", json!({"MUSIC_U":"fixture"}))
            .unwrap();
        host.fixture = Some(json!({"song/enhance/download/url/v1":{"responses":[
            {"code":200,"data":{"id":7,"code":403,"url":"https://m1.music.126.net/denied.mp3"}},
            {"code":200,"data":{"id":7,"code":200,"url":"https://m1.music.126.net/trial.mp3","freeTrialInfo":{"start":0,"end":30}}},
            {"code":200,"data":{"id":7,"code":200,"url":"http://m1.music.126.net/complete.flac","size":1042}}
        ]}}));
    }
    for _ in 0..2 {
        assert!(
            s.call(request())
                .unwrap_err()
                .to_string()
                .contains("下载权限")
        );
    }
    assert_eq!(
        s.call(request()).unwrap(),
        json!({"url":"https://m1.music.126.net/complete.flac","size":1042})
    );
}

#[test]
fn open_catalog_search_and_download_share_a_stable_recording_and_credit() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = bundle(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/incompetech-js"
    )))
    .unwrap();
    let mut s = session(Package::read(&bytes).unwrap(), directory.path());
    s.host.borrow_mut().fixture = Some(json!({
        "https://incompetech.com/music/royalty-free/pieces.json": [
            {"title":"Open song","isrc":"US123","filename":"Open #1.mp3","length":"00:02:05","instruments":"piano"},
            {"title":"Broken","isrc":"US456","filename":"../other.mp3"}
        ]
    }));
    let page = s
        .call(Request::new(
            "search",
            json!({"kind":"track","query":"piano"}),
        ))
        .unwrap();
    let item = &page["items"][0];
    assert_eq!(item["key"], json!({"source":"incompetech","id":"US123"}));
    assert_eq!(item["duration_ms"], 125000);
    assert_eq!(item["license"], "CC BY 4.0");
    let media = s
        .call(Request::new("download", json!({"id":"US123"})))
        .unwrap();
    assert_eq!(
        media["url"],
        "https://incompetech.com/music/royalty-free/mp3-royaltyfree/Open%20%231.mp3"
    );
    assert!(
        media["attribution"]
            .as_str()
            .unwrap()
            .contains("https://creativecommons.org/licenses/by/4.0/")
    );
    assert!(
        s.call(Request::new("download", json!({"id":"US456"})))
            .is_err()
    );
}

fn audius_session(dir: &Path) -> Session {
    let bytes = bundle(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/audius-js"
    )))
    .unwrap();
    session(Package::read(&bytes).unwrap(), dir)
}

#[test]
fn audius_checks_each_recording_again_before_anonymous_download_or_playback() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = audius_session(dir.path());
    let search = "https://api.audius.co/v1/tracks/search?limit=51&query=pop&offset=0";
    let detail = "https://api.audius.co/v1/tracks/abc";
    let open = json!({"id":"abc","title":"不動守候","user":{"name":"MUGUNASHII"},"duration":206,
        "is_streamable":true,"is_stream_gated":false,"is_downloadable":true,"is_download_gated":false,
        "access":{"stream":true,"download":true}, "permalink":"/mugunashii/不動守候",
        "stream":{"url":"https://creatornode.audius.co/play"},
        "download":{"url":"https://unlisted.example/file?signature=fixture",
                    "mirrors":["https://invalid.example","https://val001.open-audio-validator.com"]}});
    let mut disabled = open.clone();
    disabled["is_downloadable"] = json!(false);
    let mut gated = open.clone();
    gated["download_conditions"] = json!({"follow_user_id":42});
    let mut denied = open.clone();
    denied["access"]["download"] = json!(false);
    let mut blocked_stream = open.clone();
    blocked_stream["is_streamable"] = json!(false);
    s.host.borrow_mut().fixture = Some(json!({
        (search):{"data":[open,disabled,gated,denied,blocked_stream]},
        (detail):{"responses":[{"data":disabled},{"data":gated},{"data":denied},{"data":open},{"data":blocked_stream}]}
    }));
    let result = s
        .call(Request::new(
            "search",
            json!({"kind":"track","query":"pop"}),
        ))
        .unwrap();
    let rows = result["items"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r["downloadable"].as_bool().unwrap())
            .collect::<Vec<_>>(),
        [true, false, false, false, true]
    );
    assert_eq!(rows[4]["unavailable"], true);
    assert_eq!(rows[0]["search_aliases"], json!(["不动守候"]));
    let request = || Request::new("download", json!({"id":"abc"}));
    for _ in 0..3 {
        assert!(
            s.call(request())
                .unwrap_err()
                .to_string()
                .contains("免登录下载")
        );
    }
    let media = s.call(request()).unwrap();
    assert_eq!(
        media["url"],
        "https://val001.open-audio-validator.com/file?signature=fixture"
    );
    assert!(
        media["attribution"]
            .as_str()
            .unwrap()
            .contains("https://audius.co/mugunashii/不動守候")
    );
    assert!(
        s.call(Request::new("stream", json!({"id":"abc"})))
            .unwrap_err()
            .to_string()
            .contains("完整播放")
    );
}

#[test]
fn audius_keeps_the_matching_chinese_spelling_for_pagination() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = audius_session(dir.path());
    let endpoint = |query: &str, offset: &str| {
        let mut url = url::Url::parse("https://api.audius.co/v1/tracks/search").unwrap();
        url.query_pairs_mut()
            .append_pair("limit", "51")
            .append_pair("query", query)
            .append_pair("offset", offset);
        url.to_string()
    };
    let rows: Vec<_> = (0..51)
        .map(|id| json!({"id":format!("s{id}"),"title":"不動守候"}))
        .collect();
    s.host.borrow_mut().fixture = Some(json!({
        (endpoint("不动守候", "0")):{"data":[]},
        (endpoint("不動守候", "0")):{"data":rows},
        (endpoint("不動守候", "50")):{"data":[rows[50]]}
    }));
    let search = |offset| {
        Request::new(
            "search",
            json!({"kind":"track","query":"不动守候","offset":offset}),
        )
    };
    let first = s.call(search(0)).unwrap();
    assert_eq!(first["more"], true);
    assert_eq!(first["items"].as_array().unwrap().len(), 50);
    let next = s.call(search(50)).unwrap();
    assert_eq!(next["items"][0]["key"]["id"], "s50");
    assert_eq!(next["more"], false);
}

fn bilibili(root: &Path) -> Session {
    let bytes = bundle(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/bilibili-js"
    )))
    .unwrap();
    session(Package::read(&bytes).unwrap(), root)
}

#[test]
fn bilibili_search_uses_issued_cookies_and_resolves_video_audio() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = bilibili(dir.path());
    let m = s.host.borrow().package.manifest.clone();
    assert_eq!(m.search_page_size.unwrap().get(), 20);
    assert!(m.capabilities.iter().any(|c| c == "video"));
    assert!(!m.supports_likes());
    assert!(
        !m.capabilities
            .iter()
            .any(|c| c == "download" || c == "anonymous")
    );
    let bv = "BV123456789a";
    let video = json!({"bvid":bv,"title":"<em class=\"keyword\">晴天</em> &amp; 现场","author":"UP&#39;主","duration":"1:02:03","pic":"//i0.hdslb.com/photo.jpg"});
    let keyword = "%E6%99%B4%E5%A4%A9";
    let entry = format!("https://search.bilibili.com/all?keyword={keyword}");
    let query = format!(
        "https://api.bilibili.com/x/web-interface/search/type?search_type=video&keyword={keyword}"
    );
    let view = format!("https://api.bilibili.com/x/web-interface/view?bvid={bv}");
    let play = format!("https://api.bilibili.com/x/player/playurl?bvid={bv}&cid=99&fnval=16");
    let pictures = format!("{play}&qn=127&fourk=1");
    s.host.borrow_mut().fixture = Some(json!({
        (entry):{"_expectCookie":"","_cookies":["buvid3=visitor; Path=/","unrelated=ignored; Path=/"]},
        (format!("{query}&page=1")):{"_expectCookie":"buvid3=visitor","code":0,"data":{"numPages":2,"result":[video]}},
        (format!("{query}&page=2")):{"_expectCookie":"buvid3=visitor; SESSDATA=user","code":0,"data":{"numPages":2,"result":[]}},
        (view):{"_expectCookie":"SESSDATA=user","code":0,"data":{"pages":[{"cid":99},{"cid":100}]}},
        (play):{"_expectCookie":"SESSDATA=user","code":0,"data":{"dash":{"audio":[{"bandwidth":20,"base_url":"https://elsewhere.example/audio","backupUrl":["https://hdslb.bilivideo.com/public.m4s"]},{"bandwidth":10,"baseUrl":"https://cdn.bilivideo.cn/other"}]}}}
    }));
    let args = json!({"query":"晴天", "kind":"track"});
    let page = s.call(Request::new("search", args.clone())).unwrap();
    let row = &page["items"][0];
    assert_eq!(row["key"], json!({"source":"bilibili","id":bv}));
    assert_eq!(row["title"], "晴天 & 现场");
    assert_eq!(row["artist"], "UP'主");
    assert_eq!(row["duration_ms"], 3_723_000);
    assert_eq!(row["cover"], "https://i0.hdslb.com/photo.jpg");
    assert_eq!(row["downloadable"], false);
    assert_eq!(page["more"], true);
    s.host
        .borrow_mut()
        .storage
        .set("session", json!({"SESSDATA":"user"}))
        .unwrap();
    let page = s
        .call(Request::new("search", json!({"query":"晴天","offset":20})))
        .unwrap();
    assert_eq!(page["more"], false);
    assert_eq!(
        s.call(Request::new("stream", json!({"id":bv}))).unwrap(),
        json!({"url":"https://hdslb.bilivideo.com/public.m4s","referrer":"https://www.bilibili.com/video/BV123456789a/"})
    );
    let picture = "https://cdn.bilivideo.cn/720.m4s";
    let sound = "https://cdn.bilivideo.cn/audio.m4s?token=a,b";
    let combined = "https://cdn.bilivideo.cn/muxed.mp4";
    let page_url = format!("https://www.bilibili.com/video/{bv}/");
    s.host.borrow_mut().fixture.as_mut().unwrap()[&pictures] = json!({
        "_expectCookie":"SESSDATA=user", "code":0, "data":{"quality":64,"volume":{"measured_i":-10,"measured_tp":0},"dash":{
            "video":[
                {"id":16,"height":360,"baseUrl":"https://cdn.bilivideo.cn/360.m4s"},
                {"id":64,"height":720,"codecid":12,"baseUrl":"https://cdn.bilivideo.cn/hevc.m4s"},
                {"id":80,"height":1080,"baseUrl":"https://cdn.bilivideo.cn/1080.m4s"},
                {"id":64,"height":720,"codecid":7,"base_url":"https://other.example/video","backup_url":[picture]}
            ],
            "audio":[{"baseUrl":sound}]
        }}
    });
    let watch = Request::new("stream", json!({"id":bv,"video":true}));
    assert_eq!(
        s.call(watch.clone()).unwrap(),
        json!({"url":picture,"audioUrl":sound,"referrer":page_url,"gain":-8,"peak":1,"qualities":[
            {"height":720,"url":picture},{"height":360,"url":"https://cdn.bilivideo.cn/360.m4s"}
        ]})
    );
    s.host.borrow_mut().fixture.as_mut().unwrap()[&pictures] = json!({"code":0,"data":{"quality":32,"durl":[{"url":combined}],
            "dash":{"video":[{"id":16,"baseUrl":picture}],"audio":[]}}});
    assert_eq!(
        s.call(watch.clone()).unwrap(),
        json!({"url":combined,"referrer":page_url,"qualities":[{"height":480,"url":combined}]})
    );
    for data in [
        json!({"dash":{"audio":[{"baseUrl":sound}]}}),
        json!({"is_preview":1,"durl":[{"url":combined}]}),
    ] {
        s.host.borrow_mut().fixture.as_mut().unwrap()[&pictures] = json!({"code":0,"data":data});
        assert!(s.call(watch.clone()).is_err());
    }
    s.host.borrow_mut().fixture.as_mut().unwrap()[format!("{query}&page=1")] = json!({"code":-352});
    assert!(
        s.call(Request::new("search", args))
            .unwrap_err()
            .to_string()
            .contains("限制")
    );
}

#[test]
fn bilibili_qr_verifies_before_saving_and_supports_sign_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = bilibili(dir.path());
    assert!(
        s.call(Request::new("account", json!({})))
            .unwrap()
            .is_null()
    );
    s.host
        .borrow_mut()
        .storage
        .set("session", json!({"SESSDATA":"previous"}))
        .unwrap();
    let passport = "https://passport.bilibili.com/x/passport-login/web/qrcode/";
    let qr = "https://account.bilibili.com/h5/account-h5/auth/scan-web";
    let nav = "https://api.bilibili.com/x/web-interface/nav";
    s.host.borrow_mut().fixture = Some(json!({
        (format!("{passport}generate")):{"responses":[
            {"code":0,"data":{"url":format!("{qr}/first"),"qrcode_key":"first"}},
            {"code":0,"data":{"url":format!("{qr}/second"),"qrcode_key":"second"}}]},
        (format!("{passport}poll?qrcode_key=first")):{"responses":[
            {"code":0,"data":{"code":86101}},{"code":0,"data":{"code":86090}},{"code":0,"data":{"code":86038}}]},
        (format!("{passport}poll?qrcode_key=second")):{"code":0,"_cookies":["SESSDATA=verified; Path=/; HttpOnly","other=skip; Path=/"],"data":{"code":0}},
        (nav):{"_expectCookie":"SESSDATA=verified","code":0,"data":{"isLogin":true,"mid":37,"uname":"B站测试"}}
    }));
    let start = start_qr_login(&mut s);
    let initial = poll_qr_after(&mut s, &start, 0).unwrap();
    assert!(initial["qr"].as_str().unwrap().ends_with("/first"));
    let mut poll = || poll_qr_after(&mut s, &start, 2100).unwrap();
    assert!(poll()["message"].as_str().unwrap().contains("扫码"));
    assert!(poll()["message"].as_str().unwrap().contains("已扫码"));
    assert!(poll()["qr"].as_str().unwrap().ends_with("/second"));
    let confirmed = poll();
    assert_eq!(confirmed["status"], "success");
    assert_eq!(confirmed["account"]["name"], "B站测试");
    assert_eq!(
        s.host.borrow().storage.get("session"),
        json!({"SESSDATA":"verified"})
    );
    drop(s);
    let mut reopened = bilibili(dir.path());
    reopened.host.borrow_mut().fixture = Some(
        json!({(nav):{"_expectCookie":"SESSDATA=verified","code":0,"data":{"isLogin":true,"mid":37,"uname":"B站测试"}}}),
    );
    assert_eq!(
        reopened.call(Request::new("account", json!({}))).unwrap()["id"],
        "37"
    );
    reopened.call(Request::new("signOut", json!({}))).unwrap();
    assert!(reopened.host.borrow().storage.get("session").is_null());
}

#[test]
fn bilibili_failed_relogin_preserves_previous_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = bilibili(dir.path());
    s.host
        .borrow_mut()
        .storage
        .set("session", json!({"SESSDATA":"previous"}))
        .unwrap();
    s.host.borrow_mut().fixture = Some(json!({
        "https://passport.bilibili.com/x/passport-login/web/qrcode/generate":{"code":0,"data":{"url":"https://account.bilibili.com/scan","qrcode_key":"new"}},
        "https://passport.bilibili.com/x/passport-login/web/qrcode/poll?qrcode_key=new":{"code":0,"data":{"code":0,"url":"https://passport.biligame.com/crossDomain?SESSDATA=unverified&ignored=ignore"}},
        "https://api.bilibili.com/x/web-interface/nav":{"_expectCookie":"SESSDATA=unverified","code":-101}
    }));
    let start = start_qr_login(&mut s);
    assert!(
        poll_qr_after(&mut s, &start, 2100)
            .unwrap_err()
            .to_string()
            .contains("未确认账号")
    );
    assert_eq!(
        s.host.borrow().storage.get("session")["SESSDATA"],
        "previous"
    );
}

#[test]
fn twenty_four_bit_combines_banks_and_requires_a_fresh_download_grant() {
    let root = tempfile::tempdir().unwrap();
    let pkg = Package::read(
        &bundle(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/24bit-js"
        )))
        .unwrap(),
    )
    .unwrap();
    let mut s = session(pkg, root.path());
    let query = json!({"keyword":"%E6%99%B4%E5%A4%A9","page":1});
    let grant = json!({"type":"c","name":"晴天","player":"歌手","album":"专辑"});
    let metadata = json!({"itemMusic":{"id":"abc","name":"晴天","album":"专辑","player":"歌手","url":"https://m701.music.126.net/test.flac"}});
    let html = format!(
        "<script>self.__next_f.push({})</script>",
        json!([1, format!("a:{metadata}\n")])
    );
    s.host.borrow_mut().fixture = Some(json!({
        "https://www.24bit.net/api/player/searchOnlineMusicOne":{"_expectBody":query,"status":true,"result":[{"id":"abc","name":"晴天","album":"专辑","player":"歌手"}]},
        "https://www.24bit.net/api/player/searchOnlineMusicTwo":{"_expectBody":query,"status":true,"result":[{"id":"abc","name":"晴天 (Live)","player":"歌手"}]},
        "https://www.24bit.net/music/c/abc":{"_text":html},
        "https://www.24bit.net/api/music/getOnlineDownload":{"responses":[{"_expectBody":grant,"status":true,"result":"limit"},{"_expectBody":grant,"status":false,"result":{"message":"拒绝下载"}},{"_expectBody":grant,"status":true,"result":true}]}
    }));
    let page = s
        .call(Request::new("search", json!({"query":"晴天"})))
        .unwrap();
    assert_eq!(page["items"][0]["key"]["id"], "c:abc");
    assert_eq!(page["items"][1]["key"]["id"], "b:abc");
    assert!(page["items"][0].get("downloadable").is_none());
    let args = json!({"id":"c:abc"});
    assert_eq!(
        s.call(Request::new("stream", args.clone())).unwrap()["url"],
        "https://m701.music.126.net/test.flac"
    );
    assert!(
        s.call(Request::new("download", args.clone()))
            .unwrap_err()
            .to_string()
            .contains("额度")
    );
    assert!(
        s.call(Request::new("download", args.clone()))
            .unwrap_err()
            .to_string()
            .contains("拒绝")
    );
    let file = s.call(Request::new("download", args)).unwrap();
    assert_eq!(file["url"], "https://m701.music.126.net/test.flac");
    assert!(
        file["attribution"]
            .as_str()
            .unwrap()
            .contains("/music/c/abc")
    );
}
