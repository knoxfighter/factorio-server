use crate::error::ServerError;
use crate::utilities::get_file_size;
use std::fs::Metadata;
use std::io::SeekFrom::Start;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};
use tokio::sync::broadcast::Sender;
use tokio::task::JoinHandle;

pub(crate) struct FactorioTracker {
    handle: Option<JoinHandle<Result<(), ServerError>>>,
    file_pos: u64,
    last_size: u64,
}

impl FactorioTracker {
    pub(crate) fn watch(
        factorio_log: impl AsRef<Path> + Send + Sync + 'static,
        factorio_pid: impl AsRef<Path> + Send + Sync + 'static,
        sender: Sender<String>,
    ) -> Self {
        let mut this = Self {
            handle: None,
            file_pos: 0,
            last_size: 0,
        };

        let t = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            // let mut interval = tokio::time::interval(Duration::from_millis(10));
            'outer: loop {
                'waiter: loop {
                    // check if file already exists and if not, we don't need to wait for a smaller filesize.
                    // and we can skip reading the file xD
                    if !factorio_log.as_ref().exists() {
                        break;
                    }

                    // check if file size changed
                    if let Ok(mut file) = File::open(&factorio_log).await {
                        let metadata = file.metadata().await?;
                        let size = get_file_size(metadata);

                        if size < this.last_size {
                            // file got smaller, read whole file
                            this.last_size = 0;
                            this.file_pos = 0;
                        }

                        if size > this.last_size {
                            // file got bigger, read lines
                            this.last_size = size;

                            loop {
                                file.seek(Start(this.file_pos)).await?;

                                let mut file_buf = BufReader::new(&mut file);

                                let mut out = String::new();
                                let read = file_buf.read_line(&mut out).await?;
                                if read == 0 {
                                    // EOF reached, we do nothing more here
                                    // also happens if nothing is read :D
                                    break 'waiter;
                                } else {
                                    this.file_pos += read as u64;
                                }

                                if out.ends_with('\n') {
                                    out.pop();

                                    if out.ends_with('\r') {
                                        out.pop();
                                    }
                                }

                                sender.send(out)?;
                            }
                        }
                    } else {
                        break;
                    }

                    // check if factorio is still running
                    if let Ok(mut file) = File::open(factorio_pid.as_ref()).await {
                        let mut buf = String::new();
                        file.read_to_string(&mut buf).await?;
                        let pid = Pid::from_str(&buf)?;

                        let system = System::new_with_specifics(
                            RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
                        );
                        let process = system.process(pid);
                        if process.is_none() {
                            sender.send(String::from("factorio process stopped"))?;
                            break 'outer;
                        }
                    }
                }
                interval.tick().await;
            }
            Ok(())
        });

        this.handle = Some(t);

        this
    }
}
