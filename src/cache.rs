use crate::credentials::CredentialManager;
use crate::error::ServerError;
use crate::mod_portal::ModPortal;
use crate::version::Version;
use crate::Progress;
use dashmap::{DashMap, Entry};
use futures_lite::StreamExt;
use rc_zip_tokio::ReadZip;
use reqwest::Client;
use scraper::Selector;
use std::collections::HashMap;
use std::fs::remove_dir_all;
use std::path::{Path, PathBuf};
use tokio::fs::{create_dir_all, File};
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tokio::sync::broadcast::{Receiver, Sender};
use tokio_util::either::Either;
use tokio_util::io::InspectReader;

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
    pub async fn get_factorio(
        &self,
        version: &Version,
        progress: &mut Progress,
    ) -> Result<PathBuf, ServerError> {
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
                self.download_factorio(version, &path, progress)
                    .await
                    .map_err(|err| {
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
        progress: &mut Progress,
    ) -> Result<(), ServerError> {
        use rc_zip_tokio::rc_zip::parse::Mode;
        use tokio::fs::OpenOptions;

        let mut download_progress = progress.allocate_fraction(2);

        ///////////////////////
        // Download Factorio //
        ///////////////////////

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

        let download_size = resp.content_length();
        let mut buffer = if let Some(size) = download_size {
            download_progress.set_internal(size);
            Vec::with_capacity(size as usize)
        } else {
            download_progress.set_internal(1);
            Vec::new()
        };

        let mut stream = resp.bytes_stream();

        // alternative stream only
        // let mut stream = stream.map_err(std::io::Error::other);
        // let stream = stream.inspect(|data| {
        //     if let Ok(data) = data {
        //         if download_size.is_some() {
        //             download_progress.advance(data.len() as u64);
        //         }
        //     }
        // });
        // let mut stream = StreamReader::new(stream);
        //
        // tokio::io::copy(&mut stream, &mut buffer).await?;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buffer.extend_from_slice(&chunk);

            if download_size.is_some() {
                download_progress.advance(chunk.len() as u64);
            }
        }

        if download_size.is_none() {
            download_progress.advance(1);
        }

        /////////////////
        // extract zip //
        /////////////////
        let extract_progress: Progress = progress.allocate_fraction(2);

        let reader = buffer.read_zip().await?;

        let entries_count = reader.entries().count() as u64;
        for entry in reader.entries() {
            let filename = entry.sanitized_name().ok_or_else(|| {
                ServerError::DownloadError("invalid filename in factorio zip-file".into())
            })?;
            let out_path = path.as_ref().join(filename);

            let mut entry_progress = extract_progress.allocate_fraction(entries_count);

            if entry.mode.has(Mode::DIR) {
                // The directory may have been created if iteration is out of order.
                if !out_path.exists() {
                    create_dir_all(&out_path).await?;
                }
                entry_progress.set_internal(1);
                entry_progress.advance(1);
            } else {
                // Creates parent directories. They may not exist if iteration is out of order
                // or the archive does not contain directory entries.
                let parent = out_path.parent().ok_or(ServerError::DownloadError(
                    "This file has no parent".to_string(),
                ))?;
                if !parent.is_dir() {
                    create_dir_all(parent).await?;
                }
                let mut writer = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&out_path)
                    .await?;

                entry_progress.set_internal(entry.uncompressed_size);

                let entry_reader = entry.reader();

                let mut entry_reader = InspectReader::new(entry_reader, |data| {
                    entry_progress.advance(data.len() as u64)
                });

                tokio::io::copy(&mut entry_reader, &mut writer).await?;
            }
        }

        Ok(())
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    async fn download_factorio(
        &self,
        version: &Version,
        path: impl AsRef<Path>,
        progress: &mut Progress,
    ) -> Result<(), ServerError> {
        use async_compression::tokio::bufread::XzDecoder;
        use futures::TryStreamExt;
        use tokio::io::BufReader;
        use tokio_tar::Archive;
        use tokio_util::io::StreamReader;

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

        let size = resp.content_length();
        if let Some(size) = size {
            progress.set_internal(size);
        } else {
            progress.set_internal(1);
        }

        let stream = resp.bytes_stream();
        let stream = stream.inspect(|e| {
            if size.is_some() {
                let len = if let Ok(e) = e { e.len() as u64 } else { 0 };
                progress.advance(len);
            }
        });
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

        if size.is_none() {
            progress.advance(1);
        }

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
        progress: &mut Progress,
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
                self.download_mod(&path, &release.download_url, progress)
                    .await
                    .map_err(|err| {
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
        progress: &mut Progress,
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

        let size = res.content_length();

        if let Some(size) = size {
            progress.set_internal(size);
        } else {
            progress.set_internal(1);
        }

        let mut file = File::create(path.as_ref()).await?;
        let mut content = res.bytes_stream();
        while let Some(chunk) = content.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            if size.is_some() {
                progress.advance(chunk.len() as u64);
            }
        }

        if size.is_none() {
            progress.advance(1);
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

        cache.credentials.login(dotenvy::var("factorio_username").unwrap(), dotenvy::var("factorio_password").unwrap()).await.unwrap();
        cache.credentials.save().unwrap();

        let mut progress = Progress::new(10000);

        cache
            .get_factorio(&Version::from([1, 1, 110]), &mut progress)
            .await
            .unwrap();

        // let versions = cache.get_available_versions().await.unwrap();
        // println!("{:?}", versions);

        let mut progress = Progress::new(10000);

        cache
            .get_mod("Bottleneck", &Version::from([0, 11, 7]), &mut progress)
            .await
            .unwrap();

        // panic!("something, so log is shown");
    }
}
