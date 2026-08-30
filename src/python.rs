use std::ffi::CString;
use std::os::raw::{c_int, c_void};

use windows::core::PCSTR;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

const PYTHON_DLL_NAMES: &[&str] = &[
    "Python311.dll", "Python310.dll", "Python39.dll", "Python38.dll",
    "Python37.dll", "Python36.dll", "Python35.dll", "Python34.dll",
    "Python33.dll", "Python32.dll", "Python31.dll", "Python30.dll",
];

type PySetProgramNameFn = unsafe extern "C" fn(*const u16);
type PyEvalInitThreadsFn = unsafe extern "C" fn();
type PyGilStateEnsureFn = unsafe extern "C" fn() -> c_int;
type PyGilStateReleaseFn = unsafe extern "C" fn(c_int);
type PyRunSimpleStringFlagsFn = unsafe extern "C" fn(*const i8, *const c_void) -> c_int;

pub struct CPython {
    py_gilstate_ensure: PyGilStateEnsureFn,
    py_gilstate_release: PyGilStateReleaseFn,
    py_run_simple_string_flags: PyRunSimpleStringFlagsFn,
}

#[derive(Debug)]
pub enum InitError {
    NoPythonModuleFound,
    MissingExport(&'static str),
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitError::NoPythonModuleFound => {
                write!(f, "no supported Python DLL found loaded in this process")
            }
            InitError::MissingExport(name) => write!(f, "missing export: {name}"),
        }
    }
}

unsafe fn resolve<F: Copy>(module: HMODULE, name: &'static str) -> Result<F, InitError> {
    let c_name = CString::new(name).unwrap();
    let addr = unsafe { GetProcAddress(module, PCSTR(c_name.as_ptr() as *const u8)) }
        .ok_or(InitError::MissingExport(name))?;
    Ok(unsafe { std::mem::transmute_copy(&addr) })
}

impl CPython {
    pub fn init() -> Result<Self, InitError> {
        let mut found: Option<HMODULE> = None;
        for name in PYTHON_DLL_NAMES {
            let c_name = CString::new(*name).unwrap();
            let handle = unsafe { GetModuleHandleA(PCSTR(c_name.as_ptr() as *const u8)) };
            if let Ok(h) = handle {
                if !h.is_invalid() {
                    found = Some(h);
                }
            }
        }
        let module = found.ok_or(InitError::NoPythonModuleFound)?;

        unsafe {
            let py_set_program_name: PySetProgramNameFn =
                resolve(module, "Py_SetProgramName")?;
            let py_eval_init_threads: PyEvalInitThreadsFn =
                resolve(module, "PyEval_InitThreads")?;
            let py_gilstate_ensure: PyGilStateEnsureFn =
                resolve(module, "PyGILState_Ensure")?;
            let py_gilstate_release: PyGilStateReleaseFn =
                resolve(module, "PyGILState_Release")?;
            let py_run_simple_string_flags: PyRunSimpleStringFlagsFn =
                resolve(module, "PyRun_SimpleStringFlags")?;

            let program_name: Vec<u16> = "Stitch\0".encode_utf16().collect();
            py_set_program_name(program_name.as_ptr());
            py_eval_init_threads();

            Ok(Self {
                py_gilstate_ensure,
                py_gilstate_release,
                py_run_simple_string_flags,
            })
        }
    }

    pub fn eval(&self, code: &str) {
        let c_code = match CString::new(code) {
            Ok(c) => c,
            Err(_) => return,
        };
        unsafe {
            let state = (self.py_gilstate_ensure)();
            (self.py_run_simple_string_flags)(c_code.as_ptr(), std::ptr::null());
            (self.py_gilstate_release)(state);
        }
    }
}
