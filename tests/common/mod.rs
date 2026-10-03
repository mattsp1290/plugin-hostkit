#![allow(dead_code)]
use std::path::{Path, PathBuf};
pub fn create_fake_vst3_bundle(parent_dir: &Path, name: &str) -> PathBuf {
    let bundle_dir = parent_dir.join(format!("{name}.vst3"));

    #[cfg(target_os = "linux")]
    let arch_dir = bundle_dir.join("Contents/x86_64-linux");
    #[cfg(target_os = "macos")]
    let arch_dir = bundle_dir.join("Contents/MacOS");
    #[cfg(target_os = "windows")]
    let arch_dir = bundle_dir.join("Contents/x86_64-win");
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let arch_dir = bundle_dir.join("Contents/x86_64-linux");

    std::fs::create_dir_all(&arch_dir).expect("failed to create bundle dirs");

    #[cfg(target_os = "windows")]
    let binary_name = format!("{name}.vst3");
    #[cfg(not(target_os = "windows"))]
    let binary_name = format!("{name}.so");

    let binary_path = arch_dir.join(binary_name);
    std::fs::write(&binary_path, b"FAKE_VST3_BINARY").expect("failed to write fake binary");

    bundle_dir
}

/// Create a fake .vstpreset file.
pub fn create_fake_preset(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).expect("failed to create preset dir");
    let path = dir.join(format!("{name}.vstpreset"));
    // Write a minimal preset file (no VST3 header, just raw state)
    let data: Vec<u8> = (0..64).collect();
    std::fs::write(&path, &data).expect("failed to write preset");
    path
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Platform-specific search paths for the Vital VST3 bundle.
fn vital_search_paths() -> Vec<Option<PathBuf>> {
    let mut paths = Vec::new();

    #[cfg(target_os = "linux")]
    {
        paths.push(home_dir().map(|h| h.join(".vst3/Vital.vst3")));
        paths.push(Some(PathBuf::from("/usr/lib/vst3/Vital.vst3")));
        paths.push(Some(PathBuf::from("/usr/local/lib/vst3/Vital.vst3")));
    }

    #[cfg(target_os = "macos")]
    {
        paths.push(home_dir().map(|h| h.join("Library/Audio/Plug-Ins/VST3/Vital.vst3")));
        paths.push(Some(PathBuf::from(
            "/Library/Audio/Plug-Ins/VST3/Vital.vst3",
        )));
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(pf) = std::env::var("PROGRAMFILES") {
            paths.push(Some(
                PathBuf::from(pf)
                    .join("Common Files")
                    .join("VST3")
                    .join("Vital.vst3"),
            ));
        }
        if let Ok(pf) = std::env::var("PROGRAMFILES(X86)") {
            paths.push(Some(
                PathBuf::from(pf)
                    .join("Common Files")
                    .join("VST3")
                    .join("Vital.vst3"),
            ));
        }
    }

    paths
}

/// Check if Vital is installed at any common path.
pub fn is_vital_installed() -> bool {
    vital_search_paths()
        .into_iter()
        .flatten()
        .any(|p| p.exists())
}

/// Find the Vital bundle path, if installed.
pub fn find_vital_path() -> Option<PathBuf> {
    vital_search_paths()
        .into_iter()
        .flatten()
        .find(|p| p.exists())
}
