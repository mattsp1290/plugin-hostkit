use super::*;

// ── IBStream / MemoryStream ─────────────────────────────────────────

/// IBStream vtable: FUnknown (3) + IBStream (4) = 7 function pointers.
#[repr(C)]
pub(crate) struct IBStreamVtbl {
    // FUnknown (3)
    pub query_interface:
        unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    // IBStream (4)
    pub read: unsafe extern "C" fn(
        this: *mut c_void,
        buffer: *mut c_void,
        num_bytes: i32,
        num_bytes_read: *mut i32,
    ) -> TResult,
    pub write: unsafe extern "C" fn(
        this: *mut c_void,
        buffer: *const c_void,
        num_bytes: i32,
        num_bytes_written: *mut i32,
    ) -> TResult,
    pub seek:
        unsafe extern "C" fn(this: *mut c_void, pos: i64, mode: i32, result: *mut i64) -> TResult,
    pub tell: unsafe extern "C" fn(this: *mut c_void, pos: *mut i64) -> TResult,
}

/// In-memory IBStream for state transfer between component and controller.
#[repr(C)]
pub(crate) struct MemoryStream {
    vtable: *const IBStreamVtbl,
    data: Vec<u8>,
    position: usize,
}

static MEMORY_STREAM_VTBL: IBStreamVtbl = IBStreamVtbl {
    query_interface: mem_stream_query_interface,
    add_ref: mem_stream_add_ref,
    release: mem_stream_release,
    read: mem_stream_read,
    write: mem_stream_write,
    seek: mem_stream_seek,
    tell: mem_stream_tell,
};

/// IBStream IID: {C3BF6EA2-3099-4752-9B6B-F9901EE33E9B}
const IID_IBSTREAM: TUID = [
    0xC3, 0xBF, 0x6E, 0xA2, 0x30, 0x99, 0x47, 0x52, 0x9B, 0x6B, 0xF9, 0x90, 0x1E, 0xE3, 0x3E, 0x9B,
];

