use crate::error::ServerError;
use std::fs::Metadata;
use std::io;
use std::net::IpAddr;
use std::path::Path;
use tokio::net::TcpListener;

pub(crate) fn get_file_size(metadata: Metadata) -> u64 {
    #[cfg(target_family = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_size()
    }
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.size()
    }
}

pub(crate) async fn get_random_port(addr: IpAddr) -> Result<u16, ServerError> {
    let listener = TcpListener::bind((addr, 0)).await?;

    let port = listener.local_addr()?.port();

    Ok(port)
}

pub(crate) fn symlink_file(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::symlink_file;
        symlink_file(src, dst)
    }
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::symlink;
        symlink(src, dst)
    }
}

pub(crate) fn symlink_folder(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::symlink_dir;
        symlink_dir(src, dst)
    }
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::symlink;
        symlink(src, dst)
    }
}
