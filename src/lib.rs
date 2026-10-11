pub mod pipe;
pub mod python;

use std::cell::UnsafeCell;
use std::ffi::c_void;

use windows::Win32::Foundation::{BOOL, HINSTANCE};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        std::thread::spawn(stub_main);
    }
    BOOL(1)
}

pub const PAYLOAD_MAGIC: &[u8] = b"STITCHPL";
pub const PAYLOAD_CAPACITY: usize = 16384;

#[repr(transparent)]
pub struct Payload(UnsafeCell<[u8; PAYLOAD_CAPACITY]>);
unsafe impl Sync for Payload {}

#[unsafe(no_mangle)]
#[used]
pub static STITCH_PAYLOAD: Payload = Payload(UnsafeCell::new({
    let mut buf = [0u8; PAYLOAD_CAPACITY];
    let mut i = 0;
    while i < PAYLOAD_MAGIC.len() {
        buf[i] = PAYLOAD_MAGIC[i];
        i += 1;
    }
    buf
}));

fn get_payload() -> Option<String> {
    let base = STITCH_PAYLOAD.0.get() as *const u8;
    let mut buf = [0u8; PAYLOAD_CAPACITY];
    for i in 0..PAYLOAD_CAPACITY {
        buf[i] = unsafe { std::ptr::read_volatile(base.add(i)) };
    }
    let payload_bytes = &buf[PAYLOAD_MAGIC.len()..];
    let end = payload_bytes.iter().position(|&b| b == 0).unwrap_or(0);
    if end == 0 {
        return None;
    }
    let encoded = std::str::from_utf8(&payload_bytes[..end]).ok()?;
    use base64::{engine::general_purpose::STANDARD, Engine};
    let decoded = STANDARD.decode(encoded).ok()?;
    String::from_utf8(decoded).ok()
}

fn stub_main() {
    let cpython = match python::CPython::init() {
        Ok(c) => c,
        Err(e) => {
            if let Ok(conn) = pipe::client::connect(pipe::PIPE_NAME) {
                let _ = pipe::write_message(&conn, format!("ERR:{e}").as_bytes());
            }
            return;
        }
    };

    if let Some(code) = get_payload() {
        cpython.eval(&code);
        return;
    }

    let Ok(conn) = pipe::client::connect(pipe::PIPE_NAME) else {
        return;
    };

    let mut conn = conn;
    loop {
        if pipe::write_message(&conn, b"OK").is_err() {
            return;
        }

        loop {
            let code = match pipe::read_message(&conn) {
                Ok(Some(bytes)) => bytes,
                _ => break,
            };
            let code = String::from_utf8_lossy(&code);
            let out = cpython.eval_capture(&code);

            if pipe::write_message(&conn, out.as_bytes()).is_err() {
                break;
            }
        }

        conn = match pipe::client::connect(pipe::PIPE_NAME) {
            Ok(c) => c,
            Err(_) => return,
        };
    }
}
