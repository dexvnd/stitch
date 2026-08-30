pub mod pipe;
pub mod python;

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

#[unsafe(no_mangle)]
pub static STITCH_PAYLOAD: [u8; 16384] = {
    let mut buf = [0u8; 16384];
    buf[0] = 0x53;
    buf[1] = 0x54;
    buf[2] = 0x49;
    buf[3] = 0x54;
    buf[4] = 0x43;
    buf[5] = 0x48;
    buf[6] = 0x50;
    buf[7] = 0x4C;
    buf
};

fn get_payload() -> Option<String> {
    let bytes = &STITCH_PAYLOAD;
    let payload_bytes = &bytes[8..];
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
    let Ok(conn) = pipe::client::connect(pipe::PIPE_NAME) else {
        if let Ok(cpython) = python::CPython::init() {
            if let Some(code) = get_payload() {
                cpython.eval(&code);
            }
        }
        return;
    };

    let cpython = match python::CPython::init() {
        Ok(c) => c,
        Err(e) => {
            let _ = pipe::write_message(&conn, format!("ERR:{e}").as_bytes());
            return;
        }
    };

    if let Some(code) = get_payload() {
        cpython.eval(&code);
    }

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
            cpython.eval(&code);

            if pipe::write_message(&conn, b"").is_err() {
                break;
            }
        }

        conn = match pipe::client::connect(pipe::PIPE_NAME) {
            Ok(c) => c,
            Err(_) => return,
        };
    }
}
