use std::ffi::c_void;
use std::mem;

pub unsafe fn fn_from_const<T>(ptr: *const c_void) -> T {
    assert_eq!(mem::size_of::<T>(), mem::size_of::<*const c_void>());
    mem::transmute_copy::<*const c_void, T>(&ptr)
}

pub unsafe fn fn_from_mut<T>(ptr: *mut c_void) -> T {
    assert_eq!(mem::size_of::<T>(), mem::size_of::<*mut c_void>());
    mem::transmute_copy::<*mut c_void, T>(&ptr)
}
