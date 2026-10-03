use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Metadata describing a plugin, without application preset counts.
#[derive(Debug, Clone)]
pub struct PluginDescriptor {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub path: PathBuf,
    pub version: String,
    pub subcategories: String,
}

/// Broad classification of pipe-separated plugin subcategories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginKind {
    Instrument,
    Effect,
    Other,
}

impl PluginKind {
    pub fn from_subcategories(value: &str) -> Self {
        let segments: Vec<_> = value.split('|').collect();
        if segments
            .iter()
            .any(|s| s.eq_ignore_ascii_case("Instrument"))
        {
            Self::Instrument
        } else if segments.iter().any(|s| s.eq_ignore_ascii_case("Fx")) {
            Self::Effect
        } else {
            Self::Other
        }
    }
}

use crate::discovery::{self, Vst3Bundle};

/// Callback for scan progress updates.
pub type ScanProgressCallback = Arc<dyn Fn(ScanProgress) + Send + Sync>;

/// Progress information during a plugin scan.
#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub scanned: usize,
    pub total: usize,
    pub current_plugin: Option<String>,
}

/// Scans the filesystem for VST3 plugins.
pub struct PluginScanner {
    search_paths: Vec<PathBuf>,
}

impl PluginScanner {
    /// Create a scanner using default platform-specific paths.
    pub fn new() -> Self {
        Self {
            search_paths: discovery::default_search_paths(),
        }
    }

    /// Create a scanner with custom search paths.
    pub fn with_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            search_paths: paths,
        }
    }

    /// Discover all VST3 bundles in the search paths.
    #[tracing::instrument(name = "discover_bundles", skip_all)]
    pub fn discover_bundles(&self) -> Vec<Vst3Bundle> {
        let mut bundles = Vec::new();
        let mut visited = HashSet::new();
        for search_path in &self.search_paths {
            if !search_path.is_dir() {
                continue;
            }
            self.scan_directory(search_path, &mut bundles, &mut visited);
        }
        bundles.sort_by_key(|a| a.name.to_lowercase());
        bundles
    }

    /// Scan with a progress callback.
    pub fn discover_with_progress(&self, callback: ScanProgressCallback) -> Vec<Vst3Bundle> {
        let bundles = self.discover_bundles();
        let total = bundles.len();
        for (i, bundle) in bundles.iter().enumerate() {
            callback(ScanProgress {
                scanned: i + 1,
                total,
                current_plugin: Some(bundle.name.clone()),
            });
        }
        bundles
    }

    /// Convert a bundle to a PluginDescriptor (metadata extraction).
    ///
    /// Extracts metadata from the bundle's filesystem structure:
    /// 1. `Contents/moduleinfo.json` (VST3 SDK 3.7+) — has subcategories, vendor, version
    /// 2. `Contents/Info.plist` — has bundle identifier for vendor extraction
    ///
    /// Does NOT load the plugin binary (which can crash the process).
    #[tracing::instrument(name = "bundle_to_info", skip_all, fields(bundle_name = %bundle.name))]
    pub fn bundle_to_info(bundle: &Vst3Bundle) -> PluginDescriptor {
        let metadata = query_plugin_metadata(bundle);

        let (vendor, version, subcategories) = metadata
            .map(|m| (m.vendor, m.version, m.subcategories))
            .unwrap_or_default();

        PluginDescriptor {
            id: bundle.name.to_lowercase().replace(' ', "-"),
            name: bundle.name.clone(),
            vendor,
            path: bundle.path.clone(),
            version,
            subcategories,
        }
    }

    fn scan_directory(
        &self,
        dir: &Path,
        bundles: &mut Vec<Vst3Bundle>,
        visited: &mut HashSet<PathBuf>,
    ) {
        let Ok(canonical) = dir.canonicalize() else {
            return;
        };
        if !visited.insert(canonical) {
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!(path = %dir.display(), error = %e, "failed to read plugin directory");
                return;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(bundle) = Vst3Bundle::from_path(&path) {
                    if path.canonicalize().is_ok_and(|p| visited.insert(p)) {
                        bundles.push(bundle);
                    }
                } else {
                    // Not a .vst3 bundle — recurse into vendor subfolders
                    self.scan_directory(&path, bundles, visited);
                }
            }
        }
    }
}

/// Metadata extracted from a VST3 bundle's filesystem metadata.
#[derive(Debug, Default)]
struct PluginMetadata {
    vendor: String,
    version: String,
    /// Pipe-separated subcategories (e.g. "Instrument|Synth").
    subcategories: String,
}

