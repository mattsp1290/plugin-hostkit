use super::*;

// ── IConnectionPoint proxy ──────────────────────────────────────────
//
// Instead of connecting component ↔ controller IConnectionPoints directly,
// the host inserts proxies that forward notify() calls. This provides:
// 1. Clean lifetime management — disconnecting the proxy on terminate
// 2. Logging/tracing of cross-component messages
// 3. A future extension point for thread marshaling if needed
//
// Host-owned connection proxies forward messages between peers.

/// IConnectionPoint IID: {70A4156F-6E6E-4026-9891-48BFAA60D8D1}
/// Source: pluginterfaces/vst/ivstmessage.h
pub(crate) const IID_ICONNECTION_POINT: TUID = [
    0x70, 0xA4, 0x15, 0x6F, 0x6E, 0x6E, 0x40, 0x26, 0x98, 0x91, 0x48, 0xBF, 0xAA, 0x60, 0xD8, 0xD1,
];

/// IConnectionPoint vtable: 3 FUnknown + 3 methods.
#[repr(C)]
struct IConnectionPointProxyVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    connect: unsafe extern "C" fn(*mut c_void, other: *mut c_void) -> TResult,
    disconnect: unsafe extern "C" fn(*mut c_void, other: *mut c_void) -> TResult,
    notify: unsafe extern "C" fn(*mut c_void, message: *mut c_void) -> TResult,
}

/// Host-side IConnectionPoint proxy.
///
/// Wraps a real IConnectionPoint (from the plugin's component or controller)
/// and forwards notify() calls. The proxy is heap-allocated and ref-counted.
#[repr(C)]
pub(crate) struct ConnectionProxyObj {
    vtable: *const IConnectionPointProxyVtbl,
    ref_count: std::sync::atomic::AtomicU32,
    /// The real IConnectionPoint target that receives forwarded messages.
    /// Set via connect(), cleared via disconnect(). Not synchronized —
    /// all IConnectionPoint methods must be called from the same thread
    /// (guaranteed by VST3 spec: main thread only).
    target: *mut c_void,
    /// The parent COM object (component or controller) for QI forwarding.
    /// Some plugins query their
    /// IConnectionPoint peer for custom interfaces to obtain engine
    /// pointers or shared state. Since the IConnectionPoint may be a
    /// tearoff object that doesn't forward QIs to the parent, we store
    /// the parent explicitly and forward QIs there.
    qi_parent: *mut c_void,
    /// Label for tracing (e.g., "comp→ctrl" or "ctrl→comp").
    label: &'static str,
}

static CONNECTION_PROXY_VTBL: IConnectionPointProxyVtbl = IConnectionPointProxyVtbl {
    query_interface: cp_proxy_qi,
    add_ref: cp_proxy_add_ref,
    release: cp_proxy_release,
    connect: cp_proxy_connect,
    disconnect: cp_proxy_disconnect,
    notify: cp_proxy_notify,
};

impl ConnectionProxyObj {
    /// Create a new heap-allocated proxy and return as raw COM pointer.
    ///
    /// `qi_parent` is the component or controller COM pointer used to
    /// forward unknown queryInterface calls. This enables plugins that
    /// query custom interfaces from their IConnectionPoint peer.
    pub(crate) fn new_boxed(label: &'static str, qi_parent: *mut c_void) -> *mut c_void {
        // AddRef the parent so the proxy holds a strong reference.
        if !qi_parent.is_null() {
            unsafe {
                let vtbl = *(qi_parent as *const *const FUnknownVtbl);
                ((*vtbl).add_ref)(qi_parent);
            }
        }
        let obj = Box::new(Self {
            vtable: &CONNECTION_PROXY_VTBL,
            ref_count: std::sync::atomic::AtomicU32::new(1),
            target: std::ptr::null_mut(),
            qi_parent,
            label,
        });
        Box::into_raw(obj) as *mut c_void
    }

    /// Clear the forwarding target to prevent dangling notify() calls.
    /// Releases the old target if non-null. Used during terminate cleanup.
    pub(crate) unsafe fn clear_target(ptr: *mut c_void) {
        unsafe {
            let obj = &mut *(ptr as *mut Self);
            let old = obj.target;
            obj.target = std::ptr::null_mut();
            if !old.is_null() {
                let vtbl = *(old as *const *const FUnknownVtbl);
                ((*vtbl).release)(old);
            }
        }
    }
}

