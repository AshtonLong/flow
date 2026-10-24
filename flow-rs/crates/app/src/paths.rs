//! Where Flow keeps its files, in an installed build and in development.
//! Mirrors `src/main/windows/paths.ts` and the path setup in `src/main/index.ts`.

use std::path::{Path, PathBuf};

/// The app's directories, resolved once at startup.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `%APPDATA%\Flow` (or `FLOW_USER_DATA`): config, keys, history, logs.
    pub user_data: PathBuf,
    /// `%LOCALAPPDATA%\Flow\models` (or `FLOW_MODELS_DIR`).
    pub models: PathBuf,
    pub logs: PathBuf,
    /// Optional GPU pack: a CUDA build of the speech library plus the CUDA runtime.
    pub gpu_pack: PathBuf,
    /// Bundled resources: `catalog.json` and its signature, icons.
    pub resources: PathBuf,
    /// The speech library shipped with the app.
    pub engine_library: PathBuf,
    /// This executable.
    pub exe: PathBuf,
    /// True for an installed build (resources next to the executable).
    pub packaged: bool,
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn known_dir(var: &str) -> PathBuf {
    env_dir(var).unwrap_or_else(std::env::temp_dir)
}

/// The prebuilt speech library from an installed Electron Flow, used in development.
fn electron_engine_library() -> PathBuf {
    known_dir("LOCALAPPDATA").join(
        r"Programs\Flow\resources\app.asar.unpacked\node_modules\@transcribe-cpp\win32-x64-cpu-vulkan\transcribe.dll",
    )
}

/// Walks up from `start` looking for the repository's `resources/catalog.json`.
fn find_repo_resources(start: &Path) -> Option<PathBuf> {
    start.ancestors().map(|dir| dir.join("resources")).find(|dir| dir.join("catalog.json").is_file())
}

impl Paths {
    pub fn resolve() -> Self {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("Flow.exe"));
        let exe_dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        let user_data = env_dir("FLOW_USER_DATA").unwrap_or_else(|| known_dir("APPDATA").join("Flow"));
        let local = known_dir("LOCALAPPDATA").join("Flow");
        let models = env_dir("FLOW_MODELS_DIR").unwrap_or_else(|| local.join("models"));
        let installed_resources = exe_dir.join("resources");
        let packaged = installed_resources.join("catalog.json").is_file();
        let resources = env_dir("FLOW_RESOURCES")
            .or_else(|| packaged.then(|| installed_resources.clone()))
            .or_else(|| find_repo_resources(&exe_dir))
            .unwrap_or(installed_resources);
        let bundled_engine = exe_dir.join("engine").join("transcribe.dll");
        let engine_library =
            if bundled_engine.is_file() || packaged { bundled_engine } else { electron_engine_library() };
        Self {
            logs: user_data.join("logs"),
            user_data,
            models,
            gpu_pack: local.join("gpu-pack"),
            resources,
            engine_library,
            exe,
            packaged,
        }
    }

    pub fn resource(&self, name: &str) -> PathBuf {
        self.resources.join(name)
    }

    pub fn config_file(&self) -> PathBuf {
        self.user_data.join("config.toml")
    }
}
