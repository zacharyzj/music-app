use super::*;

#[test]
fn accepts_plugin_ids_and_netease_playlist_links() {
    for value in [
        "443",
        "https://music.163.com/#/playlist?id=443",
        "https://music.163.com/playlist?id=443&userid=2",
        "https://y.music.163.com/m/playlist/443",
    ] {
        assert_eq!(playlist_id("netease", value).unwrap(), "443");
    }
    assert_eq!(playlist_id("another", "abc/123").unwrap(), "abc/123");
    assert_eq!(
        playlist_id("another", "https://provider.example/lists/ab").unwrap(),
        "https://provider.example/lists/ab"
    );
    assert!(playlist_id("netease", "https://music.163.com/song?id=443").is_err());
    assert!(playlist_id("netease", "https://other.example/playlist?id=443").is_err());
}

#[test]
fn incomplete_or_repeated_pages_fail_the_entire_import() {
    let read = |_: u64| {
        Ok(Page {
            items: vec![Item {
                key: ResourceKey {
                    source: "wrong".into(),
                    id: "7".into(),
                },
                kind: "track".into(),
                ..Default::default()
            }],
            more: true,
            ..Default::default()
        })
    };
    assert!(read_playlist("test", read).is_err());
    assert!(
        read_playlist("test", |offset| if offset == 0 {
            read(offset)
        } else {
            anyhow::bail!("offline")
        })
        .is_err()
    );
}

