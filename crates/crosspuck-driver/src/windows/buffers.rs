use std::ffi::c_void;
use std::ptr;
use std::slice;

pub const TRUE_U8: u8 = 1;
pub const FALSE_U8: u8 = 0;

pub unsafe fn write_wide_string(buffer: *mut c_void, buffer_len: u32, value: &str) -> u8 {
    if buffer.is_null() || buffer_len < 2 {
        return FALSE_U8;
    }

    let slots = buffer_len as usize / 2;
    let out = slice::from_raw_parts_mut(buffer as *mut u16, slots);
    let mut written = 0;
    for unit in value.encode_utf16().take(slots.saturating_sub(1)) {
        out[written] = unit;
        written += 1;
    }
    out[written] = 0;
    TRUE_U8
}

pub unsafe fn zero_buffer(buffer: *mut c_void, buffer_len: u32) {
    if !buffer.is_null() && buffer_len > 0 {
        ptr::write_bytes(buffer, 0, buffer_len as usize);
    }
}

pub unsafe fn report_id_from_buffer(buffer: *const c_void, buffer_len: u32) -> u8 {
    if buffer.is_null() || buffer_len == 0 {
        0
    } else {
        *(buffer as *const u8)
    }
}

pub unsafe fn input_slice<'a>(buffer: *const c_void, buffer_len: u32) -> Option<&'a [u8]> {
    if buffer_len == 0 {
        return Some(&[]);
    }
    if buffer.is_null() {
        return None;
    }
    Some(slice::from_raw_parts(
        buffer as *const u8,
        buffer_len as usize,
    ))
}

pub unsafe fn output_slice<'a>(buffer: *mut c_void, buffer_len: u32) -> Option<&'a mut [u8]> {
    if buffer_len == 0 {
        return Some(&mut []);
    }
    if buffer.is_null() {
        return None;
    }
    Some(slice::from_raw_parts_mut(
        buffer as *mut u8,
        buffer_len as usize,
    ))
}
