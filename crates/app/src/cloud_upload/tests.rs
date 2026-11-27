use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
};

#[test]
fn nos_posts_exact_bytes_to_the_selected_object() -> Result<()> {
    let target = upload_url("http://nosup-jd1.127.net", "music/id + 1.flac")?;
    assert_eq!(
        target.as_str(),
        format!(
            "https://nosup-jd1.127.net/{BUCKET}/music%2Fid%20+%201.flac?offset=0&complete=true&version=1.0"
        )
    );
    for address in [
        "https://another.example",
        "https://nosup-jd1.127.net.example",
        "http://nosup-jd1.127.net:8080",
    ] {
        assert!(upload_url(address, "a").is_err());
    }
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let bytes = b"audio\0fixture";
    let digest = format!("{:x}", md5::compute(bytes));
    let expected = digest.clone();
    let server = std::thread::spawn(move || -> Result<()> {
        let (mut socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(&mut socket);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            ensure!(reader.read_line(&mut line)? > 0, "missing request headers");
            if line == "\r\n" {
                break;
            }
            headers.push_str(&line.to_ascii_lowercase());
        }
        assert!(headers.starts_with("post /file http/1.1"));
        assert!(headers.contains("x-nos-token: ticket\r\n"));
        assert!(headers.contains(&format!("content-md5: {expected}\r\n")));
        assert!(headers.contains(&format!("content-length: {}\r\n", bytes.len())));
        assert!(!headers.contains("cookie:"));
        let mut payload = [0; 13];
        reader.read_exact(&mut payload)?;
        assert_eq!(&payload, bytes);
        let reply = format!("{{\"offset\":{}}}", bytes.len());
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        )?;
        Ok(())
    });
    let mut audio = tempfile::tempfile()?;
    audio.write_all(bytes)?;
    audio.rewind()?;
    let ticket = Ticket {
        need_upload: true,
        token: "ticket".into(),
        object_key: "file".into(),
        resource_id: "91".into(),
        song_id: "0".into(),
    };
    post_file(
        &Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()?,
        Url::parse(&format!("http://{address}/file"))?,
        &ticket,
        audio,
        bytes.len() as u64,
        &digest,
        "flac",
    )?;
    server.join().unwrap()?;
    Ok(())
}

#[cfg(feature = "ui-smoke")]
#[test]
fn playlist_and_queue_upload_use_the_original_source_grant_and_continue_after_denial() -> Result<()>
{
    use crate::{test_ui, workers::Workers};
    use music_core::{Manifest, Queue, ResourceKey};
    use std::fs;
    let _clock = test_ui::test_clock()?;
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    let fixture = tempfile::tempdir()?;
    let root = fixture.path();
    fs::create_dir(root.join("plugins"))?;
    let plugin = |id: &str, caps: Value, code: &str| -> Result<(Manifest, bool)> {
        let source = root.join(id);
        fs::create_dir(&source)?;
        fs::write(source.join("plugin.json"),json!({"id":id,"name":id,"main":"index.js","version":"1","capabilities":caps,"permissions":{"media":["media.example"]}}).to_string())?;
        fs::write(source.join("index.js"), code)?;
        let bytes = music_plugin_host::bundle(&source)?;
        fs::write(root.join("plugins").join(format!("{id}.ypm.zip")), &bytes)?;
        Ok((music_plugin_host::manifest(&bytes)?, true))
    };
    let target = plugin(
        "netease",
        json!(["cloudUpload"]),
        r#"
        export function cloudUpload(a) {
          if(a.step==="account") return {id:"account-1",name:"测试账号"};
          if(a.accountId!=="account-1") throw Error("wrong account");
          if(a.step==="session") return true;
          if(a.step==="prepare" && a.md5.length===32 && a.size>44 && a.ext==="wav" && a.song==="本地录音")
            return {needUpload:false,objectKey:"",token:"",resourceId:"29",songId:"12"};
          if(a.step==="finish" && a.resourceId==="29" && a.songId==="12") return {songId:"88"};
          throw Error("unexpected upload request");
        }"#,
    )?;
    let online = plugin(
        "fixture",
        json!(["download"]),
        r#"
        export function stream(){throw Error("unexpected playback request")}
        export function download({id}){
            if(id==="denied") throw Error("当前账号无下载权限");
            return {url:"https://outside.example/track.wav"};
        }"#,
    )?;
    let mut db = Library::open(root)?;
    let folder = root.join("downloads");
    fs::create_dir(&folder)?;
    let path = folder.join("test.wav");
    let mut wav = File::create(&path)?;
    let data = [0u8; 16000];
    wav.write_all(b"RIFF")?;
    wav.write_all(&(36 + data.len() as u32).to_le_bytes())?;
    wav.write_all(b"WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\x40\x1f\x00\x00\x80\x3e\x00\x00\x02\x00\x10\x00data")?;
    wav.write_all(&(data.len() as u32).to_le_bytes())?;
    wav.write_all(&data)?;
    drop(wav);
    let song = |id: &str| Item {
        key: ResourceKey {
            source: "fixture".into(),
            id: id.into(),
        },
        kind: "track".into(),
        title: id.into(),
        ..Default::default()
    };
    let cached = song("cached");
    db.index_download(&path, cached.clone())?;
    let mut local = db.tracks()?.remove(0);
    local.title = "本地录音".into();
    let mut queue = Queue::default();
    queue.items.push(local.clone());
    let state = Rc::new(RefCell::new(UiState::new(
        queue,
        Default::default(),
        vec![target, online],
    )));
    state.borrow_mut().local_playlist.tracks =
        vec![song("denied"), song("invalid-grant"), local, cached];
    let library = Arc::new(Mutex::new(db));
    let (jobs, rx) = mpsc::channel();
    let (cloud_reply, responses) = mpsc::channel();
    Workers {
        root: root.into(),
        library: library.clone(),
        generation: Default::default(),
        out: mpsc::channel().0,
        download_tx: mpsc::channel().0,
        cloud_reply,
    }
    .start(rx, Arc::new(AtomicBool::new(false)));
    let ui = AppWindow::new()?;
    let updates = install(
        &ui,
        state,
        library,
        jobs,
        responses,
        Arc::new(AtomicBool::new(false)),
    );
    for (queue, count) in [(false, 4), (true, 1)] {
        ui.invoke_cloud_upload(queue);
        ui.invoke_cloud_upload(queue);
        assert!(ui.get_show_cloud_upload() && ui.get_cloud_busy());
        assert_eq!(ui.get_cloud_rows().row_count(), count);
        while ui.get_cloud_busy() {
            present(&ui, updates.recv_timeout(Duration::from_secs(8))?);
        }
        let rows = ui.get_cloud_rows();
        if queue {
            assert_eq!(rows.row_data(0).unwrap().status, "已秒传");
        } else {
            assert!(
                rows.row_data(0)
                    .unwrap()
                    .status
                    .contains("当前账号无下载权限")
            );
            assert!(rows.row_data(1).unwrap().status.contains("媒体域名"));
            assert_eq!(rows.row_data(2).unwrap().status, "已秒传");
            assert_eq!(rows.row_data(3).unwrap().status, "相同文件已上传");
            assert!(ui.get_cloud_status().contains("完成 2 首，失败 2 首"));
        }
    }
    Ok(())
}
