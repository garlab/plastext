use super::{Info, Region};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
const MAX_MESSAGE: usize = 16 * 1024;
#[derive(Serialize, Deserialize, Clone)]
pub(super) enum Operation {
    Info,
    Render(Vec<Region>),
}
#[derive(Serialize, Deserialize)]
pub(super) enum Response {
    Info(Info),
    Pixels { regions: Vec<Region>, delay_ms: u32 },
    Error(String),
}
pub(super) fn write(output: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::other("Image message exceeds limit"));
    }
    output.write_all(&(bytes.len() as u32).to_le_bytes())?;
    output.write_all(&bytes)?;
    output.flush()
}
pub(super) fn read<T: serde::de::DeserializeOwned>(input: &mut impl Read) -> io::Result<T> {
    let mut size = [0; 4];
    input.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    if size > MAX_MESSAGE {
        return Err(io::Error::other("Image message exceeds limit"));
    }
    let mut bytes = vec![0; size];
    input.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_headers_fail_before_payload_allocation_or_read() {
        let bytes = (MAX_MESSAGE as u32 + 1).to_le_bytes();
        let result = read::<Response>(&mut &bytes[..]);
        assert!(result.err().unwrap().to_string().contains("exceeds limit"));
    }
}
