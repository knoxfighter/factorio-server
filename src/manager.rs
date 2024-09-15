use std::path::PathBuf;

pub struct Manager {
    root_path: PathBuf,
    cache_path: PathBuf,
    data_path: PathBuf,
    instances_path: PathBuf,
}

impl Manager {
    pub fn new(root_path: impl Into<PathBuf>) -> Self {
        let root_path = root_path.into();
        
        Self {
            root_path: root_path.clone(),
            cache_path: root_path.join("cache"),
            data_path: root_path.join("data"),
            instances_path: root_path.join("instances"),
        }
    }
    
    // pub fn generate_instance(instance_settings: InstanceSettings) -> Instance {
    //     Instance::new(instance_settings)
    // }
}