unsafe extern "C" fn cp_proxy_qi(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        let proxy = &*(this as *const ConnectionProxyObj);
        if *iid == IID_ICONNECTION_POINT || *iid == IID_FUNKNOWN {
            cp_proxy_add_ref(this);
            *obj = this;
            return K_RESULT_OK;
        }
        // Forward unknown QIs to the parent COM object (component or controller).
        // Some plugins query their
        // IConnectionPoint peer for custom interfaces to obtain engine
        // pointers or shared state. The IConnectionPoint returned by QI
        // may be a tearoff that doesn't forward QIs to the parent, so
        // we forward directly to the parent object.
        if !proxy.qi_parent.is_null() {
            let parent_vtbl = *(proxy.qi_parent as *const *const FUnknownVtbl);
            let result = ((*parent_vtbl).query_interface)(proxy.qi_parent, iid, obj);
            if result == K_RESULT_OK {
                tracing::debug!(
                    label = proxy.label,
                    iid = ?&iid.cast::<[u8; 16]>().read(),
                    "IConnectionPoint QI forwarded to parent"
                );
                return result;
            }
        }
        tracing::debug!(
            label = proxy.label,
            iid = ?&iid.cast::<[u8; 16]>().read(),
            "IConnectionPoint QI → kNoInterface"
        );
        *obj = std::ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "C" fn cp_proxy_add_ref(this: *mut c_void) -> u32 {
    unsafe {
        let obj = &*(this as *const ConnectionProxyObj);
        obj.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }
}

unsafe extern "C" fn cp_proxy_release(this: *mut c_void) -> u32 {
    unsafe {
        let obj = &*(this as *const ConnectionProxyObj);
        let prev = obj.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            std::sync::atomic::fence(Ordering::Acquire);
            // Release the qi_parent reference before dropping.
            let obj_mut = &mut *(this as *mut ConnectionProxyObj);
            if !obj_mut.qi_parent.is_null() {
                let vtbl = *(obj_mut.qi_parent as *const *const FUnknownVtbl);
                ((*vtbl).release)(obj_mut.qi_parent);
                obj_mut.qi_parent = std::ptr::null_mut();
            }
            drop(Box::from_raw(this as *mut ConnectionProxyObj));
            return 0;
        }
        prev - 1
    }
}

unsafe extern "C" fn cp_proxy_connect(this: *mut c_void, other: *mut c_void) -> TResult {
    unsafe {
        let obj = &mut *(this as *mut ConnectionProxyObj);
        tracing::debug!(label = obj.label, peer = ?other, "IConnectionPoint connect");
        // addRef the new target (COM ownership: we retain the pointer).
        if !other.is_null() {
            let vtbl = *(other as *const *const FUnknownVtbl);
            ((*vtbl).add_ref)(other);
        }
        obj.target = other;
        tracing::trace!(label = obj.label, "ConnectionProxy::connect");
        K_RESULT_OK
    }
}

unsafe extern "C" fn cp_proxy_disconnect(this: *mut c_void, _other: *mut c_void) -> TResult {
    unsafe {
        let obj = &mut *(this as *mut ConnectionProxyObj);
        tracing::debug!(label = obj.label, "IConnectionPoint disconnect");
        // Release the old target and clear. Proxy has at most one connection.
        let old = obj.target;
        obj.target = std::ptr::null_mut();
        if !old.is_null() {
            let vtbl = *(old as *const *const FUnknownVtbl);
            ((*vtbl).release)(old);
        }
        tracing::trace!(label = obj.label, "ConnectionProxy::disconnect");
        K_RESULT_OK
    }
}

unsafe extern "C" fn cp_proxy_notify(this: *mut c_void, message: *mut c_void) -> TResult {
    unsafe {
        let obj = &*(this as *const ConnectionProxyObj);
        tracing::debug!(label = obj.label, message = ?message, "IConnectionPoint notify");
        if obj.target.is_null() {
            tracing::debug!(
                label = obj.label,
                "ConnectionProxy::notify — target is null, dropping message"
            );
            return K_RESULT_FALSE;
        }
        // Forward the message to the real target's notify().
        // The target is an IConnectionPoint — its vtable has notify at slot 5.
        let target_vtbl = *(obj.target as *const *const IConnectionPointProxyVtbl);
        let result = ((*target_vtbl).notify)(obj.target, message);
        tracing::debug!(
            label = obj.label,
            result,
            "ConnectionProxy::notify forwarded"
        );
        result
    }
}
