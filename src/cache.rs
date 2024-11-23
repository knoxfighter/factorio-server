use crate::credentials::CredentialManager;
use crate::error::ServerError;
use crate::mod_portal::ModPortal;
use crate::version::Version;
use dashmap::{DashMap, Entry};
use futures::{StreamExt, TryStreamExt};
use reqwest::Client;
use scraper::Selector;
use std::collections::HashMap;
use std::fs::remove_dir_all;
use std::path::{Path, PathBuf};
use tokio::fs::{create_dir_all, File};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::broadcast::{Receiver, Sender};
use tokio::sync::broadcast;
use tokio_util::either::Either;
use tokio_util::io::StreamReader;

type InFlight = DashMap<PathBuf, Sender<()>>;

pub struct Cache {
    root_path: PathBuf,
    factorio_dir: PathBuf,
    mods_dir: PathBuf,
    credentials: CredentialManager,
    mod_portal: ModPortal,
    client: Client,
    in_flight: InFlight,
}

struct SenderGuard<'a> {
    path: PathBuf,
    in_flight: &'a InFlight,
    sender: Sender<()>,
}

impl Drop for SenderGuard<'_> {
    fn drop(&mut self) {
        self.in_flight.remove(&self.path);
    }
}

impl Cache {
    pub(crate) fn new(root_path: PathBuf) -> Result<Self, ServerError> {
        Ok(Self {
            factorio_dir: root_path.join("factorio"),
            mods_dir: root_path.join("mods"),
            credentials: CredentialManager::load(root_path.join("credentials.json"))?,
            root_path,
            mod_portal: ModPortal::new()?,
            client: Client::new(),
            in_flight: DashMap::new(),
        })
    }

    /// Download factorio from the official website.
    /// This function is save to be called multiple times, all futures will be fulfilled when the download is done.
    /// 
    /// On Windows it needs a valid login to download factorio.
    /// Make sure to login first with the `crate::credentials::CredentialManager`.
    /// 
    /// # Arguments 
    /// 
    /// * `version`: The factorio version to download
    /// 
    /// returns: Result<PathBuf, ServerError> 
    /// 
    /// # Examples 
    /// 
    /// ```
    /// 
    /// ```
    pub async fn get_factorio(&self, version: &Version) -> Result<PathBuf, ServerError> {
        let path = self.factorio_dir.join(version.to_string());
        if path.exists() {
            return Ok(path);
        }

        match self.check_inflight(path.clone()) {
            Either::Left(mut receiver) => {
                receiver.recv().await?;
                if path.exists() {
                    Ok(path)
                } else {
                    Err(ServerError::InFlightError)
                }
            }
            Either::Right(sender_guard) => {
                create_dir_all(&path).await?;

                // TODO: maybe do this as drop_guard within download_mod
                self.download_factorio(version, &path).await.map_err(|err| {
                    let remove_err = remove_dir_all(&path);
                    if remove_err.is_err() {
                        remove_err.err().unwrap().into()
                    } else {
                        err
                    }
                })?;

                sender_guard.sender.send(()).ok();

                Ok(path)
            }
        }
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    async fn download_factorio(
        &self,
        version: &Version,
        path: impl AsRef<Path>,
    ) -> Result<(), ServerError> {
        use async_zip::base::read::seek::ZipFileReader;
        use std::io::Cursor;
        use tokio::fs::OpenOptions;
        use tokio_util::compat::TokioAsyncWriteCompatExt;

        if !self.credentials.has_token() {
            return Err(ServerError::NotAllowed(
                "Please Login before downloading factorio".to_string(),
            ));
        }

        let credentials = self.credentials.get_credentials()?;
        let build = if version >= &Version::from([2, 0, 0]) {
            "expansion"
        } else {
            "alpha"
        };
        let distro = "win64-manual";
        let resp = self
            .client
            .get(format!(
                "https://www.factorio.com/get-download/{}/{build}/{distro}?username={}&token={}",
                version, credentials.username, credentials.token
            ))
            .send()
            .await?
            .error_for_status()?;
        let stream = resp.bytes_stream();
        let stream = StreamReader::new(
            stream.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)),
        );
        let mut stream = BufReader::new(stream);

        let mut buffer = Vec::new();

        tokio::io::copy(&mut stream, &mut buffer).await?;

        let cursor = Cursor::new(buffer);

