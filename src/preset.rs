use std::path::{Path, PathBuf};

/// A discovered preset belonging to its manager's plugin.
#[derive(Debug, Clone)]
pub struct PresetEntry {
    pub name: String,
    pub index: usize,
    pub path: Option<PathBuf>,
    pub category: String,
    pub origin: PresetOrigin,
}

/// The source of a preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetOrigin {
    Factory,
    File,
}

use crate::error::Vst3Error;

/// Manages presets for a VST3 plugin.
pub struct PresetManager {
    plugin_id: String,
    plugin_name: String,
    presets: Vec<PresetEntry>,
}

impl PresetManager {
    pub fn new(plugin_id: String, plugin_name: String) -> Self {
        Self {
            plugin_id,
            plugin_name,
            presets: Vec::new(),
        }
    }

    /// Scan for .vstpreset files in standard locations.
    ///
    /// On macOS, searches `~/Library/Audio/Presets/{Vendor}/{PluginName}/`
    /// and `/Library/Audio/Presets/{Vendor}/{PluginName}/` recursively.
    pub fn scan_preset_files(&mut self) -> Vec<PresetEntry> {
        let mut presets = Vec::new();
        let mut index = 0;
        for dir in preset_search_paths(&self.plugin_id, &self.plugin_name) {
            if !dir.is_dir() {
                continue;
            }
            scan_vstpreset_recursive(&dir, &dir, &mut presets, &mut index);
        }
        presets.sort_by_key(|a| a.name.to_lowercase());
        for (i, p) in presets.iter_mut().enumerate() {
            p.index = i;
        }
        self.presets = presets.clone();
        presets
    }

    /// Load a .vstpreset file into raw bytes.
    /// The caller is responsible for applying the state to a VstInstance.
    pub fn load_preset_file(path: &Path) -> Result<PresetData, Vst3Error> {
        let data = std::fs::read(path).map_err(Vst3Error::Io)?;
        PresetData::parse(&data)
    }

    /// Get the list of discovered presets.
    pub fn presets(&self) -> &[PresetEntry] {
        &self.presets
    }

    /// Query factory presets from a loaded VST3 instance via IUnitInfo.
    ///
    /// This requires a loaded and initialized VstInstance. The approach:
    /// 1. Query the component for IUnitInfo interface
    /// 2. Enumerate program lists via getProgramListCount/getProgramListInfo
    /// 3. For each list, enumerate entries via getProgramName
    /// 4. Map to PresetEntry with source=Factory
    ///
    /// Returns empty vec if IUnitInfo is not supported or FFI is not yet implemented.
    pub fn query_factory_presets(&mut self) -> Vec<PresetEntry> {
        // TODO: Implement IUnitInfo COM query when VST3 FFI is ready.
        // Most plugins don't implement IUnitInfo, so returning empty is common.
        Vec::new()
    }

    /// Add factory presets (from IUnitInfo program lists, when available).
    pub fn add_factory_presets(&mut self, names: Vec<String>) {
        let start_idx = self.presets.len();
        for (i, name) in names.into_iter().enumerate() {
            self.presets.push(PresetEntry {
                name,
                index: start_idx + i,
                path: None,
                origin: PresetOrigin::Factory,
                category: String::new(),
            });
        }
    }
}

/// Parsed .vstpreset file data.
#[derive(Debug, Clone)]
pub struct PresetData {
    /// Raw component state bytes.
    pub component_state: Vec<u8>,
    /// Raw controller state bytes (may be empty).
    pub controller_state: Vec<u8>,
}

impl PresetData {
    /// Parse a .vstpreset binary file.
    ///
    /// VST3 preset format:
    /// - Header: "VST3" magic + version + class ID
    /// - Chunk list with component and controller state
    fn parse(data: &[u8]) -> Result<Self, Vst3Error> {
        // Simplified parsing — full format has a chunk list
        // For a minimal implementation, treat the entire blob as component state
        if data.len() < 4 {
            return Err(Vst3Error::InitFailed("preset file too small".into()));
        }

        // Check for VST3 magic bytes
        let has_header = data.starts_with(b"VST3");

        if has_header && data.len() > 48 {
            // Skip the header (typically 48 bytes), rest is state
            Ok(Self {
                component_state: data[48..].to_vec(),
                controller_state: Vec::new(),
            })
        } else {
            // Treat entire file as component state
            Ok(Self {
                component_state: data.to_vec(),
                controller_state: Vec::new(),
            })
        }
    }
}

/// Recursively scan a directory for .vstpreset files.
fn scan_vstpreset_recursive(
    dir: &Path,
    base_dir: &Path,
    presets: &mut Vec<PresetEntry>,
    index: &mut usize,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            scan_vstpreset_recursive(&path, base_dir, presets, index);
        } else if entry.file_type().is_ok_and(|kind| kind.is_file())
            && path.extension().is_some_and(|ext| ext == "vstpreset")
        {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_string();

            // Category from first path component relative to base_dir
            let category = path
                .parent()
                .and_then(|p| p.strip_prefix(base_dir).ok())
                .and_then(|rel| rel.components().next())
                .and_then(|c| c.as_os_str().to_str())
                .unwrap_or("")
                .to_string();

            presets.push(PresetEntry {
                name,
                index: *index,
                path: Some(path),
                origin: PresetOrigin::File,
                category,
            });
            *index += 1;
        }
    }
}

