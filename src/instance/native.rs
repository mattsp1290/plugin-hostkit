//! Native bundle loading, creation, and reverse-lifecycle bus cleanup.

use super::*;

/// Deactivate buses in reverse order using raw pointers.
///
/// Best-effort: all failures are trace-logged, never returned as errors.
/// Used by both `VstInstance::deactivate_buses()` and the terminate closure
/// (which operates on `TerminateCtx` fields instead of `&self`).
///
/// SAFETY: `component` and `component_vtbl` must be valid, non-null pointers.
pub(super) unsafe fn deactivate_buses_raw(
    component: *mut c_void,
    component_vtbl: *const IComponentVtbl,
    bus_info: &BusInfo,
    name: &str,
) {
    // Event inputs first (reverse of activation order).
    for i in (0..bus_info.num_event_inputs).rev() {
        let result =
            unsafe { ((*component_vtbl).activate_bus)(component, K_EVENT, K_INPUT, i as i32, 0) };
        if result != K_RESULT_OK {
            tracing::trace!(
                plugin = %name,
                bus_index = i,
                result,
                "deactivateBus(event input) failed (best-effort)"
            );
        }
    }
    // Audio outputs last (reverse of activation order).
    for i in (0..bus_info.num_audio_outputs).rev() {
        let result =
            unsafe { ((*component_vtbl).activate_bus)(component, K_AUDIO, K_OUTPUT, i as i32, 0) };
        if result != K_RESULT_OK {
            tracing::trace!(
                plugin = %name,
                bus_index = i,
                result,
                "deactivateBus(audio output) failed (best-effort)"
            );
        }
    }
}

/// Resolve the .vst3 bundle directory from a binary path.
///
/// Binary is at Something.vst3/Contents/MacOS/Something — returns the .vst3 dir.
#[cfg(target_os = "macos")]
pub(super) fn vst3_bundle_path(library_path: &Path) -> Result<std::path::PathBuf, Vst3Error> {
    library_path
        .ancestors()
        .find(|p| {
            p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("vst3"))
        })
        .map(|p| p.to_path_buf())
        .ok_or_else(|| Vst3Error::LoadError("could not find .vst3 bundle directory".into()))
}

/// Create and load a CFBundle for a VST3 plugin BEFORE dlopen.
///
/// This must be called before `Library::new()` (dlopen) so that the CFBundle is
/// registered in the global bundle table when static constructors and ObjC +load
/// methods run. VSTGUI-based plugins look up their bundle during
/// static initialization to cache resource paths, font managers, and image loaders.
/// If the bundle isn't registered yet, these lookups return null and the cached nulls
/// cause SIGSEGV later in `IPlugView::attached()`.
///
/// Returns the CFBundleRef (retained, caller must not release — the plugin retains it).
#[cfg(target_os = "macos")]
pub(super) unsafe fn prepare_macos_bundle(library_path: &Path) -> Result<*mut c_void, Vst3Error> {
    let bundle_path = vst3_bundle_path(library_path)?;
    let bundle_path_str = bundle_path
        .to_str()
        .ok_or_else(|| Vst3Error::LoadError("bundle path is not valid UTF-8".into()))?;

    // Load CoreFoundation dynamically
    let cf = unsafe {
        libloading::Library::new(
            "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation",
        )
    }
    .map_err(|e| Vst3Error::LoadError(format!("failed to load CoreFoundation: {e}")))?;

    type CFStringCreateWithCString =
        unsafe extern "C" fn(*mut c_void, *const u8, u32) -> *mut c_void;
    type CFURLCreateWithFileSystemPath =
        unsafe extern "C" fn(*mut c_void, *mut c_void, i64, bool) -> *mut c_void;
    type CFBundleCreate = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    // Use CFBundleLoadExecutableAndReturnError (preferred API)
    // rather than the older CFBundleLoadExecutable.
    type CFBundleLoadExecutableAndReturnError =
        unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> u8;
    type CFRelease = unsafe extern "C" fn(*mut c_void);

    let cf_string_create: CFStringCreateWithCString =
        *unsafe { cf.get::<CFStringCreateWithCString>(b"CFStringCreateWithCString") }
            .map_err(|e| Vst3Error::LoadError(format!("CFStringCreateWithCString: {e}")))?;
    let cf_url_create: CFURLCreateWithFileSystemPath =
        *unsafe { cf.get::<CFURLCreateWithFileSystemPath>(b"CFURLCreateWithFileSystemPath") }
            .map_err(|e| Vst3Error::LoadError(format!("CFURLCreateWithFileSystemPath: {e}")))?;
    let cf_bundle_create: CFBundleCreate = *unsafe { cf.get::<CFBundleCreate>(b"CFBundleCreate") }
        .map_err(|e| Vst3Error::LoadError(format!("CFBundleCreate: {e}")))?;
    let cf_bundle_load_executable: CFBundleLoadExecutableAndReturnError = *unsafe {
        cf.get::<CFBundleLoadExecutableAndReturnError>(b"CFBundleLoadExecutableAndReturnError")
    }
    .map_err(|e| Vst3Error::LoadError(format!("CFBundleLoadExecutableAndReturnError: {e}")))?;
    let cf_release: CFRelease = *unsafe { cf.get::<CFRelease>(b"CFRelease") }
        .map_err(|e| Vst3Error::LoadError(format!("CFRelease: {e}")))?;

    // Keep CoreFoundation loaded for process lifetime
    std::mem::forget(cf);

    unsafe {
        // Create CFString from path (kCFStringEncodingUTF8 = 0x08000100)
        let path_cstr = std::ffi::CString::new(bundle_path_str)
            .map_err(|_| Vst3Error::LoadError("null byte in path".into()))?;
        let cf_path = cf_string_create(std::ptr::null_mut(), path_cstr.as_ptr() as _, 0x08000100);
        if cf_path.is_null() {
            return Err(Vst3Error::LoadError(
                "CFStringCreateWithCString failed".into(),
            ));
        }

        // Create CFURL (kCFURLPOSIXPathStyle = 0)
        let cf_url = cf_url_create(std::ptr::null_mut(), cf_path, 0, true);
        cf_release(cf_path);
        if cf_url.is_null() {
            return Err(Vst3Error::LoadError(
                "CFURLCreateWithFileSystemPath failed".into(),
            ));
        }

        // Create CFBundle — this registers it in the global bundle table
        let bundle = cf_bundle_create(std::ptr::null_mut(), cf_url);
        cf_release(cf_url);
        if bundle.is_null() {
            return Err(Vst3Error::LoadError("CFBundleCreate failed".into()));
        }

        // Load the bundle's executable using the preferred API.
        // This triggers dlopen internally, causing static constructors and +load
        // methods to run WITH the CFBundle already registered in the global table.
        let mut cf_error: *mut c_void = std::ptr::null_mut();
        let loaded = cf_bundle_load_executable(bundle, &mut cf_error);
        if loaded == 0 {
            // Release error if present, then fail
            if !cf_error.is_null() {
                cf_release(cf_error);
            }
            cf_release(bundle);
            return Err(Vst3Error::LoadError(
                "CFBundleLoadExecutableAndReturnError failed".into(),
            ));
        }
        tracing::debug!(
            path = %bundle_path.display(),
            "CFBundleLoadExecutableAndReturnError succeeded"
        );

        // Don't release the bundle — the plugin retains it for resource loading.
        Ok(bundle)
    }
}

