use crate::error::ServerError;
use crate::utilities::{assure_subdir, get_file_size};
use std::fs::remove_file;
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs::{File, create_dir_all};

pub(crate) struct Data {
    root_path: PathBuf,
    saves_path: PathBuf,
    files_path: PathBuf,
}

impl Data {
    pub(crate) fn new(root_path: impl AsRef<Path>) -> Result<Self, ServerError> {
        let root_path = root_path.as_ref();

        let saves_path = root_path.join("saves");
        let files_path = root_path.join("files");

        // assure that the directories exist
        assure_subdir(&root_path)?;
        assure_subdir(&saves_path)?;
        assure_subdir(&files_path)?;

        Ok(Self {
            root_path: root_path.into(),
            saves_path,
            files_path,
        })
    }

    pub(crate) fn get_saves_folder(&self, name: &String) -> Result<PathBuf, ServerError> {
        let path = self.saves_path.join(name);
        if !path.exists() {
            return Err(ServerError::NotAllowed("Save folder not found".into()));
        }
        Ok(path)
    }

    fn file_add_number(file: impl AsRef<Path>, num: u8) -> PathBuf {
        let mut file = file.as_ref().as_os_str().to_os_string();
        file.push(format!(".{}", num));
        file.into()
    }

    fn rotate_file(file: PathBuf, num: u8, end: u8) -> io::Result<()> {
        let current_file = Self::file_add_number(&file, num);

        if current_file.exists() {
            // delete file on end
            if num == end {
                remove_file(current_file)?;
            }
            // rotate next file and rename this
            else {
                Self::rotate_file(file.clone(), num + 1, end)?;
                let new_file = Self::file_add_number(file, num + 1);
                std::fs::rename(current_file, new_file)?;
            }
        }

        Ok(())
    }

    pub(crate) async fn get_and_rotate_file(
        &self,
        instance_name: impl AsRef<str>,
        file_name: impl AsRef<str>,
        amount: u8,
    ) -> io::Result<PathBuf> {
        let instance_path = self.files_path.join(instance_name.as_ref());

        create_dir_all(&instance_path).await?;

        let file_path = instance_path.join(file_name.as_ref());
        if file_path.exists() {
            // check if file is empty
            if get_file_size(File::open(&file_path).await?.metadata().await?) != 0 {
                Self::rotate_file(file_path.clone(), 0, amount)?;
                let new_file = Self::file_add_number(&file_path, 0);
                std::fs::rename(&file_path, new_file)?;
            }
        }
        Ok(file_path)
    }

    pub(crate) async fn get_file(
        &self,
        instance_name: impl AsRef<str>,
        file_name: impl AsRef<str>,
    ) -> io::Result<PathBuf> {
        let instance_path = self.files_path.join(instance_name.as_ref());
        create_dir_all(&instance_path).await?;
        Ok(instance_path.join(file_name.as_ref()))
    }
}
