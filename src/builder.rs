use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine};
use stitch_stub::{PAYLOAD_CAPACITY, PAYLOAD_MAGIC};

use crate::injector::STUB_DLL_BYTES;

#[derive(Debug)]
pub enum BuildError {
    MarkerNotFound,
    ScriptTooLarge { max: usize, got: usize },
    Write(std::io::Error),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::MarkerNotFound => write!(f, "payload marker not found in stub DLL"),
            BuildError::ScriptTooLarge { max, got } => {
                write!(f, "script too large: {got} encoded bytes, max {max}")
            }
            BuildError::Write(e) => write!(f, "write failed: {e}"),
        }
    }
}

pub fn build_payload_dll(script: &str, out_path: &Path) -> Result<(), BuildError> {
    let bytes = patch_payload(STUB_DLL_BYTES, script)?;
    std::fs::write(out_path, bytes).map_err(BuildError::Write)
}

fn patch_payload(dll: &[u8], script: &str) -> Result<Vec<u8>, BuildError> {
    let region = PAYLOAD_CAPACITY - PAYLOAD_MAGIC.len();
    let marker = find_marker(dll, region).ok_or(BuildError::MarkerNotFound)?;
    let encoded = STANDARD.encode(script);
    let max = region - 1;
    if encoded.len() > max {
        return Err(BuildError::ScriptTooLarge { max, got: encoded.len() });
    }

    let mut out = dll.to_vec();
    let start = marker + PAYLOAD_MAGIC.len();
    for b in &mut out[start..start + region] {
        *b = 0;
    }
    out[start..start + encoded.len()].copy_from_slice(encoded.as_bytes());
    Ok(out)
}

fn find_marker(dll: &[u8], region: usize) -> Option<usize> {
    dll.windows(PAYLOAD_MAGIC.len())
        .enumerate()
        .filter(|(_, w)| *w == PAYLOAD_MAGIC)
        .map(|(i, _)| i)
        .find(|&i| {
            let start = i + PAYLOAD_MAGIC.len();
            dll.len() >= start + region && dll[start..start + region].iter().all(|&b| b == 0)
        })
}
