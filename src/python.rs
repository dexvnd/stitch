use std::ffi::CString;
use std::os::raw::{c_int, c_void};

use windows::core::PCSTR;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

const PYTHON_DLL_NAMES: &[&str] = &[
    "Python314.dll", "Python313.dll", "Python312.dll", "Python311.dll",
    "Python310.dll", "Python39.dll", "Python38.dll", "Python37.dll",
    "Python36.dll", "Python35.dll", "Python34.dll", "Python33.dll",
    "Python32.dll", "Python31.dll", "Python30.dll",
];

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
                    break;
                }
            }
        }
        let module = found.ok_or(InitError::NoPythonModuleFound)?;

        unsafe {
            let py_gilstate_ensure: PyGilStateEnsureFn =
                resolve(module, "PyGILState_Ensure")?;
            let py_gilstate_release: PyGilStateReleaseFn =
                resolve(module, "PyGILState_Release")?;
            let py_run_simple_string_flags: PyRunSimpleStringFlagsFn =
                resolve(module, "PyRun_SimpleStringFlags")?;

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

    pub fn eval_capture(&self, code: &str) -> String {
        use base64::{engine::general_purpose::STANDARD, Engine};
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut out_path = std::env::temp_dir();
        out_path.push(format!("stitch_out_{}_{}.txt", std::process::id(), unique));
        let out_str = out_path.to_string_lossy().replace('\\', "/");
        let encoded = STANDARD.encode(code);

        let wrapper = format!(
            "import sys, io, base64, traceback
__stitch_buf = io.StringIO()
__stitch_old = (sys.stdout, sys.stderr)
sys.stdout = __stitch_buf
sys.stderr = __stitch_buf
try:
    exec(compile(base64.b64decode('{encoded}').decode('utf-8'), '<stitch>', 'exec'), __import__('__main__').__dict__)
except Exception:
    traceback.print_exc()
finally:
    sys.stdout, sys.stderr = __stitch_old
try:
    open('{out_str}', 'w', encoding='utf-8').write(__stitch_buf.getvalue())
except Exception:
    pass
"
        );

        let c_code = match CString::new(wrapper) {
            Ok(c) => c,
            Err(_) => return String::new(),
        };
        unsafe {
            let state = (self.py_gilstate_ensure)();
            (self.py_run_simple_string_flags)(c_code.as_ptr(), std::ptr::null());
            (self.py_gilstate_release)(state);
        }

        let data = std::fs::read(&out_path).unwrap_or_default();
        let _ = std::fs::remove_file(&out_path);
        String::from_utf8_lossy(&data).into_owned()
    }
}