unsafe extern "C" fn mem_stream_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        if *iid == IID_FUNKNOWN || *iid == IID_IBSTREAM {
            mem_stream_add_ref(this);
            *obj = this;
            return K_RESULT_OK;
        }
        *obj = std::ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "C" fn mem_stream_add_ref(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "C" fn mem_stream_release(_this: *mut c_void) -> u32 {
    1
}

unsafe fn stream_from_ptr(this: *mut c_void) -> *mut MemoryStream {
    this as *mut MemoryStream
}

unsafe extern "C" fn mem_stream_read(
    this: *mut c_void,
    buffer: *mut c_void,
    num_bytes: i32,
    num_bytes_read: *mut i32,
) -> TResult {
    unsafe {
        let s = &mut *stream_from_ptr(this);
        if num_bytes <= 0 {
            if !num_bytes_read.is_null() {
                *num_bytes_read = 0;
            }
            return K_RESULT_OK;
        }
        let available = s.data.len().saturating_sub(s.position);
        let to_read = (num_bytes as usize).min(available);
        if to_read > 0 {
            std::ptr::copy_nonoverlapping(
                s.data.as_ptr().add(s.position),
                buffer as *mut u8,
                to_read,
            );
            s.position += to_read;
        }
        if !num_bytes_read.is_null() {
            *num_bytes_read = to_read as i32;
        }
        // Steinberg's reference MemoryStream always returns kResultTrue from
        // read(), even when fewer bytes are available than requested. The caller
        // checks *num_bytes_read for the actual count. Returning kResultFalse
        // here caused some plugins  to treat partial reads as
        // errors, leaving internal state uninitialized.
        K_RESULT_OK
    }
}

unsafe extern "C" fn mem_stream_write(
    this: *mut c_void,
    buffer: *const c_void,
    num_bytes: i32,
    num_bytes_written: *mut i32,
) -> TResult {
    unsafe {
        let s = &mut *stream_from_ptr(this);
        if num_bytes <= 0 {
            if !num_bytes_written.is_null() {
                *num_bytes_written = 0;
            }
            return K_RESULT_OK;
        }
        let count = num_bytes as usize;
        // Extend or overwrite
        let end = s.position + count;
        if end > s.data.len() {
            s.data.resize(end, 0);
        }
        std::ptr::copy_nonoverlapping(
            buffer as *const u8,
            s.data.as_mut_ptr().add(s.position),
            count,
        );
        s.position += count;
        if !num_bytes_written.is_null() {
            *num_bytes_written = count as i32;
        }
        K_RESULT_OK
    }
}

unsafe extern "C" fn mem_stream_seek(
    this: *mut c_void,
    pos: i64,
    mode: i32,
    result: *mut i64,
) -> TResult {
    unsafe {
        let s = &mut *stream_from_ptr(this);
        let new_pos: i64 = match mode {
            K_IB_SEEK_SET => pos,
            K_IB_SEEK_CUR => s.position as i64 + pos,
            K_IB_SEEK_END => s.data.len() as i64 + pos,
            _ => return K_RESULT_FALSE,
        };
        s.position = new_pos.max(0) as usize;
        if !result.is_null() {
            *result = s.position as i64;
        }
        K_RESULT_OK
    }
}

unsafe extern "C" fn mem_stream_tell(this: *mut c_void, pos: *mut i64) -> TResult {
    unsafe {
        let s = &mut *stream_from_ptr(this);
        if !pos.is_null() {
            *pos = s.position as i64;
        }
        K_RESULT_OK
    }
}

impl MemoryStream {
    pub fn new() -> Self {
        Self {
            vtable: &MEMORY_STREAM_VTBL,
            data: Vec::new(),
            position: 0,
        }
    }

    /// Create a memory stream pre-loaded with the given bytes.
    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self {
            vtable: &MEMORY_STREAM_VTBL,
            data,
            position: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn reset_position(&mut self) {
        self.position = 0;
    }

    /// Get the COM interface pointer. The object must not be moved
    /// while this pointer is in use (it points to `self`).
    pub fn as_ptr(&mut self) -> *mut c_void {
        self as *mut Self as *mut c_void
    }

    /// Consume the stream and return its data bytes.
    pub fn into_vec(self) -> Vec<u8> {
        self.data
    }
}

/// Seek modes matching the VST3 IBStream spec.
const K_IB_SEEK_SET: i32 = 0;
const K_IB_SEEK_CUR: i32 = 1;
const K_IB_SEEK_END: i32 = 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_stream_read_full() {
        let data = vec![10u8, 20, 30, 40, 50];
        let mut stream = MemoryStream::from_bytes(data.clone());
        let ptr = stream.as_ptr();

        let mut buf = [0u8; 5];
        let mut bytes_read: i32 = 0;
        unsafe {
            let result = mem_stream_read(ptr, buf.as_mut_ptr() as *mut c_void, 5, &mut bytes_read);
            assert_eq!(result, K_RESULT_OK);
            assert_eq!(bytes_read, 5);
            assert_eq!(buf, [10, 20, 30, 40, 50]);
        }
    }

    #[test]
    fn memory_stream_read_partial() {
        let data = vec![1u8, 2, 3];
        let mut stream = MemoryStream::from_bytes(data);
        let ptr = stream.as_ptr();

        let mut buf = [0u8; 2];
        let mut bytes_read: i32 = 0;
        unsafe {
            mem_stream_read(ptr, buf.as_mut_ptr() as *mut c_void, 2, &mut bytes_read);
            assert_eq!(bytes_read, 2);
            assert_eq!(buf, [1, 2]);

            // Read remaining
            mem_stream_read(ptr, buf.as_mut_ptr() as *mut c_void, 2, &mut bytes_read);
            assert_eq!(bytes_read, 1);
            assert_eq!(buf[0], 3);

            // Read past end
            mem_stream_read(ptr, buf.as_mut_ptr() as *mut c_void, 2, &mut bytes_read);
            assert_eq!(bytes_read, 0);
        }
    }

    #[test]
    fn memory_stream_seek_and_tell() {
        let data = vec![0u8; 100];
        let mut stream = MemoryStream::from_bytes(data);
        let ptr = stream.as_ptr();

        unsafe {
            let mut pos: i64 = 0;

            // Seek from start
            assert_eq!(
                mem_stream_seek(ptr, 50, K_IB_SEEK_SET, &mut pos),
                K_RESULT_OK
            );
            assert_eq!(pos, 50);

            // Tell
            mem_stream_tell(ptr, &mut pos);
            assert_eq!(pos, 50);

            // Seek from current
            assert_eq!(
                mem_stream_seek(ptr, 10, K_IB_SEEK_CUR, &mut pos),
                K_RESULT_OK
            );
            assert_eq!(pos, 60);

            // Seek from end
            assert_eq!(
                mem_stream_seek(ptr, -20, K_IB_SEEK_END, &mut pos),
                K_RESULT_OK
            );
            assert_eq!(pos, 80);

            // Seek past end — clamps to end
            assert_eq!(
                mem_stream_seek(ptr, 1, K_IB_SEEK_END, &mut pos),
                K_RESULT_OK
            );
            assert_eq!(pos, 101);

            // Seek before start — clamps to 0
            assert_eq!(
                mem_stream_seek(ptr, -1, K_IB_SEEK_SET, &mut pos),
                K_RESULT_OK
            );
            assert_eq!(pos, 0);
        }
    }

    #[test]
    fn memory_stream_write() {
        let mut stream = MemoryStream::new();
        let ptr = stream.as_ptr();

        let data = [1u8; 5];
        let mut bytes_written: i32 = 0;
        unsafe {
            let result =
                mem_stream_write(ptr, data.as_ptr() as *const c_void, 5, &mut bytes_written);
            assert_eq!(result, K_RESULT_OK);
            assert_eq!(bytes_written, 5);
        }
    }
}
