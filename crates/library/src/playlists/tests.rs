use super::*;

fn track(source: &str, id: &str, unavailable: bool) -> Item {
    Item {
        key: ResourceKey {
            source: source.into(),
            id: id.into(),
        },
        title: format!("{source} {id}"),
        kind: "track".into(),
        unavailable,
        ..Default::default()
    }
}

#[test]
fn legacy_local_members_and_mixed_source_ids_survive_reopen_and_edits() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let mut db = Library::open(tmp.path())?;
    db.db.execute(
        "INSERT INTO tracks VALUES('7','music.mp3','','',?1)",
        [serde_json::to_string(&track("local", "7", false))?],
    )?;
    db.db
        .execute("INSERT INTO playlists VALUES('old','旧歌单','[\"7\"]')", [])?;
    let songs = [
        track("first", "7", false),
        track("first", "8", true),
        track("second", "7", true),
    ];
    assert_eq!(db.import_playlist(Some("old"), "", &songs)?.1, 3);
    assert_eq!(db.import_playlist(Some("old"), "", &songs)?.1, 0);
    assert!(
        db.import_playlist(
            None,
            "失败的歌单",
            &[songs[0].clone(), track("local", "x", false)]
        )
        .is_err()
    );
    assert!(db.import_playlist(Some("deleted"), "", &songs).is_err());
    drop(db);
    let mut db = Library::open(tmp.path())?;
    assert_eq!(db.playlist_choices()?.len(), 1);
    let read = |db: &mut Library| -> Result<Vec<ResourceKey>> {
        let response = db.call(Request::new("playlist", json!({"id":"old"})))?;
        Ok(serde_json::from_value::<Page>(response)?
            .items
            .into_iter()
            .map(|t| t.key)
            .collect())
    };
    assert_eq!(
        read(&mut db)?,
        [
            track("local", "7", false).key,
            songs[0].key.clone(),
            songs[1].key.clone(),
            songs[2].key.clone()
        ]
    );
    db.replace_playlist_track("old", &songs[1].key, track("second", "7", false), true)?;
    assert!(!db.playlist_items("old")?.1.last().unwrap().unavailable);
    db.call(Request::new(
        "removeFromPlaylist",
        json!({"id":"old","source":"first","track":"7"}),
    ))?;
    assert_eq!(
        read(&mut db)?,
        [track("local", "7", false).key, songs[2].key.clone()]
    );
    let playable = track("second", "7", false);
    let (copy, _) = db.import_playlist(None, "另一张歌单", std::slice::from_ref(&playable))?;
    assert!(
        db.replace_playlist_track("old", &playable.key, songs[0].clone(), true)
            .is_err()
    );
    db.replace_playlist_track("old", &playable.key, songs[0].clone(), false)?;
    assert_eq!(db.playlist_items(&copy)?.1[0].key, playable.key);
    assert_eq!(
        db.call(Request::new(
            "addToPlaylist",
            json!({"id":copy,"track":"7","source":"first","item":songs[0]})
        ))?,
        true
    );
    assert_eq!(db.playlist_items(&copy)?.1[1].key, songs[0].key);
    assert!(db.playlist_items("old")?.1[0].unavailable);
    assert!(
        db.replace_playlist_track("old", &songs[0].key, track("local", "7", false), false)
            .is_err()
    );
    let file = tmp.path().join("song.mp3");
    std::fs::write(&file, b"audio")?;
    db.db.execute(
        "UPDATE tracks SET path=?1 WHERE id='7'",
        [file.to_string_lossy().as_ref()],
    )?;
    db.replace_playlist_track("old", &songs[0].key, track("local", "7", false), false)?;
    assert_eq!(read(&mut db)?, [track("local", "7", false).key]);
    assert!(!db.playlist_items("old")?.1[0].unavailable);
    Ok(())
}
