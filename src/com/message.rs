use super::*;

/// IMessage IID: {936F033B-C6C0-47DB-BB08-82F813C1E613}
pub(super) const IID_IMESSAGE: TUID = [
    0x93, 0x6F, 0x03, 0x3B, 0xC6, 0xC0, 0x47, 0xDB, 0xBB, 0x08, 0x82, 0xF8, 0x13, 0xC1, 0xE6, 0x13,
];

// ── IMessage minimal implementation ─────────────────────────────────

/// IMessage vtable: 3 FUnknown + 3 IMessage methods.
#[repr(C)]
struct IMessageVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    get_message_id: unsafe extern "C" fn(*mut c_void) -> *const u8,
    set_message_id: unsafe extern "C" fn(*mut c_void, id: *const u8),
    get_attributes: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
}

/// Heap-allocated IMessage implementation.
#[repr(C)]
pub(super) struct MessageObj {
    vtable: *const IMessageVtbl,
    ref_count: std::sync::atomic::AtomicU32,
    message_id: Vec<u8>,
    attributes: *mut c_void, // AttributeListObj pointer
}

static MESSAGE_VTBL: IMessageVtbl = IMessageVtbl {
    query_interface: msg_query_interface,
    add_ref: msg_add_ref,
    release: msg_release,
    get_message_id: msg_get_message_id,
    set_message_id: msg_set_message_id,
    get_attributes: msg_get_attributes,
};

impl MessageObj {
    /// Create a new heap-allocated message and return as raw pointer.
    pub(super) fn new_boxed() -> *mut c_void {
        let attr = AttributeListObj::new_boxed();
        let msg = Box::new(Self {
            vtable: &MESSAGE_VTBL,
            ref_count: std::sync::atomic::AtomicU32::new(1),
            message_id: vec![0],
            attributes: attr,
        });
        Box::into_raw(msg) as *mut c_void
    }
}

unsafe extern "C" fn msg_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        if *iid == IID_IMESSAGE || *iid == IID_FUNKNOWN {
            msg_add_ref(this);
            *obj = this;
            return K_RESULT_OK;
        }
        *obj = std::ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "C" fn msg_add_ref(this: *mut c_void) -> u32 {
    unsafe {
        let obj = &*(this as *const MessageObj);
        obj.ref_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }
}

unsafe extern "C" fn msg_release(this: *mut c_void) -> u32 {
    unsafe {
        let obj = &*(this as *const MessageObj);
        let prev = obj
            .ref_count
            .fetch_sub(1, std::sync::atomic::Ordering::Release);
        if prev == 1 {
            std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
            let boxed = Box::from_raw(this as *mut MessageObj);
            if !boxed.attributes.is_null() {
                attr_release(boxed.attributes);
            }
            // boxed drops here
            return 0;
        }
        prev - 1
    }
}

unsafe extern "C" fn msg_get_message_id(this: *mut c_void) -> *const u8 {
    unsafe { (*(this as *const MessageObj)).message_id.as_ptr() }
}

unsafe extern "C" fn msg_set_message_id(this: *mut c_void, id: *const u8) {
    unsafe {
        let obj = &mut *(this as *mut MessageObj);
        let mut key = read_cstr_key(id);
        key.push(0); // IMessage stores the null terminator
        obj.message_id = key;
    }
}

/// Returns a borrowed (non-retained) pointer to the message's attribute list.
/// Per VST3 convention, IMessage::getAttributes() returns a raw pointer that
/// the caller may use within the same scope but must NOT retain (no addRef).
/// The attribute list's lifetime is tied to the MessageObj — it is freed when
/// the message's ref count hits zero (msg_release).
unsafe extern "C" fn msg_get_attributes(this: *mut c_void) -> *mut c_void {
    unsafe { (*(this as *const MessageObj)).attributes }
}

// ── IAttributeList minimal implementation ────────────────────────────

/// IAttributeList IID: {1E5F0AEB-CC7F-4533-A254-401138AD5EE4}
pub(super) const IID_IATTRIBUTE_LIST: TUID = [
    0x1E, 0x5F, 0x0A, 0xEB, 0xCC, 0x7F, 0x45, 0x33, 0xA2, 0x54, 0x40, 0x11, 0x38, 0xAD, 0x5E, 0xE4,
];

