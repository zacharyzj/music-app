use super::*;
use anyhow::{Context, ensure};
use rusqlite::TransactionBehavior;
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

// Local IDs keep their original on-disk representation. Plugin tracks need a snapshot
// so their titles and source remain visible after the plugin is removed.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Member {
    Local(String),
    Plugin(Box<Item>),
}

impl Member {
    fn key(&self) -> ResourceKey {
        match self {
            Self::Local(id) => ResourceKey {
                source: "local".into(),
                id: id.clone(),
            },
            Self::Plugin(item) => item.key.clone(),
        }
    }

    fn save_like(&self, db: &Connection) -> Result<()> {
        match self {
            Self::Local(id) => db.execute("INSERT OR IGNORE INTO liked(id) VALUES(?1)", [id])?,
            Self::Plugin(t) => db.execute(
                "INSERT INTO liked_plugin_tracks(source,id,data) VALUES(?1,?2,?3)
                 ON CONFLICT(source,id) DO UPDATE SET data=excluded.data",
                params![t.key.source, t.key.id, serde_json::to_string(t)?],
            )?,
        };
        Ok(())
    }
}

fn remove_like(db: &Connection, source: &str, id: &str) -> rusqlite::Result<usize> {
    if source == "local" {
        db.execute("DELETE FROM liked WHERE id=?1", [id])
    } else {
        db.execute(
            "DELETE FROM liked_plugin_tracks WHERE source=?1 AND id=?2",
            [source, id],
        )
    }
}

fn local_track(id: String, record: Option<(String, String)>) -> Result<Item> {
    if let Some((data, path)) = record {
        let mut item: Item = serde_json::from_str(&data)?;
        item.unavailable |= !Path::new(&path).is_file();
        return Ok(item);
    }
    Ok(Item {
        key: ResourceKey {
            source: "local".into(),
            id: id.clone(),
        },
        kind: "track".into(),
        title: id,
        unavailable: true,
        ..Default::default()
    })
}

impl Library {
    pub(super) fn edit_like(&self, id: &str, args: &Value) -> Result<()> {
        let source = args["trackSource"].as_str().unwrap_or("local");
        if args["on"] != true {
            remove_like(&self.db, source, id)?;
        } else if source == "local" {
            Member::Local(id.to_owned()).save_like(&self.db)?;
        } else {
            let track: Item = serde_json::from_value(args["track"].clone())?;
            ensure!(
                track.kind == "track"
                    && track.key.source == source
                    && track.key.id == id
                    && !id.is_empty(),
                "歌曲信息不匹配"
            );
            Member::Plugin(Box::new(track)).save_like(&self.db)?;
        }
        Ok(())
    }

    pub(super) fn liked_items(&self) -> Result<Vec<Item>> {
        let mut liked = self.db.prepare(
            "SELECT l.id,t.data,t.path FROM liked l LEFT JOIN tracks t ON t.id=l.id
             ORDER BY t.path IS NULL,t.path,l.rowid",
        )?;
        let mut items = Vec::new();
        let mut rows = liked.query([])?;
        while let Some(row) = rows.next()? {
            let data: Option<String> = row.get(1)?;
            items.push(local_track(row.get(0)?, data.zip(row.get(2)?))?);
        }
        let mut saved = self
            .db
            .prepare("SELECT data FROM liked_plugin_tracks ORDER BY rowid")?;
        for data in saved.query_map([], |r| r.get::<_, String>(0))? {
            items.push(serde_json::from_str(&data?)?);
        }
        Ok(items)
    }

    pub(super) fn playlist_items(&self, id: &str) -> Result<(String, Vec<Item>)> {
        let (name, data): (String, String) =
            self.db
                .query_row("SELECT name,tracks FROM playlists WHERE id=?1", [id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        let members: Vec<Member> = serde_json::from_str(&data)?;
        let mut local = self
            .db
            .prepare_cached("SELECT data,path FROM tracks WHERE id=?1")?;
        let mut items = Vec::with_capacity(members.len());
        for member in members {
            match member {
                Member::Plugin(item) => items.push(*item),
                Member::Local(id) => {
                    let row: Option<(String, String)> = local
                        .query_row([&id], |r| Ok((r.get(0)?, r.get(1)?)))
                        .optional()?;
                    items.push(local_track(id, row)?);
                }
            }
        }
        Ok((name, items))
    }
    fn change_members<T>(
        &mut self,
        target: Option<&str>,
        name: &str,
        edit: impl FnOnce(&mut Vec<Member>) -> Result<T>,
    ) -> Result<(String, T)> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (id, mut members) = match target {
            Some(id) => {
                let data: String = tx
                    .query_row("SELECT tracks FROM playlists WHERE id=?1", [id], |r| {
                        r.get(0)
                    })
                    .optional()?
                    .context("目标歌单已删除")?;
                (id.to_owned(), serde_json::from_str(&data)?)
            }
            None => {
                ensure!(!name.trim().is_empty(), "请输入新歌单名称");
                let id = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO playlists VALUES(?1,?2,'[]')",
                    params![id, name.trim()],
                )?;
                (id, Vec::new())
            }
        };
        let result = edit(&mut members)?;
        tx.execute(
            "UPDATE playlists SET tracks=?1 WHERE id=?2",
            params![serde_json::to_string(&members)?, id],
        )?;
        tx.commit()?;
        Ok((id, result))
    }