/// Query metadata from a VST3 bundle.
///
/// Tries these sources in order:
/// 1. `Contents/moduleinfo.json` (VST3 SDK 3.7+) — most reliable, has subcategories
/// 2. `Contents/Info.plist` (macOS bundles) — vendor/version only (no subcategories)
/// 3. Out-of-process scanner — loads the binary in a child process to query
///    IPluginFactory2 for subcategories, vendor, and version
///
/// Sources 2 and 3 are merged: plist provides vendor/version fallbacks,
/// the factory scanner provides subcategories.
fn query_plugin_metadata(bundle: &Vst3Bundle) -> Option<PluginMetadata> {
    // Try moduleinfo.json first (VST3 SDK 3.7+)
    let module_meta = parse_moduleinfo_json(&bundle.path);
    if module_meta
        .as_ref()
        .is_some_and(|meta| !meta.subcategories.is_empty())
    {
        return module_meta;
    }

    // Get plist-based metadata (vendor/version only, no subcategories)
    #[cfg(target_os = "macos")]
    let plist_meta = module_meta.or_else(|| parse_info_plist(&bundle.path));
    #[cfg(not(target_os = "macos"))]
    let plist_meta: Option<PluginMetadata> = module_meta;

    // Try out-of-process scanner for subcategories.
    // Skip plugins that previously crashed during scanning.
    let factory_meta = bundle.binary_path.as_ref().and_then(|binary_path| {
        if let Ok(bl) = get_blacklist().lock()
            && bl.contains(binary_path)
        {
            tracing::debug!(
                binary = %binary_path.display(),
                "skipping blacklisted plugin"
            );
            return None;
        }
        let scanner_bin = get_scanner_binary()?;
        query_metadata_out_of_process(scanner_bin, binary_path)
    });

    // Merge: factory has subcategories, plist has fallback vendor/version
    match (plist_meta, factory_meta) {
        (Some(plist), Some(factory)) => Some(PluginMetadata {
            vendor: if factory.vendor.is_empty() {
                plist.vendor
            } else {
                factory.vendor
            },
            version: if factory.version.is_empty() {
                plist.version
            } else {
                factory.version
            },
            subcategories: factory.subcategories,
        }),
        (None, Some(factory)) => Some(factory),
        (Some(plist), None) => Some(plist),
        (None, None) => None,
    }
}

// ── Crash blacklist ─────────────────────────────────────────────────
//
// Plugins that crash or timeout during out-of-process scanning are
// recorded in a JSON blacklist file. On subsequent scans, blacklisted
// paths are skipped entirely.

/// Global blacklist instance (loaded lazily on first access).
static BLACKLIST: OnceLock<Mutex<Blacklist>> = OnceLock::new();

fn get_blacklist() -> &'static Mutex<Blacklist> {
    BLACKLIST.get_or_init(|| Mutex::new(Blacklist::load()))
}

/// Persistent crash blacklist for plugin scanning.
struct Blacklist {
    paths: HashSet<PathBuf>,
    file_path: PathBuf,
}

impl Blacklist {
    /// Load the blacklist from disk, or create an empty one.
    fn load() -> Self {
        let file_path = blacklist_file_path();
        let paths = if file_path.exists() {
            match std::fs::read_to_string(&file_path) {
                Ok(content) => serde_json::from_str::<Vec<PathBuf>>(&content)
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
                Err(_) => HashSet::new(),
            }
        } else {
            HashSet::new()
        };
        if !paths.is_empty() {
            tracing::info!(count = paths.len(), "loaded plugin crash blacklist");
        }
        Self { paths, file_path }
    }

    /// Check if a binary path is blacklisted.
    fn contains(&self, path: &Path) -> bool {
        self.paths.contains(path)
    }

    /// Add a binary path to the blacklist and persist to disk.
    fn add(&mut self, path: PathBuf) {
        if self.paths.insert(path.clone()) {
            tracing::warn!(path = %path.display(), "blacklisted plugin (crashed during scan)");
            self.save();
        }
    }

    /// Remove all entries and persist.
    fn clear(&mut self) {
        if !self.paths.is_empty() {
            let count = self.paths.len();
            self.paths.clear();
            self.save();
            tracing::info!(count, "cleared plugin crash blacklist");
        }
    }

