use crate::error::ServerError;
use crate::factorio_tracker::FactorioTracker;
use crate::manager::Manager;
use crate::utilities::{get_random_port, symlink_file, symlink_folder};
use crate::version::Version;
use crate::Progress;
use rand::distr::Alphanumeric;
use rand::Rng;
use rcon::Connection;
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use sysinfo::{Pid, System};
use tokio::fs::{create_dir_all, remove_dir_all, File};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::broadcast::channel;
use tokio::sync::watch::Sender;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const PID_FILE_NAME: &str = "factorio.pid";

#[derive(PartialEq, Default, Debug)]
pub enum Status {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Closed, // Set between factorio output "changing state from(Disconnected) to(Closed)" and process end.
}

pub struct Instance<'a> {
    settings: InstanceSettings,

    path: PathBuf,
    name: String,

    manager: &'a Manager,
}

pub struct RunningInstance<'a> {
    settings: InstanceSettings,

    path: PathBuf,
    name: String,

    manager: &'a Manager,

    process: Child,
    status: Sender<Status>,
    tracker: FactorioTracker,
    tracker_resv: JoinHandle<Result<(), ServerError>>,
}

pub struct BaseMods {
    pub base: bool, // always has to be enabled
    pub elevated_rails: bool,
    pub quality: bool,
    pub space_age: bool,
}
impl Default for BaseMods {
    fn default() -> Self {
        Self {
            base: true,
            elevated_rails: true,
            quality: true,
            space_age: true,
        }
    }
}

pub struct Mod {
    name: String,
    version: Version,
}

pub struct InstanceSettings {
    pub executable_path: PathBuf,
    pub saves_path: PathBuf,

    pub factorio_version: Version,
    pub save: String, // Insert a save out of the `data` dir

    pub host: IpAddr,
    pub port: u16,

    pub rcon_host: IpAddr,
    pub rcon_port: u16,
    pub rcon_pass: String,

    pub mods: Vec<Mod>,
    pub base_mods: BaseMods,
}

impl InstanceSettings {
    // This also sets the default values
    pub fn new(save: String, factorio_version: Version) -> Result<Self, ServerError> {
        let default_addr = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));

        Ok(Self {
            executable_path: Self::default_executable_path(),
            saves_path: "saves".into(),
            factorio_version,
            save,
            host: default_addr,
            port: 34197u16,
            rcon_host: default_addr,
            rcon_port: 0u16,
            rcon_pass: rand::rng()
                .sample_iter(&Alphanumeric)
                .take(16)
                .map(char::from)
                .collect(),
            mods: vec![],
            base_mods: BaseMods::default(),
        })
    }

    pub(crate) fn default_executable_path() -> PathBuf {
        #[cfg(target_os = "windows")]
        return "bin/x64/factorio.exe".into();
        #[cfg(not(target_os = "windows"))]
        return "bin/x64/factorio".into();
    }

    // If this is set, there have to be some changed to the "config-path.cfg", i am not even sure if that is supported at all.
    // pub fn executable_path(&mut self, executable_path: impl AsRef<Path>) -> &mut Self {
    //     self.executable_path = executable_path.as_ref().to_path_buf();
    //     self
    // }

    pub fn saves_path(&mut self, saves_path: impl AsRef<Path>) -> &mut Self {
        self.saves_path = saves_path.as_ref().to_path_buf();
        self
    }

    pub fn factorio_version(&mut self, factorio_version: Version) -> &mut Self {
        self.factorio_version = factorio_version;
        self
    }

    pub fn save(&mut self, save: &str) -> &mut Self {
        self.save = save.to_string();
        self
    }

    pub fn host(&mut self, host: IpAddr) -> &mut Self {
        self.host = host;
        self
    }

    pub fn port(&mut self, port: u16) -> &mut Self {
        self.port = port;
        self
    }

    pub fn rcon_host(&mut self, host: IpAddr) -> &mut Self {
        self.rcon_host = host;
        self
    }

    pub fn rcon_port(&mut self, port: u16) -> &mut Self {
        self.rcon_port = port;
        self
    }

    pub fn rcon_pass(&mut self, pass: String) -> &mut Self {
        self.rcon_pass = pass;
        self
    }

    pub fn mods(&mut self, mods: Vec<Mod>) -> &mut Self {
        self.mods = mods;
        self
    }

    pub fn add_mod(&mut self, name: impl AsRef<str>, version: Version) -> &mut Self {
        self.mods.push(Mod {
            name: name.as_ref().to_string(),
            version,
        });
        self
    }

    pub fn with_space_age(&mut self) -> &mut Self {
        self.base_mods.space_age = true;
        self.base_mods.quality = true;
        self.base_mods.elevated_rails = true;
        self
    }

    pub fn base_mods(&mut self, base_mods: BaseMods) -> &mut Self {
        self.base_mods = base_mods;
        self
    }
}

