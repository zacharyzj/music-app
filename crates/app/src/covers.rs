use music_core::ResourceKey;
use std::{
    hash::{Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};
pub struct Cover {
    pub key: ResourceKey,
    pub url: String,
    pub domains: Vec<String>,
    pub generation: u64,
}
pub fn worker(
    root: PathBuf,
    generation: Arc<AtomicU64>,
) -> (
    mpsc::SyncSender<Cover>,
    mpsc::Receiver<(ResourceKey, String, u64)>,
) {
    let (tx, rx) = mpsc::sync_channel::<Cover>(128);
    let (out, results) = mpsc::channel();
    std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(6))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let _ = std::fs::create_dir_all(&root);
        for job in rx {
            if generation.load(Ordering::Relaxed) != job.generation
                || !music_plugin_host::media_allowed(&job.url, &job.domains)
            {
                continue;
            }
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            job.url.hash(&mut hash);
            let path = root.join(format!("{:016x}.png", hash.finish()));
            if !valid_cache(&path) {
                let result = (|| -> anyhow::Result<()> {
                    let r = client.get(&job.url).send()?.error_for_status()?;
                    let mut bytes = Vec::new();
                    r.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                    anyhow::ensure!(bytes.len() <= 8 * 1024 * 1024, "cover too large");
                    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                        .with_guessed_format()?;
                    let mut limits = image::Limits::default();
                    limits.max_image_width = Some(4096);
                    limits.max_image_height = Some(4096);
                    limits.max_alloc = Some(64 * 1024 * 1024);
                    reader.limits(limits);
                    save_cache(&path, &reader.decode()?.thumbnail(320, 320))?;
                    Ok(())
                })();
                if result.is_err() {
                    continue;
                }
            }
            let _ = out.send((job.key, path.to_string_lossy().into_owned(), job.generation));
        }
    });
    (tx, results)
}

fn valid_cache(path: &Path) -> bool {
    (|| -> anyhow::Result<()> {
        let mut reader = image::ImageReader::open(path)?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(320);
        limits.max_image_height = Some(320);
        limits.max_alloc = Some(2 * 1024 * 1024);
        reader.limits(limits);
        reader.decode()?;
        Ok(())
    })()
    .is_ok()
}

fn save_cache(path: &Path, cover: &image::DynamicImage) -> anyhow::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    cover.write_to(file.as_file_mut(), image::ImageFormat::Png)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncated_cover_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cover.png");
        let cover = image::DynamicImage::new_rgb8(32, 32);
        save_cache(&path, &cover).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(!valid_cache(&path));
        save_cache(&path, &cover).unwrap();
        assert!(valid_cache(&path));
    }
}
