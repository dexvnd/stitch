use std::io;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_SHARE_MODE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW};

pub const PIPE_NAME: &str = r"\\.\pipe\pystitch";

pub struct PipeConn(HANDLE);

impl Drop for PipeConn {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn os_err(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0)
}

pub fn write_message(conn: &PipeConn, data: &[u8]) -> io::Result<()> {
    write_all(conn, &(data.len() as u32).to_le_bytes())?;
    write_all(conn, data)
}

pub fn read_message(conn: &PipeConn) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    if !read_exact(conn, &mut len_buf)? {
        return Ok(None);
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut data = vec![0u8; len];
    if !read_exact(conn, &mut data)? {
        return Ok(None);
    }
    Ok(Some(data))
}

fn write_all(conn: &PipeConn, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let mut written = 0u32;
        unsafe { WriteFile(conn.0, Some(buf), Some(&mut written), None) }.map_err(os_err)?;
        if written == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "pipe wrote 0 bytes"));
        }
        buf = &buf[written as usize..];
    }
    Ok(())
}

fn read_exact(conn: &PipeConn, buf: &mut [u8]) -> io::Result<bool> {
    let mut total = 0usize;
    while total < buf.len() {
        let mut read = 0u32;
        let result = unsafe { ReadFile(conn.0, Some(&mut buf[total..]), Some(&mut read), None) };
        if let Err(e) = result {
            return if total == 0 { Ok(false) } else { Err(os_err(e)) };
        }
        if read == 0 {
            return if total == 0 {
                Ok(false)
            } else {
                Err(io::Error::new(io::ErrorKind::UnexpectedEof, "pipe closed mid-message"))
            };
        }
        total += read as usize;
    }
    Ok(true)
}

pub mod server {
    use super::*;
    use std::time::Duration;
    use windows::Win32::Foundation::ERROR_OPERATION_ABORTED;
    use windows::Win32::System::Pipes::{PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT};
    use windows::Win32::System::Threading::{
        GetCurrentThreadId, OpenThread, THREAD_TERMINATE,
    };
    use windows::Win32::System::IO::CancelSynchronousIo;

    const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

    pub fn wait_for_client() -> io::Result<PipeConn> {
        let wide_name = to_wide(PIPE_NAME);
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide_name.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                65536,
                65536,
                0,
                None,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let conn = PipeConn(handle);

        let this_thread_id = unsafe { GetCurrentThreadId() };
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            if done_rx.recv_timeout(CONNECT_TIMEOUT).is_ok() {
                return;
            }
            if let Ok(thread_handle) = unsafe { OpenThread(THREAD_TERMINATE, false, this_thread_id) }
            {
                unsafe {
                    let _ = CancelSynchronousIo(thread_handle);
                    let _ = CloseHandle(thread_handle);
                }
            }
        });

        let result = unsafe { ConnectNamedPipe(conn.0, None) };
        let _ = done_tx.send(());

        match result {
            Ok(()) => Ok(conn),
            Err(e) if e.code() == ERROR_OPERATION_ABORTED.to_hresult() => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for the stub DLL to connect",
            )),
            Err(e) => Err(os_err(e)),
        }
    }
}

pub mod client {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY};

    const CONNECT_RETRY_BUDGET: Duration = Duration::from_secs(10);

    pub fn connect(name: &str) -> io::Result<PipeConn> {
        let wide_name = to_wide(name);
        let deadline = Instant::now() + CONNECT_RETRY_BUDGET;
        loop {
            let result = unsafe {
                CreateFileW(
                    PCWSTR(wide_name.as_ptr()),
                    (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
                    FILE_SHARE_MODE(0),
                    None,
                    OPEN_EXISTING,
                    FILE_FLAGS_AND_ATTRIBUTES(0),
                    None,
                )
            };
            match result {
                Ok(handle) => return Ok(PipeConn(handle)),
                Err(e)
                    if (e.code() == ERROR_FILE_NOT_FOUND.to_hresult()
                        || e.code() == ERROR_PIPE_BUSY.to_hresult())
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(os_err(e)),
            }
        }
    }
}