#[cfg(feature = "ui-smoke")]
#[test]
fn imports_with_installed_plugins_and_only_replaces_after_manual_selection() -> Result<()> {
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    let _clock = crate::test_ui::test_clock()?;
    let ui = AppWindow::new()?;
    ui.show()?;
    ui.window().set_size(slint::PhysicalSize::new(720, 480));
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::WindowActiveChanged(true));
    let dir = tempfile::tempdir()?;
    let packages = dir.path().join("plugins");
    std::fs::create_dir(&packages)?;
    let script = r#"
        export function playlists() {return [{id:'test',name:'My mix'}];}
        export function playlist({id,offset}) {
            if(id==='failed' && offset) throw Error('page failed');
            if(id==='later') return {name:'Later',tracks:[{id:'extra',name:'Extra'}]};
            return {name:'My mix', more:!offset, tracks:offset
                ? [{id:'lost',name:'Lost',artists:[{name:'Singer'}],unavailable:true}]
                : [{id:'same',name:'Original'}]};
        }
        export function search({kind,query}) { return {tracks:query==='Lost Singer' && kind==='tracks'
            ? [{id:'same',name:'Other version'},{id:'bad',name:'Gone',unavailable:true}] : []}; }
        export function stream({id}) {return {url:'https://media.example/'+id};}
    "#;
    for (id, caps) in [
        ("original", json!([])),
        ("other", json!(["tracksOnly"])),
        ("off", json!([])),
    ] {
        let source = dir.path().join(id);
        std::fs::create_dir(&source)?;
        std::fs::write(source.join("plugin.json"),json!({"id":id,"version":"1","name":id,"main":"index.js","capabilities":caps,"navigation":[{"builtin":"playlists"}],"permissions":{"media":["media.example"]}}).to_string())?;
        std::fs::write(source.join("index.js"), script)?;
        std::fs::write(
            packages.join(format!("{id}.ypm.zip")),
            music_plugin_host::bundle(&source)?,
        )?;
    }
    let db = Library::open(dir.path())?;
    db.set("disabled:off", json!(true))?;
    let plugins = plugin_list(&packages, &db);
    let library = Arc::new(Mutex::new(db));
    let genr = Arc::new(AtomicU64::new(1));
    let (jobs, rx) = mpsc::channel();
    let (out, replies) = mpsc::channel();
    workers::Workers {
        root: dir.path().into(),
        library: library.clone(),
        generation: genr.clone(),
        out,
        download_tx: mpsc::channel().0,
    }
    .start(rx, Arc::new(AtomicBool::new(false)));
    let state = Rc::new(RefCell::new(UiState::new(
        Queue::default(),
        Default::default(),
        plugins,
    )));
    update_plugins(&ui, &mut state.borrow_mut());
    install(&ui, state.clone(), jobs.clone(), genr.clone());
    let receive = |op: &str| -> Result<Reply> {
        loop {
            let reply = replies.recv_timeout(Duration::from_secs(10))?;
            if reply.op == op {
                return Ok(reply);
            }
            ensure!(
                !reply.op.starts_with("import-"),
                "unexpected import reply: {}",
                reply.op
            );
        }
    };
    let finish = |op: &str| -> Result<()> {
        let reply = receive(op)?;
        state
            .borrow_mut()
            .finish_import_reply(&ui, &jobs, &genr, reply);
        Ok(())
    };
    let action = |a: &str, i| ui.invoke_import_action(a.into(), i);
    ui.window().take_snapshot()?;
    crate::test_ui::click(ui.window(), 660., 75.);
    finish("import-targets")?;
    assert_eq!(
        ui.get_import_sources().iter().collect::<Vec<_>>(),
        ["original"]
    );
    ui.set_import_id("failed".into());
    action("submit", 0);
    finish("import-playlist")?;
    assert!(library.lock().unwrap().playlist_choices()?.is_empty());
    assert_eq!(ui.get_import_step(), ImportStep::Form);

    assert!(ui.get_import_can_browse());
    action("browse", 0);
    finish("import-choices")?;
    assert_eq!(ui.get_import_choices().row_count(), 2);
    action("pick", 1);
    assert_eq!(ui.get_import_id(), "test");
    action("submit", 0);
    finish("import-playlist")?;
    assert_eq!(ui.get_import_step(), ImportStep::Confirm);
    assert!(replies.try_recv().is_err());
    ui.window().take_snapshot()?;
    let dest = library.lock().unwrap().playlist_choices()?[0].clone();
    assert_eq!(dest.title, "My mix");
    let saved = || -> Result<Vec<Item>> {
        let page = library
            .lock()
            .unwrap()
            .call(Request::new("playlist", json!({"id":dest.key.id})))?;
        Ok(serde_json::from_value::<Page>(page)?.items)
    };
    assert!(saved()?[1].unavailable);
    // Declining leaves the original entry in its position. Re-import into that same local playlist.
    action("close", 0);
    assert_eq!(ui.get_import_step(), ImportStep::Closed);
    action("open", 0);
    finish("import-targets")?;
    ui.set_import_target_index(1);
    ui.set_import_id("test".into());
    action("submit", 0);
    finish("import-playlist")?;
    ui.window().take_snapshot()?;
    crate::test_ui::click(ui.window(), 550., 310.);
    finish("import-search")?;
    assert_eq!(ui.get_import_candidates().row_count(), 1);
    assert!(
        ui.get_import_candidates()
            .row_data(0)
            .unwrap()
            .detail
            .contains("other")
    );
    assert!(saved()?[1].unavailable);
    ui.window().take_snapshot()?;
    crate::test_ui::click(ui.window(), 550., 198.);
    finish("import-replace")?;
    assert_eq!(ui.get_import_step(), ImportStep::Closed);
    assert_eq!(library.lock().unwrap().playlist_choices()?.len(), 1);
    let mut items = saved()?;
    assert_eq!(
        items
            .iter()
            .map(|i| (&*i.key.source, &*i.key.id))
            .collect::<Vec<_>>(),
        [("original", "same"), ("other", "same")]
    );

    let mut s = state.borrow_mut();
    let (cover_tx, _) = mpsc::sync_channel(2);
    prepare_items(&mut items, &s.plugins, "local", &cover_tx, 0);
    s.items = items.clone();
    s.replace_queue_from_page();
    assert_eq!(s.queue.items.len(), 2);
    let (commands, _keep_player) = mpsc::channel();
    play(&ui, &mut s, &jobs, &commands, items[1].clone(), false);
    let resolved = receive("resolve")?;
    assert_eq!(resolved.source, "other");
    assert_eq!(resolved.value.unwrap()["url"], "https://media.example/same");

    let index = s.plugins.iter().position(|(m, _)| m.id == "other").unwrap();
    s.plugins[index].1 = false;
    s.update_rows(&ui);
    assert!(ui.get_rows().row_data(1).unwrap().disabled);
    let (unexpected, dispatch) = mpsc::channel();
    play(&ui, &mut s, &unexpected, &commands, items[1].clone(), false);
    assert!(dispatch.try_recv().is_err());
    s.queue.index = Some(0);
    assert!(s.advance_available(true).is_none());
    s.replace_queue_from_page();
    assert_eq!(s.queue.items.len(), 1);
    s.plugins[index].1 = true;
    s.update_rows(&ui);
    assert!(!ui.get_rows().row_data(1).unwrap().disabled);
    Ok(())
}
