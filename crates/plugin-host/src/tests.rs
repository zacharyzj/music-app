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
fn trial_fallback_and_lyrics() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(upstream(), dir.path());
    s.host.borrow_mut().fixture = Some(
        json!({"song/enhance/player/url/v1":{"responses":[{"code":200,"data":[{"url":"https://m7.music.126.net/trial","freeTrialInfo":{}}]},{"code":200,"data":[{"url":"https://m7.music.126.net/full","freeTrialInfo":null,"level":"standard"}]}]},"song/lyric":{"code":200,"lrc":{"lyric":"[00:01.00]歌词"}}}),
    );
    let p = s
        .call(Request::new("stream", json!({"id":"7","quality":"high"})))
        .unwrap();
    assert_eq!(p["url"], "https://m7.music.126.net/full");
    assert_eq!(p["quality"], "standard");
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

#[test]
fn upstream_login_rotates_qr_and_commits_verified_account() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(upstream(), dir.path());
    s.host.borrow_mut().fixture = Some(json!({
        "login/qrcode/unikey":{"responses":[{"code":200,"unikey":"first-key"},{"code":200,"unikey":"second-key"}]},
        "login/qrcode/client/login":{"responses":[{"code":802},{"code":800},{"code":803,"_cookies":["MUSIC_U=verified; Path=/; HttpOnly"]}]},
        "w/nuser/account/get":{"code":200,"profile":{"userId":42,"nickname":"fixture-account"}}
    }));
    s.eval::<()>("const realNow=Date.now;let advance=0;Date.now=()=>realNow()+advance;")
        .unwrap();
    let start = s.call(Request::new("loginStart", json!({}))).unwrap();
    let args = json!({"key":start["key"]});
    s.eval::<()>("advance+=2000").unwrap();
    let waiting = s.call(Request::new("loginPoll", args.clone())).unwrap();
    assert_eq!(waiting["status"], "waiting");
    assert!(waiting["message"].as_str().unwrap().contains("已扫码"));
    s.eval::<()>("advance+=2000").unwrap();
    let rotated = s.call(Request::new("loginPoll", args.clone())).unwrap();
    assert!(rotated["qr"].as_str().unwrap().contains("second-key"));
    s.eval::<()>("advance+=2000").unwrap();
    let success = s.call(Request::new("loginPoll", args)).unwrap();
    assert_eq!(success["status"], "success");
    assert_eq!(success["account"]["id"], "42");
    assert_eq!(
        s.host.borrow().storage.get("cookies")["MUSIC_U"],
        "verified"
    );
    assert_eq!(s.host.borrow().storage.get("account")["id"], "42");
}