        let mut reader = ZipFileReader::with_tokio(cursor).await?;

        for index in 0..reader.file().entries().len() {
            let entry = reader.file().entries().get(index).unwrap();

            let filepath: PathBuf = entry.filename().as_str()?.into();
            // remove first element from path in zip, it is `Factorio_<version>` and uninteresting
            let filepath: PathBuf = filepath.components().skip(1).collect();

            let path = path.as_ref().join(filepath);
            // If the filename of the entry ends with '/', it is treated as a directory.
            // This is implemented by previous versions of this crate and the Python Standard Library.
            // https://docs.rs/async_zip/0.0.8/src/async_zip/read/mod.rs.html#63-65
            // https://github.com/python/cpython/blob/820ef62833bd2d84a141adedd9a05998595d6b6d/Lib/zipfile.py#L528
            let entry_is_dir = entry.dir()?;

            let mut entry_reader = reader.reader_without_entry(index).await?;

            if entry_is_dir {
                // The directory may have been created if iteration is out of order.
                if !path.exists() {
                    create_dir_all(&path).await?;
                }
            } else {
                // Creates parent directories. They may not exist if iteration is out of order
                // or the archive does not contain directory entries.
                let parent = path.parent().ok_or(ServerError::DownloadError(
                    "This file has no parent".to_string(),
                ))?;
                if !parent.is_dir() {
                    create_dir_all(parent).await?;
                }
                let writer = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .await?;
                futures_lite::io::copy(&mut entry_reader, &mut writer.compat_write()).await?;
            }
        }