impl<'a> Instance<'a> {
    pub(crate) async fn prepare(
        manager: &'a Manager,
        name: impl AsRef<str>,
        settings: InstanceSettings,
        instance_path: impl AsRef<Path>,
        factorio_cache_path: impl AsRef<Path>,
        saves_path: impl AsRef<Path>,
        prog: &mut Progress,
    ) -> Result<Self, ServerError> {
        let instance_path = instance_path.as_ref();
        let factorio_cache_path = factorio_cache_path.as_ref();

        // first thing: cleanup the folder we want to run in.
        // It could still exist from previous runs
        if instance_path.exists() {
            remove_dir_all(&instance_path).await?;
        }

        let executable_path = instance_path.join(&settings.executable_path);
        let executable_parent = executable_path.parent().ok_or(ServerError::NotAllowed(
            "Configured executable path has no parent".to_string(),
        ))?;
        create_dir_all(&executable_parent).await?;

        symlink_file(
            factorio_cache_path.join(InstanceSettings::default_executable_path()),
            executable_path,
        )?;
        symlink_file(
            factorio_cache_path.join("config-path.cfg"),
            instance_path.join("config-path.cfg"),
        )?;
        symlink_folder(factorio_cache_path.join("data"), instance_path.join("data"))?;

        symlink_folder(saves_path, instance_path.join("saves"))?;

        let mods_dir = instance_path.join("mods");
        create_dir_all(&mods_dir).await?;

        for mod_ in &settings.mods {
            let mut sub_prog = prog.allocate_fraction(settings.mods.len() as u64);

            let mod_path_src = manager
                .get_mod(&mod_.name, &mod_.version, &mut sub_prog)
                .await?;

            let file_name = mod_path_src
                .file_name()
                .ok_or(ServerError::NotAllowed("mod has no name".to_string()))?;

            let mod_path_dst = mods_dir.join(file_name);
            symlink_file(mod_path_src, mod_path_dst)?;
            // tokio::fs::copy(mod_path_src, mod_path_dst).await?;
        }

        build_mod_list_json(&settings, mods_dir.join("mod-list.json")).await?;

        // copy in mod settings
        let mod_settings_dat = manager
            .load_backup_file(name.as_ref(), "mod-settings.dat")
            .await?;
        if mod_settings_dat.exists() {
            tokio::fs::copy(mod_settings_dat, mods_dir.join("mod-settings.dat")).await?;
        } else {
            let settings_json = manager
                .load_backup_file(name.as_ref(), "mod-settings.json")
                .await?;
            if settings_json.exists() {
                tokio::fs::copy(settings_json, mods_dir.join("mod-settings.json")).await?;
            }
        }

        Ok(Self {
            settings,
            path: instance_path.into(),
            name: name.as_ref().to_string(),
            manager,
        })
    }

    pub(crate) async fn check_running(instance_path: impl AsRef<Path>) -> Result<(), ServerError> {
        let instance_path = instance_path.as_ref();

        if !instance_path.exists() {
            return Err(ServerError::AlreadyRunningError);
        }

        let pid_file = instance_path.join(PID_FILE_NAME);
        if !pid_file.exists() {
            return Err(ServerError::AlreadyRunningError);
        }

        let pid = tokio::fs::read_to_string(&pid_file).await?;
        let pid = pid.parse::<Pid>()?;
        let system = System::new_all();
        let process = system.process(pid);
        if process.is_some() {
            return Err(ServerError::AlreadyRunningError);
        }
        
        Ok(())
    }

