use std::io::{self, Read};

pub(crate) fn decompress(data: &[u8], limit: usize) -> io::Result<Vec<u8>> {
    let mut decoder = zstd::stream::read::Decoder::new(data)?;
    decoder.window_log_max(28)?;
    let mut output = Vec::new();
    decoder.take(limit as u64 + 1).read_to_end(&mut output)?;
    if output.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Decompressed data exceeds its size limit",
        ));
    }
    Ok(output)
}