        Ok(())
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    async fn download_factorio(
        &self,
        version: &Version,
        path: impl AsRef<Path>,
    ) -> Result<(), ServerError> {
        use async_compression::tokio::bufread::XzDecoder;
        use tokio_tar::Archive;

        let build = "headless";
        let distro = "linux64";
        let resp = self
            .client
            .get(format!(
                "https://www.factorio.com/get-download/{}/{build}/{distro}",
                version
            ))
            .send()
            .await?
            .error_for_status()?;
        let stream = resp.bytes_stream();
        let stream = StreamReader::new(
            stream.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)),
        );
        let stream = BufReader::new(stream);

        let decoder = XzDecoder::new(stream);

        let mut archive = Archive::new(decoder);

        archive.unpack(&path).await?;

        let mut entries = tokio::fs::read_dir(&path).await?;
        let entry = entries
            .next_entry()
            .await?
            .ok_or(ServerError::DownloadError(
                "missing subfolder after extracting tar".to_string(),
            ))?;

        let mut entries = tokio::fs::read_dir(entry.path()).await?;
        while let Some(entry) = entries.next_entry().await? {
            let sub_path = entry.path();
            let file_name = match sub_path.file_name() {
                Some(name) => name.to_os_string(),
                None => continue,
            };

            // Define destination path in the parent directory
            let dest_path = path.as_ref().join(file_name);

            tokio::fs::rename(&sub_path, &dest_path).await?;
        }
        tokio::fs::remove_dir(&entry.path()).await?;

        Ok(())
    }

    // return (available, downloaded)
    pub async fn get_available_versions(
        &self,
    ) -> Result<HashMap<String, (bool, bool)>, ServerError> {
        let mut versions = HashMap::new();

        let mut dir_reader = tokio::fs::read_dir(&self.factorio_dir).await?;
        while let Some(file) = dir_reader.next_entry().await? {
            let name = file.file_name().to_str().unwrap().into();
            let value = versions.entry(name).or_insert((false, false));
            value.1 = true;
        }

        let downloadable = self
            .client
            .get("https://www.factorio.com/download/archive/")
            .send()
            .await?
            .text()
            .await?;
        let document = scraper::Html::parse_document(&downloadable);
        let selector = Selector::parse("a.slot-button-inline").unwrap();
        for elem in document.select(&selector) {
            let link = elem
                .attr("href")
                .ok_or(ServerError::DownloadError("no href present".to_string()))?;

            let version = link.split("/").last().ok_or(ServerError::DownloadError(
                "href link is wrongly formatted".to_string(),
            ))?;

            let value = versions
                .entry(version.to_string())
                .or_insert((false, false));
            value.0 = true;
        }

        Ok(versions)
    }

    pub async fn delete_version(&self, version: impl AsRef<str>) -> Result<(), ServerError> {
        let version = version.as_ref();

        let dir = self.factorio_dir.join(version);
        if !dir.exists() {
            return Err(ServerError::NotAllowed("version doesn't exist".to_string()));
        }
        tokio::fs::remove_dir_all(&dir).await?;

        Ok(())
    }

    fn check_inflight(&self, path: PathBuf) -> Either<Receiver<()>, SenderGuard> {
        let entry = self.in_flight.entry(path.clone());

        match entry {
            // TODO: If this causes issues, use a Weak<Mutex<Sender<()>>> instead
            Entry::Occupied(mut elem) => {
                let e = elem.get_mut();
                Either::Left(e.subscribe())
            }
            Entry::Vacant(elem) => {
                let sender = broadcast::channel(1).0;
                elem.insert(sender.clone());
                Either::Right(SenderGuard {
                    path,
                    in_flight: &self.in_flight,
                    sender,
                })
            }
        }
    }

    /// Download a mod from the official mod portal.
    /// This function is save to be called multiple times, all futures will be fulfilled when the download is done.
    /// 
    /// Make sure to login first with the `crate::credentials::CredentialManager`.
    /// 
    /// # Arguments 
    /// 
    /// * `name`: 
    /// * `version`: 
    /// 
    /// returns: Result<PathBuf, ServerError> 
    /// 
    /// # Examples 
    /// 
    /// ```
    /// 
    /// ```
    pub async fn get_mod(
        &self,
        name: impl AsRef<str>,
        version: &Version,
    ) -> Result<PathBuf, ServerError> {
        let path = self.mods_dir.join(name.as_ref()).join(version.to_string());
        let path = path.join(format!("{}_{}.zip", name.as_ref(), version));

        if path.exists() {
            return Ok(path);
        }

        match self.check_inflight(path.clone()) {
            Either::Left(mut receiver) => {
                receiver.recv().await?;
                if path.exists() {
                    Ok(path)
                } else {
                    Err(ServerError::InFlightError)
                }
            }
            Either::Right(sender_guard) => {
                if !self.credentials.has_token() {
                    return Err(ServerError::NotAllowed("credentials required".to_string()));
                }

                let result = self.mod_portal.mod_short(name.as_ref()).await?;
                let release = result
                    .result
                    .releases
                    .ok_or(ServerError::DownloadError("no releases found".to_string()))?;
                let version_str = version.to_string();
                let release = release
                    .iter()
                    .find(|release| release.version == version_str)
                    .ok_or(ServerError::DownloadError("release not found".to_string()))?;

                // TODO: maybe do this as drop_guard within download_mod
                self.download_mod(&path, &release.download_url).await.map_err(|err| {
                    let remove_err = remove_dir_all(&path);
                    if remove_err.is_err() {
                        remove_err.err().unwrap().into()
                    } else {
                        err
                    }
                })?;

                sender_guard.sender.send(()).ok();

                Ok(path)
            }
        }
    }

    async fn download_mod(
        &self,
        path: impl AsRef<Path>,
        url: impl AsRef<str>,
    ) -> Result<(), ServerError> {
        tokio::fs::create_dir_all(path.as_ref().parent().ok_or(ServerError::NotAllowed(
            "mod_file_path has no parent".to_string(),
        ))?)
        .await?;

        let creds = self.credentials.get_credentials()?;
        let url = format!(
            "https://mods.factorio.com/{}?username={}&token={}",
            url.as_ref(),
            creds.username,
            creds.token
        );

        let res = self.client.get(url).send().await?.error_for_status()?;

        let mut file = File::create(path.as_ref()).await?;
        let mut content = res.bytes_stream();
        while let Some(chunk) = content.next().await {
            file.write_all(&chunk?).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test() {
        let mut cache = Cache::new(PathBuf::from("/tmp")).unwrap();
        // let mut cache = Cache::new(PathBuf::from("C:\\Data\\tmp\\factorio")).unwrap();
        // cache.credentials.login("asdff45", "<pw>").await.unwrap();
        // cache.credentials.save().unwrap();
        cache
            .get_factorio(&Version::from([1, 1, 110]))
            .await
            .unwrap();

        // let versions = cache.get_available_versions().await.unwrap();
        // println!("{:?}", versions);

        cache
            .get_mod("Bottleneck", &Version::from([0, 11, 17]))
            .await
            .unwrap();

        panic!("something, so log is shown");
    }
}
