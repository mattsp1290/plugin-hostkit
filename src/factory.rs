//! VST3 IPluginFactory FFI for extracting plugin metadata.
//!
//! Loads VST3 binaries and queries the factory interface to extract
//! vendor, version, and subcategories without initializing the plugin.

use std::os::raw::c_void;
use std::path::Path;

/// Result code from VST3 COM calls.
type TResult = i32;
const K_RESULT_OK: TResult = 0;

/// 16-byte COM interface identifier.
#[allow(clippy::upper_case_acronyms)] // SDK ABI spelling.
type TUID = [u8; 16];

/// IPluginFactory2 IID: {0007B650-F24B4C0B-A464EDB9-F00B2ABB}
const IID_IPLUGIN_FACTORY2: TUID = [
    0x00, 0x07, 0xB6, 0x50, 0xF2, 0x4B, 0x4C, 0x0B, 0xA4, 0x64, 0xED, 0xB9, 0xF0, 0x0B, 0x2A, 0xBB,
];

/// Factory info returned by IPluginFactory::getFactoryInfo.
#[repr(C)]
struct PFactoryInfo {
    vendor: [u8; 64],
    url: [u8; 256],
    email: [u8; 128],
    flags: i32,
}

impl Default for PFactoryInfo {
    fn default() -> Self {
        Self {
            vendor: [0; 64],
            url: [0; 256],
            email: [0; 128],
            flags: 0,
        }
    }
}

/// Basic class info from IPluginFactory::getClassInfo.
#[repr(C)]
struct PClassInfo {
    cid: TUID,
    cardinality: i32,
    category: [u8; 32],
    name: [u8; 64],
}

impl Default for PClassInfo {
    fn default() -> Self {
        Self {
            cid: [0; 16],
            cardinality: 0,
            category: [0; 32],
            name: [0; 64],
        }
    }
}

/// Extended class info from IPluginFactory2::getClassInfo2.
/// Contains subcategories, vendor, and version.
#[repr(C)]
struct PClassInfo2 {
    cid: TUID,
    cardinality: i32,
    category: [u8; 32],
    name: [u8; 64],
    class_flags: u32,
    sub_categories: [u8; 128],
    vendor: [u8; 64],
    version: [u8; 64],
    sdk_version: [u8; 64],
}

impl Default for PClassInfo2 {
    fn default() -> Self {
        Self {
            cid: [0; 16],
            cardinality: 0,
            category: [0; 32],
            name: [0; 64],
            class_flags: 0,
            sub_categories: [0; 128],
            vendor: [0; 64],
            version: [0; 64],
            sdk_version: [0; 64],
        }
    }
}

// ── Vtable layouts (COM-style, C ABI) ─────────────────────────────