/// Call `bundleEntry` on macOS using a pre-created CFBundleRef.
///
/// The bundle_ref must have been created by `prepare_macos_bundle` before
/// `Library::new` (dlopen) so that static constructors could find the bundle.
#[cfg(target_os = "macos")]
pub(super) unsafe fn call_bundle_entry(
    library: &libloading::Library,
    bundle_ref: *mut c_void,
) -> Result<bool, Vst3Error> {
    type BundleEntryProc = unsafe extern "C" fn(*mut c_void) -> bool;
    let entry: Result<libloading::Symbol<BundleEntryProc>, _> =
        unsafe { library.get(b"bundleEntry") };
    let entry = match entry {
        Ok(f) => f,
        Err(_) => return Ok(false), // not all plugins export bundleEntry
    };

    let result = unsafe { entry(bundle_ref) };
    // Don't release the bundle — the plugin retains it for resource loading.
    // It will be released when bundleExit is called.
    tracing::debug!(result, "bundleEntry");
    Ok(result)
}

#[cfg(target_os = "linux")]
pub(super) unsafe fn call_bundle_entry(
    library: &libloading::Library,
    library_path: &Path,
) -> Result<bool, Vst3Error> {
    type ModuleEntryProc = unsafe extern "C" fn(*mut c_void) -> bool;
    let entry: Result<libloading::Symbol<ModuleEntryProc>, _> =
        unsafe { library.get(b"ModuleEntry") };
    match entry {
        Ok(f) => {
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(library_path.as_os_str().as_bytes())
                .map_err(|_| Vst3Error::LoadError("library path contains NUL".into()))?;
            // ModuleEntry receives the dlopen handle, not the filename. Retain
            // an existing handle temporarily; the Library still owns its reference.
            let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD) };
            if handle.is_null() {
                return Err(Vst3Error::LoadError(
                    "loaded module handle unavailable".into(),
                ));
            }
            let initialized = unsafe { f(handle) };
            unsafe { libc::dlclose(handle) };
            Ok(initialized)
        }
        Err(_) => Ok(false),
    }
}

