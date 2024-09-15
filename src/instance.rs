use crate::error::ServerError;
use crate::factorio_tracker::FactorioTracker;
use rand::distributions::Alphanumeric;
use rand::Rng;
use rcon::Connection;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::sync::broadcast::channel;
use tokio::sync::watch::{Receiver, Sender};
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

pub struct Instance {
    settings: InstanceSettings,

    process: Child,
    status: Sender<Status>,
    tracker: FactorioTracker,
    tracker_resv: JoinHandle<Result<(), ServerError>>,
}

pub struct InstanceSettings {
    pub factorio_path: PathBuf,
    pub executable_path: PathBuf,
    pub saves_path: PathBuf,

    pub save: String, // Insert a save out of the `data` dir

    pub host: IpAddr,
    pub port: u16,

    pub rcon_host: IpAddr,
    pub rcon_port: u16,
    pub rcon_pass: String,
}

impl InstanceSettings {
    // This also sets the default values
    pub async fn new(factorio_path: impl AsRef<Path>, save: String) -> Result<Self, ServerError> {
        let default_addr = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));

        Ok(Self {
            factorio_path: factorio_path.as_ref().to_path_buf(),
            executable_path: Self::default_executable_path(),
            saves_path: "saves".into(),
            save,
            host: default_addr,
            port: 34197u16,
            rcon_host: default_addr,
            rcon_port: Self::get_random_port(default_addr).await?,
            rcon_pass: rand::thread_rng()
                .sample_iter(&Alphanumeric)
                .take(16)
                .map(char::from)
                .collect(),
        })
    }

    fn default_executable_path() -> PathBuf {
        #[cfg(target_os = "windows")]
        return "bin/x64/factorio.exe".into();
        #[cfg(not(target_os = "windows"))]
        return "bin/x64/factorio".into();
    }

    async fn get_random_port(addr: IpAddr) -> Result<u16, ServerError> {
        let listener = TcpListener::bind((addr, 0)).await?;

        let port = listener.local_addr()?.port();

        Ok(port)
    }

    pub fn executable_path(&mut self, executable_path: impl AsRef<Path>) {
        self.executable_path = executable_path.as_ref().to_path_buf();
    }

    pub fn saves_path(&mut self, saves_path: impl AsRef<Path>) {
        self.saves_path = saves_path.as_ref().to_path_buf();
    }

    pub fn save(&mut self, save: &str) {
        self.save = save.to_string();
    }

    pub fn host(&mut self, host: IpAddr) {
        self.host = host;
    }

    pub fn port(&mut self, port: u16) {
        self.port = port;
    }

    pub fn rcon_host(&mut self, host: IpAddr) {
        self.rcon_host = host;
    }

    pub fn rcon_port(&mut self, port: u16) {
        self.rcon_port = port;
    }

    pub fn rcon_pass(mut self, pass: String) {
        self.rcon_pass = pass;
    }
}

impl Instance {
    pub async fn start(settings: InstanceSettings) -> Result<Self, ServerError> {
        let exec_path = settings.factorio_path.join(&settings.executable_path);

        let save_path = settings
            .factorio_path
            .join(&settings.saves_path)
            .join(&settings.save);

        let (sender, mut recv) = channel::<String>(32);
        
        // TODO: remove this line as soon as the folder is cleaned up properly.
        let current = settings.factorio_path.join("factorio-current.log");
        std::fs::remove_file(&current).ok();

        let tracker = FactorioTracker::watch(
            settings.factorio_path.join("factorio-current.log"),
            settings.factorio_path.join(PID_FILE_NAME),
            sender,
        );

        let mut command = Command::new(exec_path);
        command
            .current_dir(&settings.factorio_path)
            .args([
                "--start-server",
                save_path.to_str().ok_or(ServerError::Utf8Error())?,
                "--console-log",
                "test.log",
                "--bind",
                settings.host.to_string().as_str(),
                "--port",
                settings.port.to_string().as_str(),
                "--rcon-bind",
                format!("{}:{}", settings.rcon_host, settings.rcon_port).as_str(),
                "--rcon-password",
                settings.rcon_pass.as_str(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null());

        let process = command.spawn()?;

        // save pid
        let pid = process.id().ok_or(ServerError::NotAllowed("Process has no pid".into()))?;
        let pid_path = settings.factorio_path.join(PID_FILE_NAME);
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
                    status_sender.send_replace(Status::Running);
                }

                if line.ends_with("changing state from(Disconnected) to(Closed)") {
                    status_sender.send_replace(Status::Closed);
                }
            }

            Ok(())
        });

        Ok(Self{
            settings,
            process,
            status: status_sender2,
            tracker,
            tracker_resv,
        })
    }

    pub async fn kill(&mut self) -> Result<(), ServerError> {
        self.check_and_set_status(Status::Running, Status::Stopping)
            .await?;

        self.process.kill().await?;
        self.process.wait().await?; // wait for the process to finish before dropping the Child as recommended by lib

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), ServerError> {
        self.check_and_set_status(Status::Running, Status::Stopping)
            .await?;

        // send /quit via rcon
        self.send_command_internal("/quit").await?;

        // TODO: kill factorio process when it is stopped, it doesn't seem to close the process itself after sending `/quit`
        // at least on windows in 1.1

        // wait for either
        // - process.wait
        // - status_recv.wait_for + 3s
        let mut status = self.status.subscribe();
        let _ = status.wait_for(|val| *val == Status::Closed).await?;

        if timeout(Duration::from_secs(3), self.process.wait()).await.is_err() {
            self.process.kill().await.ok();
            self.process.wait().await.ok();
        }

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
            return Err(ServerError::NotAllowed(
                format!("Status not as expected {:?} != {:?}", *status, expected),
            ));
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
                return Err(ServerError::NotAllowed(
                    format!("Status (with set) not as expected {:?} != {:?}", *status, expected),
                ));
            }
        }
        self.status.send_replace(new_status);

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::error::ServerError;
    use crate::instance::{Instance, InstanceSettings};
    use std::time::Duration;

    #[tokio::test]
    async fn start_kill() {
        let settings = InstanceSettings::new(get_factorio_path(), "test3.zip".into())
            .await
            .unwrap();

        let mut instance = Instance::start(settings).await.unwrap();
        tokio::time::sleep(Duration::from_secs(15)).await;
        instance.kill().await.unwrap();
    }

    #[tokio::test]
    async fn start_stop() {
        let settings = InstanceSettings::new(get_factorio_path(), "test3.zip".into())
            .await
            .unwrap();

        let mut instance = Instance::start(settings).await.unwrap();
        tokio::time::sleep(Duration::from_secs(15)).await;
        if let Err(e) = instance.stop().await {
            if let ServerError::NotAllowed(_) = e {
                panic!("{e}");
            }
            instance.kill().await.unwrap();
            panic!("{e}");
        }
    }

    fn get_factorio_path() -> &'static str {
        #[cfg(target_os = "windows")]
        return "C:\\Data\\Development\\GO\\factorio-windows";
        #[cfg(target_os = "linux")]
        return "/mnt/c/Data/Development/GO/factorio";
    }
}