/// IAttributeList vtable: 3 FUnknown + 8 IAttributeList methods.
#[repr(C)]
struct IAttributeListVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    set_int: unsafe extern "C" fn(*mut c_void, id: *const u8, value: i64) -> TResult,
    get_int: unsafe extern "C" fn(*mut c_void, id: *const u8, value: *mut i64) -> TResult,
    set_float: unsafe extern "C" fn(*mut c_void, id: *const u8, value: f64) -> TResult,
    get_float: unsafe extern "C" fn(*mut c_void, id: *const u8, value: *mut f64) -> TResult,
    set_string: unsafe extern "C" fn(*mut c_void, id: *const u8, string: *const u16) -> TResult,
    get_string:
        unsafe extern "C" fn(*mut c_void, id: *const u8, string: *mut u16, size: u32) -> TResult,
    set_binary:
        unsafe extern "C" fn(*mut c_void, id: *const u8, data: *const c_void, size: u32) -> TResult,
    get_binary: unsafe extern "C" fn(
        *mut c_void,
        id: *const u8,
        data: *mut *const c_void,
        size: *mut u32,
    ) -> TResult,
}

/// Attribute value types for IAttributeList storage.
enum AttributeValue {
    Int(i64),
    Float(f64),
    String(Vec<u16>),
    Binary(Vec<u8>),
}

#[repr(C)]
pub(super) struct AttributeListObj {
    vtable: *const IAttributeListVtbl,
    ref_count: std::sync::atomic::AtomicU32,
    data: HashMap<Vec<u8>, AttributeValue>,
}

static ATTRIBUTE_LIST_VTBL: IAttributeListVtbl = IAttributeListVtbl {
    query_interface: attr_query_interface,
    add_ref: attr_add_ref,
    release: attr_release,
    set_int: attr_set_int,
    get_int: attr_get_int,
    set_float: attr_set_float,
    get_float: attr_get_float,
    set_string: attr_set_string,
    get_string: attr_get_string,
    set_binary: attr_set_binary,
    get_binary: attr_get_binary,
};

impl AttributeListObj {
    pub(super) fn new_boxed() -> *mut c_void {
        let obj = Box::new(Self {
            vtable: &ATTRIBUTE_LIST_VTBL,
            ref_count: std::sync::atomic::AtomicU32::new(1),
            data: HashMap::new(),
        });
        Box::into_raw(obj) as *mut c_void
    }
}

unsafe extern "C" fn attr_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        if *iid == IID_IATTRIBUTE_LIST || *iid == IID_FUNKNOWN {
            attr_add_ref(this);
            *obj = this;
            return K_RESULT_OK;
        }
        *obj = std::ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "C" fn attr_add_ref(this: *mut c_void) -> u32 {
    unsafe {
        let obj = &*(this as *const AttributeListObj);
        obj.ref_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }
}

unsafe extern "C" fn attr_release(this: *mut c_void) -> u32 {
    unsafe {
        let obj = &*(this as *const AttributeListObj);
        let prev = obj
            .ref_count
            .fetch_sub(1, std::sync::atomic::Ordering::Release);
        if prev == 1 {
            std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
            drop(Box::from_raw(this as *mut AttributeListObj));
            return 0;
        }
        prev - 1
    }
}

/// Read a null-terminated C string into a Vec<u8> key (without the null terminator).
unsafe fn read_cstr_key(id: *const u8) -> Vec<u8> {
    unsafe {
        let mut len = 0;
        // SAFETY: VST3 contract guarantees null-terminated C strings. The 4096-byte
        // cap is a safety net to prevent runaway reads from buggy plugins.
        while len < 4096 && *id.add(len) != 0 {
            len += 1;
        }
        std::slice::from_raw_parts(id, len).to_vec()
    }
}

