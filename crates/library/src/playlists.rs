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
}

impl Library {
    pub(super) fn playlist_items(&self, id: &str) -> Result<(String, Vec<Item>)> {
        let (name, data): (String, String) =
            self.db
                .query_row("SELECT name,tracks FROM playlists WHERE id=?1", [id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        let members: Vec<Member> = serde_json::from_str(&data)?;
        let mut local = self
            .db
            .prepare_cached("SELECT data FROM tracks WHERE id=?1")?;
        let mut items = Vec::with_capacity(members.len());
        for member in members {
            match member {
                Member::Plugin(item) => items.push(*item),
                Member::Local(id) => {
                    let row: Option<String> = local.query_row([id], |r| r.get(0)).optional()?;
                    if let Some(item) = row.and_then(|data| serde_json::from_str(&data).ok()) {
                        items.push(item);
                    }
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

    pub(super) fn edit_playlist(&mut self, id: &str, key: ResourceKey, add: bool) -> Result<bool> {
        let (_, changed) = self.change_members(Some(id), "", |members| {
            let index = members.iter().position(|m| m.key() == key);
            match (add, index) {
                (true, None) => {
                    ensure!(
                        key.source == "local" && !key.id.is_empty(),
                        "本地歌曲编号无效"
                    );
                    members.push(Member::Local(key.id));
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

    pub fn replace_unavailable(
        &mut self,
        id: &str,
        original: &ResourceKey,
        replacement: Item,
    ) -> Result<()> {
        ensure!(
            replacement.kind == "track"
                && !replacement.unavailable
                && !replacement.key.id.is_empty()
                && !replacement.key.source.is_empty()
                && replacement.key.source != "local"
                && replacement.key.source != original.source,
            "请选择其他音乐源中可播放的歌曲"
        );
        self.change_members(Some(id), "", |members| {
            let i = members
                .iter()
                .position(|m| matches!(m, Member::Plugin(t) if t.key == *original && t.unavailable))
                .context("原曲已被移除或已恢复可用，请重新打开歌单")?;
            if let Some(existing) = members.iter().position(|m| m.key() == replacement.key) {
                members[existing] = Member::Plugin(Box::new(replacement));
                members.remove(i);
            } else {
                members[i] = Member::Plugin(Box::new(replacement));
            }
            Ok(())
        })?;
        Ok(())
    }
}