    pub async fn start(mut self) -> Result<RunningInstance<'a>, ServerError> {
        let exec_path = self.path.join(&self.settings.executable_path);

        let save_path = self
            .path
            .join(&self.settings.saves_path)
            .join(&self.settings.save)
            .with_extension("zip");

        let (sender, mut recv) = channel::<String>(32);

        let tracker = FactorioTracker::watch(
            self.path.join("factorio-current.log"),
            self.path.join(PID_FILE_NAME),
            sender,
        );

        self.settings.rcon_port = if self.settings.rcon_port != 0 {
            self.settings.rcon_port
        } else {
            get_random_port(self.settings.rcon_host).await?
        };

        let mut command = Command::new(exec_path);
        command
            .current_dir(&self.path)
            .args([
                "--executable-path",
                self.settings.executable_path.to_str().unwrap(),
                "--start-server",
                save_path.to_str().ok_or(ServerError::Utf8Error())?,
                "--console-log",
                "console.log",
                "--no-log-rotation",
                "--bind",
                self.settings.host.to_string().as_str(),
                "--port",
                self.settings.port.to_string().as_str(),
                "--rcon-bind",
                format!("{}:{}", self.settings.rcon_host, self.settings.rcon_port).as_str(),
                "--rcon-password",
                self.settings.rcon_pass.as_str(),
                "--mod-directory",
                self.path.join("mods").to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .kill_on_drop(true);

        let process = command.spawn()?;

        // save pid
        let pid = process
            .id()
            .ok_or(ServerError::NotAllowed("Process has no pid".into()))?;
        let pid_path = self.path.join(PID_FILE_NAME);
        let mut pid_file = File::create(pid_path).await?;
        pid_file.write_all(pid.to_string().as_bytes()).await?;

        let (status_sender, _) = tokio::sync::watch::channel(Default::default());

        let status_sender2 = status_sender.clone();

        let tracker_resv = tokio::spawn(async move {
            loop {
                let line = recv.recv().await?;

                println!("{}", line);

                if line == "factorio process stopped" {
                    status_sender.send_replace(Status::Stopped);
                    break;
                }

                if line.ends_with("changing state from(CreatingGame) to(InGame)") {
                    println!("State changed to Running");
                    status_sender.send_replace(Status::Running);
                }

                if line.ends_with("changing state from(Disconnected) to(Closed)") {
                    status_sender.send_replace(Status::Closed);
                }
            }

            Ok(())
        });

        command.kill_on_drop(false);

        Ok(RunningInstance {
            path: self.path,
            settings: self.settings,
            process,
            status: status_sender2,
            tracker,
            tracker_resv,
            manager: self.manager,
            name: self.name,
        })
    }
}

impl<'a> RunningInstance<'a> {
    pub async fn kill(&mut self) -> Result<(), ServerError> {
        self.check_and_set_status(Status::Running, Status::Stopping)
            .await?;

        self.process.kill().await?;
        self.process.wait().await?; // wait for the process to finish before dropping the Child as recommended by lib

        self.cleanup().await?;

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), ServerError> {
        self.check_and_set_status(Status::Running, Status::Stopping)
            .await?;

        // send /quit via rcon
        self.send_command_internal("/quit").await?;

        // wait for either
        // - process.wait
        // - status_recv.wait_for + 3s
        let mut status = self.status.subscribe();
        let _ = status.wait_for(|val| *val == Status::Closed).await?;

        if timeout(Duration::from_secs(3), self.process.wait())
            .await
            .is_err()
        {
            self.process.kill().await.ok();
            self.process.wait().await.ok();
        }

        self.cleanup().await?;

        Ok(())
    }

    async fn send_command_internal(&self, command: &str) -> Result<(), ServerError> {
        let mut connection = <Connection<TcpStream>>::builder()
            .enable_factorio_quirks(true)
            // TODO: think if that should be the actual ip (if not 0.0.0.0)
            .connect(
                format!("{}:{}", "127.0.0.1", self.settings.rcon_port),
                self.settings.rcon_pass.as_str(),
            )
            .await?;

        connection.cmd(command).await?;

        Ok(())
    }

