use crate::cache::Cache;
use crate::data::Data;
use crate::error::ServerError;
use crate::instance::{Instance, InstanceSettings};
use crate::version::Version;
use std::path::{Path, PathBuf};
use tokio::fs::rename;

pub struct Manager {
    root_path: PathBuf,
    cache: Cache,
    data: Data,
    instances_path: PathBuf,
}

impl Manager {
    pub fn new(root_path: impl Into<PathBuf>) -> Result<Self, ServerError> {
        let root_path = root_path.into();

        Ok(Self {
            root_path: root_path.clone(),
            cache: Cache::new(root_path.join("cache"))?,
            data: Data::new(root_path.join("data")),
            instances_path: root_path.join("instances"),
        })
    }

    /// prepare a new instance, will download and await factorio and all needed mods.
    pub async fn prepare_instance(
        &self,
        name: String,
        settings: InstanceSettings,
    ) -> Result<Instance, ServerError> {
        // TODO: check if instance is already there

        // prepare instance
        let instance_path = self.instances_path.join(&name);

        let factorio_cache_path = self.cache.get_factorio(&settings.factorio_version).await?;
        let saves_path = self.data.get_saves_folder(&settings.save)?;

        Instance::prepare(
            self,
            &name,
            settings,
            &instance_path,
            &factorio_cache_path,
            &saves_path,
        )
        .await
    }

    pub(crate) async fn backup_files(
        &self,
        instance_name: impl AsRef<str>,
        paths: Vec<impl AsRef<Path>>,
    ) -> Result<(), ServerError> {
        for path in paths {
            let path = path.as_ref();
            if path.exists() {
                if let Some(filename) = path.file_name() {
                    let rotated_log = self.data.get_and_rotate_file(instance_name.as_ref(), filename.to_str().unwrap(), 9).await?;
                    rename(path, &rotated_log).await?;
                }
            }
        }

        Ok(())
    }
    
    pub(crate) async fn load_backup_file(&self, instance_name: impl AsRef<str>, name: impl AsRef<str>) -> Result<PathBuf, ServerError> {
        Ok(self.data.get_file(instance_name.as_ref(), name.as_ref()).await?)
    }

    pub async fn get_mod(&self, name: impl AsRef<str>, version: &Version) -> Result<PathBuf, ServerError> {
        self.cache.get_mod(name, version).await
    }

    pub async fn get_factorio(&self, version: &Version) -> Result<PathBuf, ServerError> {
        self.cache.get_factorio(version).await
    }
}

#[cfg(test)]
mod test {
    use crate::instance::InstanceSettings;
    use crate::manager::Manager;
    use crate::version::Version;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test() {
        #[cfg(target_os = "linux")]
        let manager = Manager::new("/mnt/c/Data/Development/tmp/factorio-server-root");
        #[cfg(target_os = "windows")]
        let manager =
            Manager::new("C:\\Data\\Development\\tmp\\factorio-server-root-windows").unwrap();
        let mut settings =
            InstanceSettings::new("test4".to_string(), Version::from([1, 1, 110])).unwrap();
        settings.add_mod("AutoDeconstruct", Version::from([0, 4, 4]));
        settings.add_mod("RateCalculator", Version::from([3, 2, 7])); // doesn't load, needs flib
        let instance = manager
            .prepare_instance("test_1.1.110".to_string(), settings)
            .await
            .unwrap();
        let mut instance = instance.start().await.unwrap();

        sleep(Duration::from_secs(120)).await;

        instance.stop().await.unwrap();
    }
}
