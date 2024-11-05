use crate::cache::Cache;
use crate::data::Data;
use crate::error::ServerError;
use crate::instance::{Instance, InstanceSettings};
use std::path::{Path, PathBuf};
use tokio::fs::rename;
use crate::version::Version;

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

    pub async fn prepare_instance(
        &self,
        name: String,
        settings: InstanceSettings,
    ) -> Result<Instance, ServerError> {
        // check if instance is already there

        // prepare instance
        let instance_path = self.instances_path.join(&name);

        let factorio_cache_path = self.cache.get_version(&settings.factorio_version).await?;
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

    pub(crate) async fn backup_logs(
        &self,
        factorio_path: impl AsRef<Path>,
        name: String,
    ) -> Result<(), ServerError> {
        let log_path = self
            .data
            .get_and_rotate_file(name.clone(), "factorio-current.log".into(), 9)
            .await?;
        rename(
            factorio_path.as_ref().join("factorio-current.log"),
            log_path,
        )
        .await?;

        let console_log = self
            .data
            .get_and_rotate_file(name, "console.log".to_string(), 9)
            .await?;
        rename(factorio_path.as_ref().join("console.log"), console_log).await?;

        Ok(())
    }
    
    pub(crate) async fn get_mod(&self, name: impl AsRef<str>, version: &Version) -> Result<PathBuf, ServerError> {
        self.cache.get_mod(name, version).await
    }
}

#[cfg(test)]
mod test {
    use crate::version::Version;
    use crate::instance::InstanceSettings;
    use crate::manager::Manager;
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
            InstanceSettings::new("test3".to_string(), Version::from([1, 1, 110])).unwrap();
        settings.add_mod("AutoDeconstruct", Version::from([1, 0, 2]));
        settings.add_mod("RateCalculator", Version::from([3, 3, 0]));
        let instance = manager
            .prepare_instance("test_1.1.110".to_string(), settings)
            .await
            .unwrap();
        let mut instance = instance.start().await.unwrap();

        sleep(Duration::from_secs(5)).await;

        instance.stop().await.unwrap();
    }
}
