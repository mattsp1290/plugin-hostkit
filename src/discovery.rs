use std::path::{Path, PathBuf};

/// Platform-specific VST3 search directories.
pub fn default_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "linux")]
    {
        if let Some(home) = home_dir() {
            paths.push(home.join(".vst3"));
        }
        paths.push(PathBuf::from("/usr/lib/vst3"));
        paths.push(PathBuf::from("/usr/local/lib/vst3"));
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = home_dir() {
            paths.push(home.join("Library/Audio/Plug-Ins/VST3"));
        }
        paths.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(pf) = std::env::var("PROGRAMFILES") {
            paths.push(PathBuf::from(pf).join("Common Files").join("VST3"));
        }
        if let Ok(pf) = std::env::var("PROGRAMFILES(X86)") {
            paths.push(PathBuf::from(pf).join("Common Files").join("VST3"));
        }
    }

    paths
}

/// Represents a discovered VST3 bundle on disk.
#[derive(Debug, Clone)]
pub struct Vst3Bundle {
    /// Path to the .vst3 bundle directory.
    pub path: PathBuf,
    /// Plugin name extracted from the bundle filename.
    pub name: String,
    /// Path to the platform-specific binary within the bundle.
    pub binary_path: Option<PathBuf>,
}

impl Vst3Bundle {
    /// Try to parse a .vst3 bundle from a directory path.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        if ext != "vst3" {
            return None;
        }
        if !path.is_dir() {
            return None;
        }

        let name = path.file_stem()?.to_str()?.to_string();
        let binary_path = find_binary(path);

        Some(Self {
            path: path.to_path_buf(),
            name,
            binary_path,
        })
    }
}

/// Find the platform-specific binary within a VST3 bundle.
fn find_binary(bundle_path: &Path) -> Option<PathBuf> {
    let contents = bundle_path.join("Contents");

    #[cfg(target_os = "linux")]
    let arch_dir = contents.join("x86_64-linux");

    #[cfg(target_os = "macos")]
    let arch_dir = contents.join("MacOS");

    #[cfg(target_os = "windows")]
    let arch_dir = contents.join("x86_64-win");

    // Fallback for non-standard platforms (e.g. running tests)
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let arch_dir = contents.join("x86_64-linux");

    if !arch_dir.is_dir() {
        return None;
    }

    let bundle_name = bundle_path.file_stem()?.to_str()?;

    #[cfg(target_os = "windows")]
    let binary_name = format!("{bundle_name}.vst3");
    #[cfg(not(target_os = "windows"))]
    let binary_name = format!("{bundle_name}.so");

    let binary = arch_dir.join(&binary_name);
    if binary.is_file() {
        Some(binary)
    } else {
        // Try any .so/.dylib/.vst3 file in the directory
        std::fs::read_dir(&arch_dir).ok()?.find_map(|entry| {
            let entry = entry.ok()?;
            let p = entry.path();
            if p.is_file() { Some(p) } else { None }
        })
    }
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn default_paths_not_empty() {
        let paths = default_search_paths();
        assert!(!paths.is_empty());
    }

    #[test]
    fn parse_bundle_from_temp_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = tmp.path().join("TestPlugin.vst3");

        #[cfg(target_os = "linux")]
        let (arch_subdir, binary_name) = ("Contents/x86_64-linux", "TestPlugin.so");
        #[cfg(target_os = "macos")]
        let (arch_subdir, binary_name) = ("Contents/MacOS", "TestPlugin.so");
        #[cfg(target_os = "windows")]
        let (arch_subdir, binary_name) = ("Contents/x86_64-win", "TestPlugin.vst3");

        fs::create_dir_all(bundle_dir.join(arch_subdir)).unwrap();
        let binary = bundle_dir.join(arch_subdir).join(binary_name);
        fs::write(&binary, b"fake").unwrap();

        let bundle = Vst3Bundle::from_path(&bundle_dir).unwrap();
        assert_eq!(bundle.name, "TestPlugin");
        assert!(bundle.binary_path.is_some());
    }

    #[test]
    fn non_vst3_dir_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("NotAPlugin");
        fs::create_dir_all(&dir).unwrap();
        assert!(Vst3Bundle::from_path(&dir).is_none());
    }
}