/// FUnknown vtable (3 methods, 24 bytes on 64-bit).
#[repr(C)]
struct FUnknownVtbl {
    query_interface:
        unsafe extern "C" fn(this: *mut c_void, iid: *const TUID, obj: *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(this: *mut c_void) -> u32,
    release: unsafe extern "C" fn(this: *mut c_void) -> u32,
}

/// IPluginFactory vtable (extends FUnknown, 4 additional methods).
#[repr(C)]
struct IPluginFactoryVtbl {
    base: FUnknownVtbl,
    get_factory_info: unsafe extern "C" fn(this: *mut c_void, info: *mut PFactoryInfo) -> TResult,
    count_classes: unsafe extern "C" fn(this: *mut c_void) -> i32,
    get_class_info:
        unsafe extern "C" fn(this: *mut c_void, index: i32, info: *mut PClassInfo) -> TResult,
    create_instance: unsafe extern "C" fn(
        this: *mut c_void,
        cid: *const TUID,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> TResult,
}

/// IPluginFactory2 vtable (extends IPluginFactory, 1 additional method).
#[repr(C)]
struct IPluginFactory2Vtbl {
    base: IPluginFactoryVtbl,
    get_class_info2:
        unsafe extern "C" fn(this: *mut c_void, index: i32, info: *mut PClassInfo2) -> TResult,
}

/// Metadata extracted from a VST3 binary's IPluginFactory.
#[derive(Debug, Default)]
pub struct FactoryMetadata {
    pub vendor: String,
    pub version: String,
    /// Raw pipe-separated subcategories (e.g. "Instrument|Synth").
    pub subcategories: String,
}

/// Query metadata from a VST3 binary by loading its IPluginFactory.
///
/// This loads the shared library, calls `GetPluginFactory`, queries
/// factory info and class info, then releases the factory.
///
/// Returns None if loading fails or the binary has no audio module classes.
pub fn query_factory_metadata(binary_path: &Path) -> Option<FactoryMetadata> {
    unsafe { query_factory_metadata_unsafe(binary_path) }
}

unsafe fn query_factory_metadata_unsafe(binary_path: &Path) -> Option<FactoryMetadata> {
    unsafe {
        // Step 1: Load the shared library
        let lib = libloading::Library::new(binary_path).ok()?;

        // Step 2: Get GetPluginFactory entry point
        type GetFactoryProc = unsafe extern "C" fn() -> *mut c_void;
        let get_factory: libloading::Symbol<GetFactoryProc> = lib.get(b"GetPluginFactory").ok()?;

        // Step 3: Call GetPluginFactory
        let factory = get_factory();
        if factory.is_null() {
            return None;
        }

        // The object's first word is the vtable pointer
        let vtbl = *(factory as *const *const IPluginFactoryVtbl);

        // Step 4: Get factory info (vendor name)
        let mut factory_info = PFactoryInfo::default();
        let vendor = if ((*vtbl).get_factory_info)(factory, &mut factory_info) == K_RESULT_OK {
            cstr_from_buf(&factory_info.vendor)
        } else {
            String::new()
        };

        // Step 5: Count audio module classes
        let count = ((*vtbl).count_classes)(factory);
        if count <= 0 {
            ((*vtbl).base.release)(factory);
            return Some(FactoryMetadata {
                vendor,
                ..Default::default()
            });
        }

        // Step 6: Try to get IPluginFactory2 for subcategories
        let mut factory2: *mut c_void = std::ptr::null_mut();
        let has_factory2 =
            ((*vtbl).base.query_interface)(factory, &IID_IPLUGIN_FACTORY2, &mut factory2)
                == K_RESULT_OK
                && !factory2.is_null();

        let mut result = FactoryMetadata {
            vendor,
            ..Default::default()
        };

        // Step 7: Iterate classes looking for audio processor components
        if has_factory2 {
            let vtbl2 = *(factory2 as *const *const IPluginFactory2Vtbl);
            for i in 0..count {
                let mut info2 = PClassInfo2::default();
                if ((*vtbl2).get_class_info2)(factory2, i, &mut info2) == K_RESULT_OK {
                    let category = cstr_from_buf(&info2.category);
                    // Audio Module Class is the standard category for audio processors
                    if category == "Audio Module Class" {
                        result.subcategories = cstr_from_buf(&info2.sub_categories);
                        result.version = cstr_from_buf(&info2.version);
                        // Prefer per-class vendor over factory vendor
                        let class_vendor = cstr_from_buf(&info2.vendor);
                        if !class_vendor.is_empty() {
                            result.vendor = class_vendor;
                        }
                        break;
                    }
                }
            }
            // Release factory2 reference
            let vtbl2_base = *(factory2 as *const *const FUnknownVtbl);
            ((*vtbl2_base).release)(factory2);
        } else {
            // Fallback: use PClassInfo (no subcategories available)
            for i in 0..count {
                let mut info = PClassInfo::default();
                if ((*vtbl).get_class_info)(factory, i, &mut info) == K_RESULT_OK {
                    let category = cstr_from_buf(&info.category);
                    if category == "Audio Module Class" {
                        // Can't determine subcategories from PClassInfo
                        break;
                    }
                }
            }
        }

        // Step 8: Release factory
        ((*vtbl).base.release)(factory);

        // Don't unload the library here — let it drop naturally.
        // Some plugins have global state that crashes on premature unload.
        std::mem::forget(lib);

        Some(result)
    }
}

/// Extract a UTF-8 string from a null-terminated byte buffer.
fn cstr_from_buf(buf: &[u8]) -> String {
    // Find the first null byte
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cstr_from_buf_extracts_string() {
        let mut buf = [0u8; 64];
        buf[..5].copy_from_slice(b"Hello");
        assert_eq!(cstr_from_buf(&buf), "Hello");
    }

    #[test]
    fn cstr_from_buf_empty() {
        let buf = [0u8; 32];
        assert_eq!(cstr_from_buf(&buf), "");
    }

    #[test]
    fn cstr_from_buf_full() {
        let buf = [b'X'; 16]; // No null terminator
        assert_eq!(cstr_from_buf(&buf), "XXXXXXXXXXXXXXXX");
    }

    #[test]
    fn nonexistent_binary_returns_none() {
        let result = query_factory_metadata(Path::new("/nonexistent/plugin.so"));
        assert!(result.is_none());
    }

    #[test]
    fn factory_info_struct_sizes() {
        // Verify our struct layouts match the VST3 SDK
        assert_eq!(std::mem::size_of::<PFactoryInfo>(), 452);
        assert_eq!(std::mem::size_of::<PClassInfo>(), 116);
        assert_eq!(std::mem::size_of::<PClassInfo2>(), 440);
        assert_eq!(std::mem::size_of::<TUID>(), 16);
    }

    #[test]
    fn vtable_sizes() {
        // Verify vtable layouts have correct sizes
        let ptr_size = std::mem::size_of::<usize>();
        assert_eq!(std::mem::size_of::<FUnknownVtbl>(), 3 * ptr_size);
        assert_eq!(std::mem::size_of::<IPluginFactoryVtbl>(), 7 * ptr_size);
        assert_eq!(std::mem::size_of::<IPluginFactory2Vtbl>(), 8 * ptr_size);
    }
}