unsafe extern "C" fn attr_set_int(this: *mut c_void, id: *const u8, value: i64) -> TResult {
    unsafe {
        let obj = &mut *(this as *mut AttributeListObj);
        let key = read_cstr_key(id);
        obj.data.insert(key, AttributeValue::Int(value));
        K_RESULT_OK
    }
}
unsafe extern "C" fn attr_get_int(this: *mut c_void, id: *const u8, value: *mut i64) -> TResult {
    unsafe {
        let obj = &*(this as *const AttributeListObj);
        let key = read_cstr_key(id);
        match obj.data.get(&key) {
            Some(AttributeValue::Int(v)) => {
                *value = *v;
                K_RESULT_OK
            }
            _ => K_RESULT_FALSE,
        }
    }
}
unsafe extern "C" fn attr_set_float(this: *mut c_void, id: *const u8, value: f64) -> TResult {
    unsafe {
        let obj = &mut *(this as *mut AttributeListObj);
        let key = read_cstr_key(id);
        obj.data.insert(key, AttributeValue::Float(value));
        K_RESULT_OK
    }
}
unsafe extern "C" fn attr_get_float(this: *mut c_void, id: *const u8, value: *mut f64) -> TResult {
    unsafe {
        let obj = &*(this as *const AttributeListObj);
        let key = read_cstr_key(id);
        match obj.data.get(&key) {
            Some(AttributeValue::Float(v)) => {
                *value = *v;
                K_RESULT_OK
            }
            _ => K_RESULT_FALSE,
        }
    }
}
unsafe extern "C" fn attr_set_string(
    this: *mut c_void,
    id: *const u8,
    string: *const u16,
) -> TResult {
    unsafe {
        let obj = &mut *(this as *mut AttributeListObj);
        let key = read_cstr_key(id);
        let mut len = 0;
        // SAFETY: VST3 contract guarantees null-terminated UTF-16 strings. The
        // 65536-char cap is a safety net to prevent runaway reads from buggy plugins.
        while len < 65536 && *string.add(len) != 0 {
            len += 1;
        }
        let chars = std::slice::from_raw_parts(string, len + 1).to_vec(); // include null
        obj.data.insert(key, AttributeValue::String(chars));
        K_RESULT_OK
    }
}
unsafe extern "C" fn attr_get_string(
    this: *mut c_void,
    id: *const u8,
    string: *mut u16,
    size: u32,
) -> TResult {
    unsafe {
        let obj = &*(this as *const AttributeListObj);
        let key = read_cstr_key(id);
        match obj.data.get(&key) {
            Some(AttributeValue::String(chars)) => {
                let capacity_chars = size as usize / std::mem::size_of::<u16>();
                if capacity_chars == 0 {
                    return K_RESULT_FALSE;
                }
                let copy_len = capacity_chars.min(chars.len());
                std::ptr::copy_nonoverlapping(chars.as_ptr(), string, copy_len);
                // Always null-terminate within buffer bounds
                if capacity_chars > 0 && copy_len < capacity_chars {
                    *string.add(copy_len) = 0;
                } else if capacity_chars > 0 {
                    *string.add(capacity_chars - 1) = 0;
                }
                K_RESULT_OK
            }
            _ => K_RESULT_FALSE,
        }
    }
}
unsafe extern "C" fn attr_set_binary(
    this: *mut c_void,
    id: *const u8,
    data: *const c_void,
    size: u32,
) -> TResult {
    unsafe {
        let obj = &mut *(this as *mut AttributeListObj);
        let key = read_cstr_key(id);
        let bytes = std::slice::from_raw_parts(data as *const u8, size as usize).to_vec();
        obj.data.insert(key, AttributeValue::Binary(bytes));
        K_RESULT_OK
    }
}
unsafe extern "C" fn attr_get_binary(
    this: *mut c_void,
    id: *const u8,
    data: *mut *const c_void,
    size: *mut u32,
) -> TResult {
    unsafe {
        let obj = &*(this as *const AttributeListObj);
        let key = read_cstr_key(id);
        match obj.data.get(&key) {
            Some(AttributeValue::Binary(bytes)) => {
                *data = bytes.as_ptr() as *const c_void;
                *size = bytes.len() as u32;
                K_RESULT_OK
            }
            _ => K_RESULT_FALSE,
        }
    }
}
