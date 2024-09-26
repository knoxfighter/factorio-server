use std::path::PathBuf;

pub(crate) struct Cache {
    root_path: PathBuf,
    factorio_dir: PathBuf,
    mods_dir: PathBuf,
}

impl Cache {
    pub(crate) fn new(root_path: PathBuf) -> Self {
        Self {
            factorio_dir: root_path.join("factorio"),
            mods_dir: root_path.join("mods"),
            root_path,
        }
    }

    pub(crate) async fn get_version(&self, version: &String) -> PathBuf {
        // TODO: actually download and install a version if not already there.
        self.factorio_dir.join(version)
    }
}
