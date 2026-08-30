use windows::Win32::Foundation::{CloseHandle, HMODULE};
use windows::Win32::System::ProcessStatus::{EnumProcessModules, EnumProcesses, GetModuleBaseNameW};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};

#[derive(Clone, Debug)]
pub struct PythonProcess {
    pub pid: u32,
    pub exe_name: String,
    pub module_name: String,
}

pub fn find_python_processes() -> Vec<PythonProcess> {
    let pids = list_pids();
    pids.into_iter()
        .filter_map(|pid| {
            let (exe_name, module_name) = python_module_in(pid)?;
            Some(PythonProcess { pid, exe_name, module_name })
        })
        .collect()
}

fn list_pids() -> Vec<u32> {
    let mut capacity = 1024usize;
    loop {
        let mut buf = vec![0u32; capacity];
        let mut bytes_returned = 0u32;
        let ok = unsafe {
            EnumProcesses(
                buf.as_mut_ptr(),
                (buf.len() * std::mem::size_of::<u32>()) as u32,
                &mut bytes_returned,
            )
        };
        if ok.is_err() {
            return Vec::new();
        }
        let count = bytes_returned as usize / std::mem::size_of::<u32>();
        if count < capacity {
            buf.truncate(count);
            return buf;
        }
        capacity *= 2;
    }
}

fn python_module_in(pid: u32) -> Option<(String, String)> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }.ok()?;

    struct Guard(windows::Win32::Foundation::HANDLE);
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    let guard = Guard(process);

    let mut exe_name_buf = [0u16; 260];
    let exe_len = unsafe { GetModuleBaseNameW(guard.0, HMODULE(std::ptr::null_mut()), &mut exe_name_buf) };
    if exe_len == 0 {
        return None;
    }
    let exe_name = String::from_utf16_lossy(&exe_name_buf[..exe_len as usize]);

    let mut modules = [HMODULE::default(); 256];
    let mut bytes_needed = 0u32;
    let ok = unsafe {
        EnumProcessModules(
            guard.0,
            modules.as_mut_ptr(),
            std::mem::size_of_val(&modules) as u32,
            &mut bytes_needed,
        )
    };
    if ok.is_err() {
        return None;
    }
    let count = (bytes_needed as usize / std::mem::size_of::<HMODULE>()).min(modules.len());

    for module in &modules[..count] {
        let mut name_buf = [0u16; 260];
        let len = unsafe { GetModuleBaseNameW(guard.0, *module, &mut name_buf) };
        if len == 0 {
            continue;
        }
        let name = String::from_utf16_lossy(&name_buf[..len as usize]);
        let lower = name.to_ascii_lowercase();
        if lower.starts_with("python") && lower.ends_with(".dll") {
            return Some((exe_name, name));
        }
    }
    None
}
