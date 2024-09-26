use crate::cache::Cache;
use crate::data::Data;
use crate::error::ServerError;
use crate::instance::{Instance, InstanceSettings};
use crate::utilities::{symlink_file, symlink_folder};
use std::path::PathBuf;
use tokio::fs::{create_dir_all, remove_dir_all, rename};

pub struct Manager {
    root_path: PathBuf,
    cache: Cache,
    data: Data,
    instances_path: PathBuf,
}

impl Manager {
    pub fn new(root_path: impl Into<PathBuf>) -> Self {
        let root_path = root_path.into();

        Self {
            root_path: root_path.clone(),
            cache: Cache::new(root_path.join("cache")),
            data: Data::new(root_path.join("data")),
            instances_path: root_path.join("instances"),
        }
    }

    pub async fn start_instance(
        &self,
        name: String,
        settings: InstanceSettings,
    ) -> Result<Instance, ServerError> {
        // check if instance is already there

        // prepare instance
        let path = self.prepare_instance(name, &settings).await?;

        Instance::start(settings, path).await
    }

    async fn prepare_instance(
        &self,
        name: String,
        settings: &InstanceSettings,
    ) -> Result<PathBuf, ServerError> {
        let factorio_path = self.instances_path.join(&name);

        let executable_path = factorio_path.join(&settings.executable_path);
        let executable_parent = executable_path.parent().ok_or(ServerError::NotAllowed(
            "Configured executable path has no parent".to_string(),
        ))?;
        create_dir_all(&executable_parent).await?;

        let version = self.cache.get_version(&settings.factorio_version).await;

        symlink_file(
            version.join(InstanceSettings::default_executable_path()),
            executable_path,
        )?;
        symlink_file(
            version.join("config-path.cfg"),
            factorio_path.join("config-path.cfg"),
        )?;
        symlink_folder(version.join("data"), factorio_path.join("data"))?;

        let save_folder_path = self.data.get_saves_folder(&settings.save)?;
        symlink_folder(save_folder_path, factorio_path.join("saves"))?;

        Ok(factorio_path)
    }

    async fn cleanup_instance(&self, name: String) -> Result<(), ServerError> {
        let factorio_path = self.instances_path.join(&name);

        let log_path = self
            .data
            .get_and_rotate_file(name.clone(), "factorio-current.log".into(), 9)
            .await?;
        rename(factorio_path.join("factorio-current.log"), log_path).await?;

        let console_log = self
            .data
            .get_and_rotate_file(name, "console.log".to_string(), 9)
            .await?;
        rename(factorio_path.join("console.log"), console_log).await?;

        remove_dir_all(factorio_path).await?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::instance::InstanceSettings;
    use crate::manager::Manager;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test() {
        #[cfg(target_os = "linux")]
        let manager = Manager::new("/mnt/c/Data/Development/tmp/factorio-server-root");
        #[cfg(target_os = "windows")]
        let manager = Manager::new("C:\\Data\\Development\\tmp\\factorio-server-root-windows");
        let settings = InstanceSettings::new("test3".to_string(), "1.1.110".to_string()).unwrap();
        let mut instance = manager
            .start_instance("test_1.1.110".to_string(), settings)
            .await
            .unwrap();

        sleep(Duration::from_secs(5)).await;

        instance.stop().await.unwrap();
        manager
            .cleanup_instance("test_1.1.110".to_string())
            .await
            .unwrap();
    }
}