    fn save(&self) {
        let entries: Vec<&PathBuf> = self.paths.iter().collect();
        if let Ok(json) = serde_json::to_string_pretty(&entries) {
            if let Some(parent) = self.file_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&self.file_path, json) {
                tracing::warn!(error = %e, "failed to save plugin blacklist");
            }
        }
    }
}

/// Resolve the blacklist file path.
/// Located in the user's local data directory alongside the database.
/// Override persistent blacklist storage before any scan or bundle_to_info call.
/// Returns the rejected path if blacklist storage was already initialized.
pub fn set_blacklist_path(path: PathBuf) -> Result<(), PathBuf> {
    let mut blacklist = Blacklist {
        paths: HashSet::new(),
        file_path: path.clone(),
    };
    blacklist.paths = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    BLACKLIST.set(Mutex::new(blacklist)).map_err(|_| path)
}

fn blacklist_file_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("plugin-hostkit")
        .join("plugin_blacklist.json")
}

/// Clear the plugin crash blacklist. Blacklisted plugins will be
/// re-scanned on the next scan.
pub fn clear_blacklist() {
    if let Ok(mut bl) = get_blacklist().lock() {
        bl.clear();
    }
}

/// Get the number of blacklisted plugins.
pub fn blacklist_count() -> usize {
    get_blacklist().lock().map(|bl| bl.paths.len()).unwrap_or(0)
}

/// Timeout for the out-of-process scanner child process.
const SCANNER_TIMEOUT: Duration = Duration::from_secs(5);

/// Cached path to the plugin-scanner binary.
static SCANNER_BIN: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Override the helper executable before any scan or bundle_to_info call.
/// Returns the rejected path if helper lookup was already initialized.
pub fn set_scanner_binary(path: PathBuf) -> Result<(), PathBuf> {
    SCANNER_BIN.set(Some(path.clone())).map_err(|_| path)
}

fn get_scanner_binary() -> Option<&'static PathBuf> {
    SCANNER_BIN.get_or_init(find_scanner_binary).as_ref()
}

/// Locate the plugin-scanner helper binary.
///
/// Checks next to the current executable, which covers both:
/// - Development: `target/debug/plugin-scanner`
/// - Production: inside the app bundle
fn find_scanner_binary() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join("plugin-scanner");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

const MAX_METADATA_BYTES: u64 = 1024 * 1024;

struct MetadataCapture {
    file: Option<std::fs::File>,
    path: PathBuf,
}

impl MetadataCapture {
    fn new() -> std::io::Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..32 {
            let path = std::env::temp_dir().join(format!(
                "plugin-hostkit-{}-{}-{}.scan",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        file: Some(file),
                        path,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "metadata capture collisions",
        ))
    }

    fn read_bounded(&mut self) -> std::io::Result<Vec<u8>> {
        use std::io::{Read, Seek};
        let file = self.file.as_mut().expect("capture remains open");
        file.rewind()?;
        let mut bytes = Vec::new();
        file.take(MAX_METADATA_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_METADATA_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "metadata output too large",
            ));
        }
        Ok(bytes)
    }
}