/// Call `bundleExit` / `ModuleExit` to clean up plugin-global resources.
pub(super) unsafe fn call_bundle_exit(library: &libloading::Library) {
    #[cfg(target_os = "macos")]
    {
        type BundleExitProc = unsafe extern "C" fn() -> bool;
        if let Ok(exit) = unsafe { library.get::<BundleExitProc>(b"bundleExit") } {
            unsafe { exit() };
        }
    }
    #[cfg(target_os = "linux")]
    {
        type ModuleExitProc = unsafe extern "C" fn() -> bool;
        if let Ok(exit) = unsafe { library.get::<ModuleExitProc>(b"ModuleExit") } {
            unsafe { exit() };
        }
    }
}

/// Load the VST3 factory and create IComponent + IAudioProcessor instances.
///
/// Finds the first "Audio Module Class" in the factory and instantiates it.
///
/// On macOS, `bundle_ref` is the CFBundleRef created by `prepare_macos_bundle`
/// (must be non-null). On other platforms it is ignored.
pub(super) unsafe fn create_instance(
    library: &libloading::Library,
    library_path: &Path,
    name: &str,
    bundle_ref: *mut c_void,
) -> Result<(*mut c_void, *mut c_void, *mut c_void), Vst3Error> {
    // Step 0: Call bundleEntry to initialize plugin-global resources (GUI, etc.)
    #[cfg(target_os = "macos")]
    unsafe {
        let _ = library_path; // only used on Linux
        call_bundle_entry(library, bundle_ref)?;
    }
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = bundle_ref; // only used on macOS
        call_bundle_entry(library, library_path)?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = (library_path, bundle_ref);

    // Step 1: Get the factory entry point
    type GetFactoryProc = unsafe extern "C" fn() -> *mut c_void;
    let get_factory: libloading::Symbol<GetFactoryProc> = unsafe {
        // SAFETY: The library is a valid VST3 plugin; GetPluginFactory is the
        // standard VST3 entry point exported by all conforming plugins.
        library
            .get(b"GetPluginFactory")
            .map_err(|_| Vst3Error::EntryPointNotFound)?
    };

    let factory = unsafe {
        // SAFETY: get_factory is a valid function pointer obtained from the library.
        get_factory()
    };
    if factory.is_null() {
        return Err(Vst3Error::FactoryFailed);
    }

    let vtbl = unsafe {
        // SAFETY: factory is non-null (checked above). COM objects begin with a
        // vtable pointer.
        *(factory as *const *const IPluginFactoryVtbl)
    };

    // Step 2: Find the first Audio Module Class
    let count = unsafe {
        // SAFETY: vtbl is valid, factory is a live COM object.
        ((*vtbl).count_classes)(factory)
    };
    let mut audio_class_cid: Option<[u8; 16]> = None;

    for i in 0..count {
        let mut info = PClassInfo::default();
        if unsafe {
            // SAFETY: vtbl and factory are valid; info is a stack-allocated output param.
            ((*vtbl).get_class_info)(factory, i, &mut info)
        } == K_RESULT_OK
        {
            let category = com::cstr_from_buf(&info.category);
            if category == "Audio Module Class" {
                audio_class_cid = Some(info.cid);
                let class_name = com::cstr_from_buf(&info.name);
                tracing::debug!(plugin = %name, class = %class_name, "found Audio Module Class");
                break;
            }
        }
    }

    let cid = match audio_class_cid {
        Some(cid) => cid,
        None => {
            unsafe {
                // SAFETY: factory is a live COM object; release decrements its ref count.
                ((*vtbl).base.release)(factory)
            };
            return Err(Vst3Error::ComponentFailed(
                "no Audio Module Class found".into(),
            ));
        }
    };

    // Step 3: Create IComponent instance
    let mut component: *mut c_void = std::ptr::null_mut();
    let result = unsafe {
        // SAFETY: factory is valid, cid is a valid class ID found above,
        // IID_ICOMPONENT is the correct interface ID. component is an output param.
        ((*vtbl).create_instance)(factory, &cid, &IID_ICOMPONENT, &mut component)
    };

    if result != K_RESULT_OK || component.is_null() {
        unsafe { ((*vtbl).base.release)(factory) };
        return Err(Vst3Error::ComponentFailed(format!(
            "createInstance returned {result} for {name}"
        )));
    }

    // Step 4: Query IAudioProcessor from IComponent
    let comp_vtbl = unsafe { *(component as *const *const IComponentVtbl) };
    let mut processor: *mut c_void = std::ptr::null_mut();
    let result = unsafe {
        // SAFETY: component is a valid IComponent (created above, result checked).
        // comp_vtbl is its vtable. processor is an output param.
        ((*comp_vtbl).query_interface)(component, &IID_IAUDIO_PROCESSOR, &mut processor)
    };

    if result != K_RESULT_OK || processor.is_null() {
        unsafe { ((*comp_vtbl).release)(component) };
        unsafe { ((*vtbl).base.release)(factory) };
        return Err(Vst3Error::ComponentFailed(format!(
            "queryInterface for IAudioProcessor failed for {name}"
        )));
    }

    Ok((component, processor, factory))
}