    pub async fn send_command(&self, command: &str) -> Result<(), ServerError> {
        // TODO: this could fail (race-condition), cause:
        // 1. check_status(Running) -> succeeds
        // 2. kill()
        // 3. send_command_internal -> fail, cause factorio is turned off, will cause connection to time out
        // This means that that check is either never needed or should be locked for the whole command execution.
        self.check_status(Status::Running).await?;

        self.send_command_internal(command).await
    }

    async fn check_status(&self, expected: Status) -> Result<(), ServerError> {
        let mut status = self.status.subscribe();
        let status = status.borrow_and_update();
        if *status != expected {
            return Err(ServerError::NotAllowed(format!(
                "Status not as expected {:?} != {:?}",
                *status, expected
            )));
        }

        Ok(())
    }

    async fn check_and_set_status(
        &mut self,
        expected: Status,
        new_status: Status,
    ) -> Result<(), ServerError> {
        {
            let mut status = self.status.subscribe();
            let status = status.borrow_and_update();
            if *status != expected {
                return Err(ServerError::NotAllowed(format!(
                    "Status (with set) not as expected {:?} != {:?}",
                    *status, expected
                )));
            }
        }
        self.status.send_replace(new_status);

        Ok(())
    }

    async fn cleanup(&self) -> Result<(), ServerError> {
        self.manager
            .backup_files(
                &self.name,
                vec![
                    self.path.join("factorio-current.log"),
                    self.path.join("console.log"),
                    self.path.join("mods").join("mod-settings.dat"),
                    self.path.join("mods").join("mod-settings.json"),
                ],
            )
            .await?;
        Ok(())
    }
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

async fn build_mod_list_json(
    settings: &InstanceSettings,
    out_path: impl AsRef<Path>,
) -> Result<(), ServerError> {
    let mut mod_list = ModList { mods: vec![] };
    mod_list.mods.push(ModListMod {
        name: "base".to_string(),
        enabled: true,
    });
    if settings.factorio_version >= Version::from([2, 0, 0]) {
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
    for mod_ in &settings.mods {
        mod_list.mods.push(ModListMod {
            name: mod_.name.clone(),
            enabled: true,
        })
    }
    let json = serde_json::to_string(&mod_list)?;
    let mut mod_list_json_file = File::create(out_path).await?;
    mod_list_json_file.write_all(json.as_bytes()).await?;
    mod_list_json_file.flush().await?;

    Ok(())
}

// #[cfg(test)]
// mod test {
//     use crate::error::ServerError;
//     use crate::instance::{Instance, InstanceSettings};
//     use std::time::Duration;
//
//     #[tokio::test]
//     async fn start_kill() {
//         let settings = InstanceSettings::new("test3.zip".into(), "1.1.109".to_string()).unwrap();
//
//         let mut instance = Instance::start(settings, get_factorio_path(), nil)
//             .await
//             .unwrap();
//         tokio::time::sleep(Duration::from_secs(15)).await;
//         instance.kill().await.unwrap();
//     }
//
//     #[tokio::test]
//     async fn start_stop() {
//         let settings = InstanceSettings::new("test3.zip".into(), "1.1.109".to_string())
//             .await
//             .unwrap();
//
//         let mut instance = Instance::start(settings, get_factorio_path())
//             .await
//             .unwrap();
//         tokio::time::sleep(Duration::from_secs(15)).await;
//         if let Err(e) = instance.stop().await {
//             if let ServerError::NotAllowed(_) = e {
//                 panic!("{e}");
//             }
//             instance.kill().await.unwrap();
//             panic!("{e}");
//         }
//     }
//
//     fn get_factorio_path() -> &'static str {
//         #[cfg(target_os = "windows")]
//         return "C:\\Data\\Development\\GO\\factorio-windows";
//         #[cfg(target_os = "linux")]
//         return "/mnt/c/Data/Development/GO/factorio";
//     }
// }