    pub(super) fn edit_playlist(
        &mut self,
        id: &str,
        key: ResourceKey,
        add: bool,
        snapshot: Option<Item>,
    ) -> Result<bool> {
        let (_, changed) = self.change_members(Some(id), "", |members| {
            let index = members.iter().position(|m| m.key() == key);
            match (add, index) {
                (true, None) => {
                    ensure!(!key.id.is_empty(), "歌曲编号无效");
                    if key.source == "local" {
                        members.push(Member::Local(key.id));
                    } else {
                        let track = snapshot
                            .filter(|t| t.kind == "track" && t.key == key)
                            .context("歌曲来源信息不匹配")?;
                        members.push(Member::Plugin(Box::new(track)));
                    }
                }
                (false, Some(i)) => {
                    members.remove(i);
                }
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        Ok(changed)
    }

    pub fn import_playlist(
        &mut self,
        target: Option<&str>,
        name: &str,
        tracks: &[Item],
    ) -> Result<(String, usize)> {
        self.change_members(target, name, |members| {
            let start = members.len();
            let mut positions: HashMap<_, _> = members
                .iter()
                .enumerate()
                .map(|(i, m)| (m.key(), i))
                .collect();
            for track in tracks {
                ensure!(
                    track.kind == "track"
                        && !track.key.id.is_empty()
                        && !track.key.source.is_empty()
                        && track.key.source != "local",
                    "插件歌曲信息无效"
                );
                let member = Member::Plugin(Box::new(track.clone()));
                if let Some(&i) = positions.get(&track.key) {
                    members[i] = member;
                } else {
                    positions.insert(track.key.clone(), members.len());
                    members.push(member);
                }
            }
            Ok(members.len() - start)
        })
    }

    fn replacement_member(
        &self,
        original: &ResourceKey,
        replacement: Item,
        unavailable_only: bool,
    ) -> Result<Member> {
        ensure!(
            replacement.kind == "track"
                && !replacement.unavailable
                && !replacement.key.id.is_empty()
                && !replacement.key.source.is_empty()
                && replacement.key.source != original.source,
            "请选择其他音乐源中可播放的歌曲"
        );
        if replacement.key.source == "local" {
            ensure!(
                !unavailable_only && self.path(&replacement.key.id)?.is_file(),
                "所选本地文件不可用"
            );
            Ok(Member::Local(replacement.key.id))
        } else {
            Ok(Member::Plugin(Box::new(replacement)))
        }
    }

    pub fn replace_liked_track(&mut self, original: &ResourceKey, replacement: Item) -> Result<()> {
        let member = self.replacement_member(original, replacement, false)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(
            remove_like(&tx, &original.source, &original.id)? == 1,
            "原曲已不在本地喜欢的歌曲中"
        );
        member.save_like(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn replace_playlist_track(
        &mut self,
        id: &str,
        original: &ResourceKey,
        replacement: Item,
        unavailable_only: bool,
    ) -> Result<()> {
        let new_member = self.replacement_member(original, replacement, unavailable_only)?;
        self.change_members(Some(id), "", |members| {
            let i = members
                .iter()
                .position(|m| {
                    m.key() == *original
                        && (!unavailable_only || matches!(m, Member::Plugin(t) if t.unavailable))
                })
                .context("原曲已变更，请重新打开歌单")?;
            let new_key = new_member.key();
            if let Some(existing) = members.iter().position(|m| m.key() == new_key) {
                members[existing] = new_member;
                members.remove(i);
            } else {
                members[i] = new_member;
            }
            Ok(())
        })?;
        Ok(())
    }
}
