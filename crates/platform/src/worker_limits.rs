//! Resource controls for native macOS helper processes.
use std::{
    ffi::{CStr, c_char},
    ptr,
};

pub(crate) fn resident_bytes(pid: u32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
    // SAFETY: proc_pidinfo initializes this exact structure on a full-size result.
    let count = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if count == size {
        Some(unsafe { info.assume_init() }.pti_resident_size)
    } else {
        None
    }
}

pub(crate) fn restrict_worker(profile: &CStr) -> Result<(), String> {
    unsafe extern "C" {
        fn sandbox_init(profile: *const c_char, flags: u64, error: *mut *mut c_char) -> i32;
        fn sandbox_free_error(error: *mut c_char);
    }
    let mut error = ptr::null_mut();
    // SAFETY: constant NUL-terminated profile; error is allocated by sandbox_init.
    let result = unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) };
    if result != 0 {
        let message = if error.is_null() {
            "Cannot restrict renderer process".into()
        } else {
            unsafe { std::ffi::CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        };
        unsafe {
            sandbox_free_error(error);
        }
        return Err(message);
    }
    // SAFETY: process-local priority and core-dump resource limits.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
        libc::setrlimit(
            libc::RLIMIT_CORE,
            &libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            },
        );
    }
    Ok(())
}
