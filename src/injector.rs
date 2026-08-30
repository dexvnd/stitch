use std::ffi::c_void;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{
    VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
};
use windows::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, OpenProcess, WaitForSingleObject, INFINITE,
    PROCESS_ALL_ACCESS,
};
use windows::core::{s, w};

#[derive(Debug)]
pub enum InjectError {
    OpenProcess(windows::core::Error),
    AllocFailed,
    WriteFailed(windows::core::Error),
    NoKernel32,
    NoLoadLibraryW,
    CreateThreadFailed(windows::core::Error),
    RemoteLoadFailed,
}

impl std::fmt::Display for InjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InjectError::OpenProcess(e) => write!(f, "OpenProcess failed: {e}"),
            InjectError::AllocFailed => write!(f, "VirtualAllocEx failed"),
            InjectError::WriteFailed(e) => write!(f, "WriteProcessMemory failed: {e}"),
            InjectError::NoKernel32 => write!(f, "could not get kernel32.dll handle"),
            InjectError::NoLoadLibraryW => write!(f, "could not resolve LoadLibraryW"),
            InjectError::CreateThreadFailed(e) => write!(f, "CreateRemoteThread failed: {e}"),
            InjectError::RemoteLoadFailed => write!(
                f,
                "LoadLibraryW returned NULL in the target process (likely a 32/64-bit \
                 mismatch between stitch.exe and the target, or a missing dependency of \
                 the embedded stub DLL)"
            ),
        }
    }
}

static STUB_DLL_BYTES: &[u8] = include_bytes!(env!("STITCH_STUB_DLL_PATH"));

pub fn extract_stub_dll() -> std::io::Result<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "stitch_stub_{}_{}.dll",
        std::process::id(),
        unique
    ));
    std::fs::write(&path, STUB_DLL_BYTES)?;
    Ok(path)
}

pub fn inject(pid: u32, dll_path: &Path) -> Result<(), InjectError> {
    let process =
        unsafe { OpenProcess(PROCESS_ALL_ACCESS, false, pid) }.map_err(InjectError::OpenProcess)?;
    let guard = ProcessGuard(process);

    let mut wide_path: Vec<u16> = dll_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let byte_len = wide_path.len() * std::mem::size_of::<u16>();

    let remote_mem = unsafe {
        VirtualAllocEx(
            guard.0,
            None,
            byte_len,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    if remote_mem.is_null() {
        return Err(InjectError::AllocFailed);
    }
    let alloc_guard = AllocGuard { process: guard.0, addr: remote_mem };

    unsafe {
        WriteProcessMemory(
            guard.0,
            remote_mem,
            wide_path.as_mut_ptr() as *const c_void,
            byte_len,
            None,
        )
    }
    .map_err(InjectError::WriteFailed)?;

    let kernel32 = unsafe { GetModuleHandleW(w!("kernel32.dll")) }.map_err(|_| InjectError::NoKernel32)?;
    let load_library = unsafe { GetProcAddress(kernel32, s!("LoadLibraryW")) }
        .ok_or(InjectError::NoLoadLibraryW)?;
    let load_library: unsafe extern "system" fn(*mut c_void) -> u32 =
        unsafe { std::mem::transmute(load_library) };

    let thread = unsafe {
        CreateRemoteThread(
            guard.0,
            None,
            0,
            Some(load_library),
            Some(remote_mem),
            0,
            None,
        )
    }
    .map_err(InjectError::CreateThreadFailed)?;

    let exit_code = unsafe {
        WaitForSingleObject(thread, INFINITE);
        let mut code = 0u32;
        let got_code = GetExitCodeThread(thread, &mut code).is_ok();
        let _ = CloseHandle(thread);
        got_code.then_some(code)
    };

    drop(alloc_guard);

    if exit_code == Some(0) {
        return Err(InjectError::RemoteLoadFailed);
    }

    Ok(())
}

use std::os::windows::ffi::OsStrExt;

struct ProcessGuard(HANDLE);
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct AllocGuard {
    process: HANDLE,
    addr: *mut c_void,
}
impl Drop for AllocGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = VirtualFreeEx(self.process, self.addr, 0, MEM_RELEASE);
        }
    }
}
