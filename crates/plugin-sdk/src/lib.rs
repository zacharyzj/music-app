use music_core::{MAX_MESSAGE, Request, SourceError, SourceResult};
use serde_json::Value;
#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "music_host_v1")]
unsafe extern "C" {
    fn request(ptr: u32, len: u32) -> i32;
    fn response(ptr: u32, len: u32) -> i32;
}
pub fn host(value: Value) -> SourceResult {
    #[cfg(target_arch = "wasm32")]
    {
        let bytes = serde_json::to_vec(&value).unwrap();
        let len = unsafe { request(bytes.as_ptr() as u32, bytes.len() as u32) };
        if len < 0 || len as usize > MAX_MESSAGE {
            return Err(SourceError::new("host", "宿主请求失败"));
        }
        let mut output = vec![0u8; len as usize];
        if unsafe { response(output.as_mut_ptr() as u32, len as u32) } != len {
            return Err(SourceError::new("host", "宿主响应失败"));
        }
        serde_json::from_slice(&output).map_err(|e| SourceError::new("protocol", e.to_string()))?
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = value;
        Err(SourceError::new(
            "unsupported",
            "宿主能力仅可在 WASM 内调用",
        ))
    }
}
// All ABI buffers are boxed slices; callers must free with the exact original length.
#[unsafe(no_mangle)]
pub extern "C" fn music_alloc(len: u32) -> u32 {
    if len as usize > MAX_MESSAGE {
        return 0;
    }
    Box::into_raw(vec![0u8; len as usize].into_boxed_slice()) as *mut u8 as usize as u32
}
/// # Safety
/// `ptr` must be a buffer returned by music_alloc or dispatch, with its original length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn music_free(ptr: u32, len: u32) {
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            ptr as usize as *mut u8,
            len as usize,
        )));
    }
}
/// # Safety
/// The input must refer to a readable guest allocation of `len` bytes.
pub unsafe fn dispatch(ptr: u32, len: u32, handler: fn(Request) -> SourceResult) -> u64 {
    let result = if len as usize > MAX_MESSAGE {
        Err(SourceError::new("protocol", "请求过大"))
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(ptr as usize as *const u8, len as usize) };
        serde_json::from_slice(bytes)
            .map_err(|e| SourceError::new("protocol", e.to_string()))
            .and_then(handler)
    };
    let output = serde_json::to_vec(&result).unwrap().into_boxed_slice();
    let len = output.len() as u64;
    let ptr = Box::into_raw(output) as *mut u8 as usize as u64;
    (len << 32) | ptr
}
