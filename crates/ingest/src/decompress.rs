use std::io::Read;

use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use maxi_loop_platform::error::AppError;

/// Decompress an HTTP request body according to `Content-Encoding` (`identity`, `gzip`, `deflate`, `br`)
/// while enforcing both compressed and decompressed size limits to prevent decompression bombs.
pub fn decompress_envelope_body(
    content_encoding: Option<&str>,
    raw_bytes: &[u8],
    max_compressed_bytes: usize,
    max_decompressed_bytes: usize,
) -> Result<Vec<u8>, AppError> {
    if raw_bytes.is_empty() {
        return Err(AppError::Validation(
            "envelope body must not be empty".to_owned(),
        ));
    }
    if raw_bytes.len() > max_compressed_bytes {
        return Err(AppError::PayloadTooLarge(
            "compressed envelope body exceeds maximum allowed size",
        ));
    }

    let encoding = content_encoding
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default();

    match encoding.as_str() {
        "" | "identity" => {
            if raw_bytes.len() > max_decompressed_bytes {
                return Err(AppError::PayloadTooLarge(
                    "envelope body exceeds maximum allowed size",
                ));
            }
            Ok(raw_bytes.to_vec())
        }
        "gzip" | "x-gzip" => {
            let decoder = GzDecoder::new(raw_bytes);
            read_bounded(decoder, max_decompressed_bytes, "invalid gzip stream")
        }
        "deflate" => {
            // Try RFC 1950 zlib-wrapped deflate first (standard for HTTP Content-Encoding: deflate
            // and Python/Go Sentry SDKs), falling back to raw RFC 1951 deflate if needed.
            let zlib_decoder = ZlibDecoder::new(raw_bytes);
            match read_bounded(
                zlib_decoder,
                max_decompressed_bytes,
                "invalid deflate stream",
            ) {
                Ok(out) => Ok(out),
                Err(AppError::PayloadTooLarge(msg)) => Err(AppError::PayloadTooLarge(msg)),
                Err(_) => {
                    let raw_decoder = DeflateDecoder::new(raw_bytes);
                    read_bounded(
                        raw_decoder,
                        max_decompressed_bytes,
                        "invalid deflate stream",
                    )
                }
            }
        }
        "br" | "brotli" => {
            let decoder = brotli::Decompressor::new(raw_bytes, 4096);
            read_bounded(decoder, max_decompressed_bytes, "invalid brotli stream")
        }
        _ => Err(AppError::UnsupportedMediaType(
            "unsupported content-encoding",
        )),
    }
}

fn read_bounded<R: Read>(
    reader: R,
    max_decompressed_bytes: usize,
    err_label: &'static str,
) -> Result<Vec<u8>, AppError> {
    let limit = (max_decompressed_bytes as u64).saturating_add(1);
    let mut bounded = reader.take(limit);
    let mut out = Vec::with_capacity(max_decompressed_bytes.min(8192));
    bounded
        .read_to_end(&mut out)
        .map_err(|_| AppError::Validation(err_label.to_owned()))?;

    if out.len() > max_decompressed_bytes {
        return Err(AppError::PayloadTooLarge(
            "decompressed envelope body exceeds maximum allowed size",
        ));
    }
    if out.is_empty() {
        return Err(AppError::Validation(
            "decompressed envelope body must not be empty".to_owned(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::{GzEncoder, ZlibEncoder};

    use super::*;

    #[test]
    fn decompresses_gzip_deflate_and_brotli_and_enforces_size_caps() {
        let sample = b"{\"event_id\":\"11111111111111111111111111111111\"}\n{\"type\":\"event\"}\n{\"message\":\"hello\"}\n";

        // gzip
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(sample).unwrap();
        let gz_bytes = gz.finish().unwrap();
        let out_gz =
            decompress_envelope_body(Some("gzip"), &gz_bytes, 4096, 4096).expect("gzip ok");
        assert_eq!(out_gz, sample);

        // deflate (zlib)
        let mut def = ZlibEncoder::new(Vec::new(), Compression::default());
        def.write_all(sample).unwrap();
        let def_bytes = def.finish().unwrap();
        let out_def =
            decompress_envelope_body(Some("deflate"), &def_bytes, 4096, 4096).expect("deflate ok");
        assert_eq!(out_def, sample);

        // brotli
        let mut br_bytes = Vec::new();
        {
            let mut br = brotli::CompressorWriter::new(&mut br_bytes, 4096, 5, 22);
            br.write_all(sample).unwrap();
        }
        let out_br = decompress_envelope_body(Some("br"), &br_bytes, 4096, 4096).expect("br ok");
        assert_eq!(out_br, sample);

        // Enforce decompressed limit (zip bomb protection)
        let err = decompress_envelope_body(Some("gzip"), &gz_bytes, 4096, 20).unwrap_err();
        assert!(matches!(err, AppError::PayloadTooLarge(_)));

        // Enforce compressed limit
        let err_comp = decompress_envelope_body(Some("gzip"), &gz_bytes, 5, 4096).unwrap_err();
        assert!(matches!(err_comp, AppError::PayloadTooLarge(_)));
    }
}
