use crate::cache::Cache;
use crate::data::Data;
use crate::error::ServerError;
use crate::factorio_version::FactorioVersion;
use crate::instance::{Instance, InstanceSettings};
use crate::utilities::{symlink_file, symlink_folder};
use std::path::{Path, PathBuf};
use serde::Serialize;
use tokio::fs::{create_dir_all, remove_dir_all, rename, File};
use tokio::io::{AsyncWriteExt, BufWriter};

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

    pub async fn start_instance(
        &self,
        name: String,
        settings: InstanceSettings,
    ) -> Result<Instance, ServerError> {
        // check if instance is already there

        // prepare instance
        let path = self.prepare_instance(&name, &settings).await?;

        Instance::start(settings, path, name, self).await
    }

    async fn prepare_instance(
        &self,
        name: &String,
        settings: &InstanceSettings,
    ) -> Result<PathBuf, ServerError> {
        let factorio_path = self.instances_path.join(name);

        // first thing: cleanup the folder we want to run in
        remove_dir_all(&factorio_path).await?;
        
        let executable_path = factorio_path.join(&settings.executable_path);
        let executable_parent = executable_path.parent().ok_or(ServerError::NotAllowed(
            "Configured executable path has no parent".to_string(),
        ))?;
        create_dir_all(&executable_parent).await?;

        let version = self.cache.get_version(&settings.factorio_version).await?;

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

        let mods_dir = factorio_path.join("mods");
        create_dir_all(&mods_dir).await?;

        for (name, version) in &settings.mods {
            let mod_path_src = self.cache.get_mod(name, version).await?;
            let file_name = mod_path_src
                .file_name()
                .ok_or(ServerError::NotAllowed("mod has no name".to_string()))?;
            let mod_path_dst = mods_dir.join(file_name);
            symlink_file(mod_path_src, mod_path_dst)?;
        }

        #[derive(Serialize)]
        struct ModListMod {
            name: String,
            enabled: bool,
        }
        #[derive(Serialize)]
        struct ModList {
            mods: Vec<ModListMod>,
        }
        let mut mod_list = ModList { mods: vec![] };
        mod_list.mods.push(ModListMod {
            name: "base".to_string(),
            enabled: true,
        });
        if settings.factorio_version >= FactorioVersion::from([2, 0, 0]) {
            mod_list.mods.push(ModListMod {
                name: "elevated-rails".to_string(),
                enabled: settings.base_mods.elevated_rails,
            });
            mod_list.mods.push(ModListMod {
                name: "quality".to_string(),
                enabled: settings.base_mods.quality,
            });
            mod_list.mods.push(ModListMod {
                name: "space-age".to_string(),
                enabled: settings.base_mods.quality,
            });
        }
        for (name, version) in &settings.mods {
            mod_list.mods.push(ModListMod {
                name: name.to_string(),
                enabled: true,
            })
        }
        let json = serde_json::to_string(&mod_list)?;
        let mut mod_list_json_file = File::create(mods_dir.join("mod-list.json")).await?;
        mod_list_json_file.write_all(json.as_bytes()).await?;
        mod_list_json_file.flush().await?;

        // TODO: link in mod-settings.dat (if it exists, if not create one, or let factorio create one, not sure)

        Ok(factorio_path)
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
}

#[cfg(test)]
mod test {
    use crate::factorio_version::FactorioVersion;
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
        let mut settings =
            InstanceSettings::new("test3".to_string(), FactorioVersion::from([1, 1, 110])).unwrap();
        settings.add_mod("AutoDeconstruct", "1.0.2");
        settings.add_mod("RateCalculator", "3.3.0");
        let instance = manager.unwrap();
        let mut instance = instance
            .start_instance("test_1.1.110".to_string(), settings)
            .await
            .unwrap();

        sleep(Duration::from_secs(5)).await;

        instance.stop().await.unwrap();
    }
}
