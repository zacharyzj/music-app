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
        export function search({kind,query,offset}) { return {more:!offset && query==='Lost Singer',tracks:['Lost Singer','Other version'].includes(query) && kind==='tracks'
            ? [{id:offset?'later':'same',name:'Other version'},{id:'bad',name:'Gone',unavailable:true}] : []}; }
        export function stream({id}) {return {url:'https://media.example/'+id};}
    "#;
    for (id, caps) in [
        ("original", json!([])),
        ("another", json!(["tracksOnly"])),
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
        cloud_reply: mpsc::channel().0,
        download_tx: mpsc::channel().0,
    }
    .start(rx, Arc::new(AtomicBool::new(false)));
    let state = Rc::new(RefCell::new(UiState::new(
        Queue::default(),
        Default::default(),
        plugins,
    )));
    update_plugins(&ui, &mut state.borrow_mut());
    let (commands, player_commands) = mpsc::channel();
    install(
        &ui,
        state.clone(),
        jobs.clone(),
        commands.clone(),
        genr.clone(),
    );
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
    assert_eq!(
        ui.get_import_filter_sources().iter().collect::<Vec<_>>(),
        ["全部来源", "another", "other"]
    );
    assert_eq!(ui.get_import_filter_index(), 0);
    let mut first = [receive("import-search")?, receive("import-search")?];
    first.sort_by(|a, b| a.source.cmp(&b.source));
    let [another, other] = first;
    state
        .borrow_mut()
        .finish_import_reply(&ui, &jobs, &genr, another);
    action("filter", 2);
    assert_eq!(ui.get_import_candidates().row_count(), 0);
    assert!(ui.get_import_status().contains("搜索中"));
    state
        .borrow_mut()
        .finish_import_reply(&ui, &jobs, &genr, other);
    action("filter", 0);
    assert_eq!(ui.get_import_candidates().row_count(), 2);
    action("filter", 2);
    assert_eq!(ui.get_import_candidates().row_count(), 1);
    assert!(ui.get_import_more());
    action("more", 0);
    let next = receive("import-search")?;
    assert_eq!(next.source, "other");
    assert!(next.args["offset"].as_u64().unwrap() > 0);
    state
        .borrow_mut()
        .finish_import_reply(&ui, &jobs, &genr, next);
    assert_eq!(ui.get_import_candidates().row_count(), 2);
    action("filter", 1);
    ui.set_import_query("Other version".into());
    action("search", 0);
    finish("import-search")?;
    assert_eq!(ui.get_import_filter_index(), 1);
    assert_eq!(ui.get_import_candidates().row_count(), 1);
    action("filter", 2);
    finish("import-search")?;
    assert!(
        ui.get_import_candidates()
            .row_data(0)
            .unwrap()
            .detail
            .contains("other")
    );
    assert!(saved()?[1].unavailable);
    ui.window().take_snapshot()?;
    let original = saved()?[0].clone();
    {
        let mut s = state.borrow_mut();
        s.normalize = true;
        s.queue.items = vec![
            original.clone(),
            Item {
                title: "Up next".into(),
                key: ResourceKey {
                    source: "local".into(),
                    id: "next".into(),
                },
                kind: "track".into(),
                ..Default::default()
            },
        ];
        s.queue.index = Some(0);
        s.queue.album_origin = Some(original.key.clone());
    }
    let previous_queue = json!(state.borrow().queue);
    crate::test_ui::click(ui.window(), 530., 198.);
    let first = receive("resolve")?;
    assert_eq!(first.source, "other");
    assert_eq!(first.value.unwrap()["url"], "https://media.example/same");
    assert!(ui.get_version_preview().loading);
    assert!(!ui.get_import_busy());
    assert!(matches!(player_commands.try_recv()?, Command::Stop));
    action("preview", 0);
    assert_eq!(state.borrow().play_generation, first.generation);
    action("filter", 1);
    action("preview", 0);
    let second = receive("resolve")?;
    assert_eq!(second.source, "another");
    assert!(second.generation > first.generation);
    assert_eq!(ui.get_version_preview().resource, "another:same");
    assert!(matches!(player_commands.try_recv()?, Command::Stop));
    {
        let mut s = state.borrow_mut();
        let track = s.now_playing.as_ref().unwrap();
        assert!(track.use_local_favorites);
        assert_eq!(s.normalization(), music_player::Normalization::Track);
        let key = track.key.clone();
        s.stream_started(&ui, key);
    }
    assert!(!ui.get_version_preview().loading);
    action("preview", 0);
    assert!(matches!(player_commands.try_recv()?, Command::Pause(true)));
    assert!(!ui.get_playing());
    action("preview", 0);
    assert!(matches!(player_commands.try_recv()?, Command::Pause(false)));
    assert!(ui.get_playing());
    state.borrow_mut().playback_ended(&ui, &jobs, &commands);
    assert!(!ui.get_playing());
    assert!(ui.get_version_preview().resource.is_empty());
    assert_eq!(json!(state.borrow().queue), previous_queue);
    assert!(matches!(
        state.borrow().normalization(),
        music_player::Normalization::Album(_)
    ));
    assert!(saved()?[1].unavailable);
    action("filter", 2);
    ui.window().take_snapshot()?;
    crate::test_ui::click(ui.window(), 577., 198.);
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
    play(
        &ui,
        &mut s,
        &jobs,
        &commands,
        items[1].clone(),
        PlayKind::Audio,
    );
    let resolved = receive("resolve")?;
    assert_eq!(resolved.source, "other");
    assert_eq!(resolved.value.unwrap()["url"], "https://media.example/same");

    let index = s.plugins.iter().position(|(m, _)| m.id == "other").unwrap();
    s.plugins[index].1 = false;
    s.update_rows(&ui);
    assert!(ui.get_rows().row_data(1).unwrap().disabled);
    let (unexpected, dispatch) = mpsc::channel();
    play(
        &ui,
        &mut s,
        &unexpected,
        &commands,
        items[1].clone(),
        PlayKind::Audio,
    );
    assert!(dispatch.try_recv().is_err());
    s.queue.index = Some(0);
    assert!(s.advance_available(true).is_none());
    s.replace_queue_from_page();
    assert_eq!(s.queue.items.len(), 1);
    s.plugins[index].1 = true;
    s.update_rows(&ui);
    assert!(!ui.get_rows().row_data(1).unwrap().disabled);
    s.last = Request::new("playlist", json!({"id":dest.key.id}));
    ui.set_local_playlist_kind(LocalPlaylistKind::Custom);
    s.open_track_details(&ui, &jobs, items[1].clone());
    drop(s);
    assert_eq!(ui.get_import_step(), ImportStep::Details);
    assert_eq!(ui.get_track_details().source, "other");
    let choices = ui.get_import_filter_sources().iter().collect::<Vec<_>>();
    assert_eq!(
        choices,
        [
            "全部可选来源",
            "本地音乐",
            "another",
            "original",
            "other（当前）"
        ]
    );
    assert_eq!(ui.get_import_filter_index(), 4);
    assert!(ui.get_choosing_version());
    let current = receive("import-search")?;
    assert_eq!(current.source, "other");
    state
        .borrow_mut()
        .finish_import_reply(&ui, &jobs, &genr, current);
    assert_eq!(
        ui.get_import_candidates().row_data(0).unwrap().resource,
        "other:same"
    );
    action("choose", 0);
    finish("import-change-source")?;
    assert_eq!(ui.get_import_step(), ImportStep::Closed);
    assert_eq!(saved()?.len(), 2);
    state
        .borrow_mut()
        .open_track_details(&ui, &jobs, saved()?[1].clone());
    finish("import-search")?;
    ui.set_import_query("Lost Singer".into());
    action("search", 0);
    finish("import-search")?;
    assert_eq!(ui.get_import_filter_index(), 4);
    assert!(ui.get_import_more());
    action("more", 0);
    finish("import-search")?;
    action("preview", 1);
    let preview = receive("resolve")?;
    assert_eq!(preview.source, "other");
    assert_eq!(preview.args["id"], "later");
    assert_eq!(ui.get_import_step(), ImportStep::Details);
    assert_eq!(saved()?[1].key.id, "same");
    let playlist = state
        .borrow_mut()
        .import
        .playlist_id
        .replace("deleted".into());
    action("choose", 1);
    finish("import-change-source")?;
    assert_eq!(ui.get_import_step(), ImportStep::Details);
    assert!(!ui.get_import_busy());
    assert_eq!(saved()?[1].key.id, "same");
    state.borrow_mut().import.playlist_id = playlist;
    action("choose", 1);
    finish("import-change-source")?;
    assert_eq!(ui.get_import_step(), ImportStep::Closed);
    assert_eq!(saved()?[1].key.id, "later");
    state
        .borrow_mut()
        .open_track_details(&ui, &jobs, saved()?[1].clone());
    finish("import-search")?;
    action("filter", 2);
    finish("import-search")?;
    assert_eq!(ui.get_import_step(), ImportStep::Details);
    action("choose", 0);
    finish("import-change-source")?;
    assert_eq!(ui.get_import_step(), ImportStep::Closed);
    assert_eq!(saved()?[1].key.source, "another");

    // A disabled source opens the alternatives immediately, using the saved song as the query.
    let mut s = state.borrow_mut();
    s.plugins
        .iter_mut()
        .find(|(p, _)| p.id == "another")
        .unwrap()
        .1 = false;
    s.open_track_details(&ui, &jobs, saved()?[1].clone());
    drop(s);
    assert!(ui.get_choosing_version());
    assert_eq!(ui.get_import_filter_index(), 0);
    assert_eq!(
        ui.get_import_filter_sources().iter().collect::<Vec<_>>(),
        ["全部可选来源", "本地音乐", "original", "other"]
    );
    for _ in 0..3 {
        finish("import-search")?;
    }
    let versions = ui.get_import_candidates();
    assert_eq!(versions.row_count(), 2);
    let other = versions
        .iter()
        .position(|v| v.resource == "other:same")
        .unwrap() as i32;
    assert_eq!(saved()?[1].key.source, "another");
    action("choose", other);
    finish("import-change-source")?;
    assert_eq!(saved()?[1].key.source, "other");
    assert!(!ui.get_choosing_version());
    assert_eq!(ui.get_import_step(), ImportStep::Closed);

    let mut favorite = saved()?[1].clone();
    favorite.use_local_favorites = true;
    library.lock().unwrap().call(Request::new(
        "like",
        json!({"on":true,"id":favorite.key.id,"trackSource":favorite.key.source,"track":favorite}),
    ))?;
    let mut s = state.borrow_mut();
    s.last = Request::new("liked", json!({}));
    s.now_playing = Some(favorite.clone());
    s.likes.insert(favorites::FavoriteKey::of(&favorite), true);
    ui.set_local_playlist_kind(LocalPlaylistKind::Favorites);
    s.open_track_details(&ui, &jobs, favorite);
    drop(s);
    finish("import-search")?;
    let source = ui
        .get_import_filter_sources()
        .iter()
        .position(|s| s == "original")
        .unwrap();
    action("filter", source as i32);
    finish("import-search")?;
    action("choose", 0);
    finish("import-change-liked-source")?;
    let reload = receive("liked")?;
    let liked: Page = serde_json::from_value(reload.value.unwrap())?;
    assert_eq!(liked.items.len(), 1);
    assert_eq!(liked.items[0].key.source, "original");
    assert_eq!(ui.get_import_step(), ImportStep::Closed);
    assert_eq!(ui.get_current_favorite(), FavoriteState::Unliked);
    assert_eq!(saved()?[1].key.source, "other");
    Ok(())
}
