use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, OsRng, rand_core::RngCore},
};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub struct Storage {
    path: PathBuf,
    key: [u8; 32],
    data: Value,
}
impl Storage {
    pub fn open(root: &Path, id: &str) -> Result<Self> {
        let entry = keyring::Entry::new("music-api", "plugin-storage-key")?;
        let key = match entry.get_password() {
            Ok(s) => hex::decode(s)?,
            Err(keyring::Error::NoEntry) => {
                let mut key = [0; 32];
                OsRng.fill_bytes(&mut key);
                entry.set_password(&hex::encode(key))?;
                key.to_vec()
            }
            Err(e) => return Err(e.into()),
        };
        Self::with_key(
            root.join(format!("{id}.enc")),
            key.try_into().map_err(|_| anyhow!("凭据密钥无效"))?,
        )
    }
    pub(crate) fn with_key(path: PathBuf, key: [u8; 32]) -> Result<Self> {
        std::fs::create_dir_all(path.parent().ok_or_else(|| anyhow!("存储目录无效"))?)?;
        let data = if path.exists() {
            let bytes = std::fs::read(&path)?;
            ensure!(
                (12..=8 * 1024 * 1024 + 28).contains(&bytes.len()),
                "插件存储损坏或过大"
            );
            let plain = Aes256Gcm::new_from_slice(&key)
                .unwrap()
                .decrypt((&bytes[..12]).into(), &bytes[12..])
                .map_err(|_| anyhow!("无法解密插件存储"))?;
            serde_json::from_slice(&plain)?
        } else {
            json!({})
        };
        ensure!(data.is_object(), "插件存储格式无效");
        Ok(Self { path, key, data })
    }
    pub fn get(&self, key: &str) -> Value {
        self.data[key].clone()
    }
    pub fn set(&mut self, key: &str, value: Value) -> Result<()> {
        let mut updated = self.data.clone();
        updated[key] = value;
        self.write(updated)
    }
    pub fn remove(&mut self, key: &str) -> Result<()> {
        let mut updated = self.data.clone();
        updated.as_object_mut().unwrap().remove(key);
        self.write(updated)
    }
    fn write(&mut self, updated: Value) -> Result<()> {
        let plain = serde_json::to_vec(&updated)?;
        ensure!(plain.len() <= 8 * 1024 * 1024, "插件存储超过限制");
        let mut nonce = [0; 12];
        OsRng.fill_bytes(&mut nonce);
        let mut data = nonce.to_vec();
        data.extend(
            Aes256Gcm::new_from_slice(&self.key)
                .unwrap()
                .encrypt((&nonce).into(), plain.as_slice())
                .map_err(|_| anyhow!("加密失败"))?,
        );
        let mut file = tempfile::NamedTempFile::new_in(self.path.parent().unwrap())?;
        file.write_all(&data)?;
        file.as_file().sync_all()?;
        file.persist(&self.path)?;
        self.data = updated;
        Ok(())
    }
}