impl Drop for MetadataCapture {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Spawn the out-of-process scanner for a single plugin binary.
///
/// Returns parsed metadata, or None if the child crashes, times out,
/// or produces invalid output.
fn query_metadata_out_of_process(scanner_bin: &Path, binary_path: &Path) -> Option<PluginMetadata> {
    let mut capture = MetadataCapture::new().ok()?;
    let output = capture.file.as_ref()?.try_clone().ok()?;
    let mut child = match Command::new(scanner_bin)
        .arg(binary_path)
        .stdout(output)
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!(binary = %binary_path.display(), error = %e, "failed to spawn out-of-process scanner");
            return None;
        }
    };

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    tracing::warn!(
                        binary = %binary_path.display(),
                        exit_code = ?status.code(),
                        "out-of-process scanner exited with error — blacklisting"
                    );
                    if let Ok(mut bl) = get_blacklist().lock() {
                        bl.add(binary_path.to_path_buf());
                    }
                    return None;
                }
                let bytes = capture.read_bounded().ok()?;
                let json: serde_json::Value = match serde_json::from_slice(&bytes) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            binary = %binary_path.display(),
                            error = %e,
                            "out-of-process scanner produced invalid output"
                        );
                        return None;
                    }
                };
                return Some(PluginMetadata {
                    vendor: json
                        .get("vendor")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    version: json
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    subcategories: json
                        .get("subcategories")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                });
            }
            Ok(None) => {
                if start.elapsed() > SCANNER_TIMEOUT
                    || capture
                        .file
                        .as_ref()
                        .and_then(|f| f.metadata().ok())
                        .is_none_or(|m| m.len() > MAX_METADATA_BYTES)
                {
                    tracing::warn!(
                        binary = %binary_path.display(),
                        timeout_secs = SCANNER_TIMEOUT.as_secs(),
                        "out-of-process scanner timed out — blacklisting"
                    );
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Ok(mut bl) = get_blacklist().lock() {
                        bl.add(binary_path.to_path_buf());
                    }
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                tracing::warn!(
                    binary = %binary_path.display(),
                    error = %e,
                    "failed to check out-of-process scanner status"
                );
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Parse `Contents/moduleinfo.json` for plugin metadata.
///
/// This file is generated by the VST3 SDK 3.7+ and contains:
/// ```json
/// {
///   "Classes": [{
///     "CID": "...",
///     "Category": "Audio Module Class",
///     "Name": "Plugin Name",
///     "Vendor": "Vendor Name",
///     "Version": "1.0.0",
///     "Sub Categories": ["Instrument", "Synth"],
///     ...
///   }],
///   "Factory Info": { "Vendor": "...", ... }
/// }
/// ```
fn parse_moduleinfo_json(bundle_path: &Path) -> Option<PluginMetadata> {
    let moduleinfo_path = bundle_path.join("Contents").join("moduleinfo.json");
    let content = std::fs::read_to_string(&moduleinfo_path).ok()?;
    if content.trim().is_empty() {
        tracing::debug!(
            path = %moduleinfo_path.display(),
            "moduleinfo.json is empty, skipping"
        );
        return None;
    }
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                path = %moduleinfo_path.display(),
                error = %e,
                "failed to parse moduleinfo.json"
            );
            return None;
        }
    };

    // Get factory-level vendor
    let factory_vendor = json
        .get("Factory Info")
        .and_then(|f| f.get("Vendor"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Find the first "Audio Module Class" entry
    let classes = json.get("Classes")?.as_array()?;
    for class in classes {
        let category = class.get("Category").and_then(|c| c.as_str()).unwrap_or("");
        if category == "Audio Module Class" {
            let subcategories = match class.get("Sub Categories") {
                Some(serde_json::Value::Array(values)) => values
                    .iter()
                    .map(serde_json::Value::as_str)
                    .collect::<Option<Vec<_>>>()?
                    .join("|"),
                Some(serde_json::Value::String(value)) => value.clone(),
                _ => String::new(),
            };
            let vendor = class
                .get("Vendor")
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .unwrap_or(&factory_vendor)
                .to_string();
            let version = class
                .get("Version")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            return Some(PluginMetadata {
                vendor,
                version,
                subcategories,
            });
        }
    }

    // No Audio Module Class found, but we still have factory info
    if !factory_vendor.is_empty() {
        return Some(PluginMetadata {
            vendor: factory_vendor,
            ..Default::default()
        });
    }

    None
}

/// Parse `Contents/Info.plist` for vendor and version (macOS only).
///
/// Uses the CFBundleIdentifier to extract vendor (e.g. "com.Arturia.Acid-V" → "Arturia")
/// and CFBundleShortVersionString for version.
#[cfg(target_os = "macos")]
fn parse_info_plist(bundle_path: &Path) -> Option<PluginMetadata> {
    let plist_path = bundle_path.join("Contents").join("Info.plist");
    let content = std::fs::read(&plist_path).ok()?;

    // Simple XML plist parsing — find key/string pairs
    let text = String::from_utf8_lossy(&content);

    let version = extract_plist_value(&text, "CFBundleShortVersionString").unwrap_or_default();
    let bundle_id = extract_plist_value(&text, "CFBundleIdentifier").unwrap_or_default();

    // Extract vendor from bundle ID: "com.Arturia.Acid-V.vst3" → "Arturia"
    let vendor = bundle_id.split('.').nth(1).unwrap_or("").to_string();

    if vendor.is_empty() && version.is_empty() {
        return None;
    }

    Some(PluginMetadata {
        vendor,
        version,
        ..Default::default()
    })
}

/// Extract a value from a simple XML plist by key name.
#[cfg(target_os = "macos")]
fn extract_plist_value(xml: &str, key: &str) -> Option<String> {
    let key_tag = format!("<key>{key}</key>");
    let key_pos = xml.find(&key_tag)?;
    let after_key = &xml[key_pos + key_tag.len()..];
    let string_start = after_key.find("<string>")? + 8;
    let string_end = after_key[string_start..].find("</string>")?;
    Some(after_key[string_start..string_start + string_end].to_string())
}

impl Default for PluginScanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sdk_metadata_without_categories_preserves_vendor_and_version() {
        let temp = tempfile::tempdir().unwrap();
        let contents = temp.path().join("Contents");
        fs::create_dir(&contents).unwrap();
        fs::write(contents.join("moduleinfo.json"), r#"{"Factory Info":{"Vendor":"Factory"},"Classes":[{"Category":"Audio Module Class","Vendor":"ClassVendor","Version":"1.2"}]}"#).unwrap();
        let bundle = Vst3Bundle {
            path: temp.path().to_path_buf(),
            name: "Fixture".into(),
            binary_path: None,
        };
        let metadata = query_plugin_metadata(&bundle).unwrap();
        assert_eq!(metadata.vendor, "ClassVendor");
        assert_eq!(metadata.version, "1.2");
        assert!(metadata.subcategories.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn scanner_capture_handles_large_output_and_inherited_descriptors() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let helper = temp.path().join("helper");
        let vendor = "x".repeat(256 * 1024);
        let json = serde_json::json!({"vendor":vendor,"version":"1","subcategories":"Instrument"});
        fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", json)).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let result = query_metadata_out_of_process(&helper, temp.path()).unwrap();
        assert_eq!(result.vendor.len(), 256 * 1024);
        fs::write(
            &helper,
            r#"#!/bin/sh
(sleep 2) &
printf '%s' '{"vendor":"descendant"}'
"#,
        )
        .unwrap();
        let start = Instant::now();
        assert_eq!(
            query_metadata_out_of_process(&helper, temp.path())
                .unwrap()
                .vendor,
            "descendant"
        );
        assert!(start.elapsed() < Duration::from_millis(1500));
    }

    #[test]
    #[cfg(unix)]
    fn discovery_deduplicates_directory_and_bundle_symlinks() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("Fixture.vst3")).unwrap();
        symlink(temp.path(), temp.path().join("cycle")).unwrap();
        symlink(
            temp.path().join("Fixture.vst3"),
            temp.path().join("Alias.vst3"),
        )
        .unwrap();
        assert_eq!(
            PluginScanner::with_paths(vec![temp.path().into(), temp.path().into()])
                .discover_bundles()
                .len(),
            1
        );
    }

    #[test]
    fn plugin_kind_matches_subcategories() {
        for (text, expected) in [
            ("Instrument|Synth", PluginKind::Instrument),
            ("fx|instrument", PluginKind::Instrument),
            ("Fx|Delay", PluginKind::Effect),
            ("Analyzer", PluginKind::Other),
            ("", PluginKind::Other),
        ] {
            assert_eq!(PluginKind::from_subcategories(text), expected);
        }
    }

    use super::*;
    use std::fs;

    #[test]
    fn scan_finds_bundles() {
        let tmp = tempfile::tempdir().unwrap();

        // Create two fake VST3 bundles
        for name in ["Alpha.vst3", "Beta.vst3"] {
            let bundle = tmp.path().join(name);
            fs::create_dir_all(bundle.join("Contents/x86_64-linux")).unwrap();
            fs::write(
                bundle
                    .join("Contents/x86_64-linux")
                    .join(name.replace(".vst3", ".so")),
                b"fake",
            )
            .unwrap();
        }

        let scanner = PluginScanner::with_paths(vec![tmp.path().to_path_buf()]);
        let bundles = scanner.discover_bundles();
        assert_eq!(bundles.len(), 2);
        assert_eq!(bundles[0].name, "Alpha");
        assert_eq!(bundles[1].name, "Beta");
    }

    #[test]
    fn scan_empty_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let scanner = PluginScanner::with_paths(vec![tmp.path().to_path_buf()]);
        let bundles = scanner.discover_bundles();
        assert!(bundles.is_empty());
    }

    #[test]
    fn bundle_to_info_converts() {
        let bundle = Vst3Bundle {
            path: PathBuf::from("/usr/lib/vst3/Vital.vst3"),
            name: "Vital".to_string(),
            binary_path: Some(PathBuf::from(
                "/usr/lib/vst3/Vital.vst3/Contents/x86_64-linux/Vital.so",
            )),
        };
        let info = PluginScanner::bundle_to_info(&bundle);
        assert_eq!(info.name, "Vital");
        assert_eq!(info.id, "vital");
        // Category depends on whether the binary exists and is loadable
        // For this test, the binary path doesn't exist so it falls back to Unknown
    }

    #[test]
    fn bundle_to_info_without_binary() {
        let bundle = Vst3Bundle {
            path: PathBuf::from("/usr/lib/vst3/NoBinary.vst3"),
            name: "NoBinary".to_string(),
            binary_path: None,
        };
        let info = PluginScanner::bundle_to_info(&bundle);
        assert_eq!(info.name, "NoBinary");
        assert_eq!(
            PluginKind::from_subcategories(&info.subcategories),
            PluginKind::Other
        );
        assert!(info.vendor.is_empty());
        assert!(info.subcategories.is_empty());
    }

    #[test]
    fn bundle_to_info_generates_slug_id() {
        let bundle = Vst3Bundle {
            path: PathBuf::from("/usr/lib/vst3/My Cool Synth.vst3"),
            name: "My Cool Synth".to_string(),
            binary_path: None,
        };
        let info = PluginScanner::bundle_to_info(&bundle);
        assert_eq!(info.id, "my-cool-synth");
    }

    #[test]
    fn query_metadata_returns_none_for_missing_bundle() {
        let bundle = Vst3Bundle {
            path: PathBuf::from("/tmp/nonexistent.vst3"),
            name: "nonexistent".to_string(),
            binary_path: None,
        };
        // Returns None because there's no moduleinfo.json or Info.plist
        assert!(query_plugin_metadata(&bundle).is_none());
    }

    #[test]
    fn query_metadata_from_moduleinfo_json() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = tmp.path().join("TestSynth.vst3");
        let contents = bundle_dir.join("Contents");
        fs::create_dir_all(&contents).unwrap();

        let moduleinfo = serde_json::json!({
            "Factory Info": { "Vendor": "Test Corp" },
            "Classes": [{
                "CID": "AAAA",
                "Category": "Audio Module Class",
                "Name": "TestSynth",
                "Vendor": "Test Corp",
                "Version": "2.1.0",
                "Sub Categories": ["Instrument", "Synth"]
            }]
        });
        fs::write(
            contents.join("moduleinfo.json"),
            serde_json::to_string_pretty(&moduleinfo).unwrap(),
        )
        .unwrap();

        let bundle = Vst3Bundle {
            path: bundle_dir,
            name: "TestSynth".to_string(),
            binary_path: None,
        };
        let meta = query_plugin_metadata(&bundle).unwrap();
        assert_eq!(meta.vendor, "Test Corp");
        assert_eq!(meta.version, "2.1.0");
        assert_eq!(meta.subcategories, "Instrument|Synth");
        assert_eq!(
            PluginKind::from_subcategories(&meta.subcategories),
            PluginKind::Instrument
        );
    }

    #[test]
    fn query_metadata_effect_from_moduleinfo() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = tmp.path().join("TestReverb.vst3");
        let contents = bundle_dir.join("Contents");
        fs::create_dir_all(&contents).unwrap();

        let moduleinfo = serde_json::json!({
            "Factory Info": { "Vendor": "FX Lab" },
            "Classes": [{
                "CID": "BBBB",
                "Category": "Audio Module Class",
                "Name": "TestReverb",
                "Vendor": "FX Lab",
                "Version": "1.0.0",
                "Sub Categories": "Fx|Reverb|Stereo"
            }]
        });
        fs::write(
            contents.join("moduleinfo.json"),
            serde_json::to_string_pretty(&moduleinfo).unwrap(),
        )
        .unwrap();

        let bundle = Vst3Bundle {
            path: bundle_dir.clone(),
            name: "TestReverb".to_string(),
            binary_path: None,
        };
        let info = PluginScanner::bundle_to_info(&bundle);
        assert_eq!(
            PluginKind::from_subcategories(&info.subcategories),
            PluginKind::Effect
        );
        assert_eq!(info.vendor, "FX Lab");
    }

    #[test]
    fn query_metadata_skips_empty_moduleinfo_json() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = tmp.path().join("FixtureSynth.vst3");
        let contents = bundle_dir.join("Contents");
        fs::create_dir_all(&contents).unwrap();
        fs::write(contents.join("moduleinfo.json"), "").unwrap();

        let bundle = Vst3Bundle {
            path: bundle_dir,
            name: "FixtureSynth".to_string(),
            binary_path: None,
        };
        // Should return None gracefully, not log a WARN
        assert!(query_plugin_metadata(&bundle).is_none());
    }
}