/// Platform-specific paths where .vstpreset files are stored.
///
/// On macOS, searches `~/Library/Audio/Presets/{Vendor}/{PluginName}/`
/// and `/Library/Audio/Presets/{Vendor}/{PluginName}/` by scanning vendor
/// directories and matching plugin_name case-insensitively.
#[allow(unused_variables)]
fn preset_search_paths(plugin_id: &str, plugin_name: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "macos")]
    {
        let mut roots = Vec::new();
        if let Some(home) = crate::discovery::home_dir() {
            roots.push(home.join("Library/Audio/Presets"));
        }
        roots.push(PathBuf::from("/Library/Audio/Presets"));

        for root in &roots {
            if !root.is_dir() {
                continue;
            }
            // List vendor directories
            let Ok(vendor_entries) = std::fs::read_dir(root) else {
                continue;
            };
            for vendor_entry in vendor_entries.flatten() {
                let vendor_path = vendor_entry.path();
                if !vendor_path.is_dir() {
                    continue;
                }
                // List plugin directories within each vendor
                let Ok(plugin_entries) = std::fs::read_dir(&vendor_path) else {
                    continue;
                };
                for plugin_entry in plugin_entries.flatten() {
                    let plugin_path = plugin_entry.path();
                    if !plugin_path.is_dir() {
                        continue;
                    }
                    let dir_name = plugin_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("");
                    if dir_name.eq_ignore_ascii_case(plugin_name) {
                        paths.push(plugin_path);
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    if let Some(home) = crate::discovery::home_dir() {
        paths.push(home.join(".vst3/presets").join(plugin_id));
        paths.push(home.join(".local/share/vst3/presets").join(plugin_id));
    }

    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        paths.push(PathBuf::from(appdata).join("VST3 Presets").join(plugin_id));
    }

    paths
}

/// Count files with a given extension recursively.
fn count_files_recursive(dir: &Path, ext: &str) -> usize {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            count += count_files_recursive(&path, ext);
        } else if entry.file_type().is_ok_and(|kind| kind.is_file())
            && path.extension().is_some_and(|e| e == ext)
        {
            count += 1;
        }
    }
    count
}

/// Count .vstpreset files in standard locations without fully loading them.
pub fn count_preset_files(plugin_id: &str, plugin_name: &str) -> usize {
    let mut count = 0;
    for dir in preset_search_paths(plugin_id, plugin_name) {
        if dir.is_dir() {
            count += count_files_recursive(&dir, "vstpreset");
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_discovers_names_categories_and_ignores_other_files() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("Bass");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(temp.path().join("Lead.vstpreset"), b"fixture").unwrap();
        std::fs::write(nested.join("Deep.vstpreset"), b"fixture").unwrap();
        std::fs::write(nested.join("ignore.txt"), b"fixture").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(temp.path(), nested.join("cycle")).unwrap();
            std::os::unix::fs::symlink(&nested, temp.path().join("Alias")).unwrap();
        }
        let mut presets = Vec::new();
        let mut index = 0;
        scan_vstpreset_recursive(temp.path(), temp.path(), &mut presets, &mut index);
        presets.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(
            presets.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Deep", "Lead"]
        );
        assert_eq!(presets[0].category, "Bass");
        assert_eq!(index, 2);
        assert_ne!(presets[0].index, presets[1].index);
        assert_eq!(count_files_recursive(temp.path(), "vstpreset"), 2);
    }

    #[test]
    fn parse_minimal_preset() {
        let data = vec![0u8; 100];
        let preset = PresetData::parse(&data).unwrap();
        assert_eq!(preset.component_state.len(), 100);
    }

    #[test]
    fn parse_with_vst3_header() {
        let mut data = b"VST3".to_vec();
        data.extend_from_slice(&[0u8; 44]); // pad to 48 bytes header
        data.extend_from_slice(&[1, 2, 3, 4]); // state
        let preset = PresetData::parse(&data).unwrap();
        assert_eq!(preset.component_state, vec![1, 2, 3, 4]);
    }

    #[test]
    fn parse_too_small() {
        let data = vec![0u8; 2];
        assert!(PresetData::parse(&data).is_err());
    }

    #[test]
    fn preset_manager_factory_presets() {
        let mut mgr = PresetManager::new("test.plugin".into(), "Test Plugin".into());
        mgr.add_factory_presets(vec!["Init".into(), "Bass".into(), "Pad".into()]);
        assert_eq!(mgr.presets().len(), 3);
        assert_eq!(mgr.presets()[1].name, "Bass");
        assert_eq!(mgr.presets()[1].index, 1);
    }
}
