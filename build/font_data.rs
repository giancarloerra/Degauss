//! Keep Slint's embedded glyph pixels unchanged without one Rust expression
//! per pixel. Byte literals remain fixed-size, compile-time arrays.

use std::fmt::Write;
use std::path::Path;

pub fn compact_file(path: &Path) {
    let source = std::fs::read_to_string(path).expect("reading generated Slint source");
    let compact = compact(&source).expect("compacting generated glyph pixels");
    std::fs::write(path, compact).expect("writing compact generated Slint source");
}

pub fn compact(source: &str) -> Result<String, String> {
    const PREFIX: &str = "static DATA : [u8 ;";
    let mut output = String::with_capacity(source.len() / 2);
    let mut remaining = source;
    while let Some(start) = remaining.find(PREFIX) {
        let array = &remaining[start + PREFIX.len()..];
        let length_end = array
            .find("] = [")
            .ok_or("unexpected glyph array declaration")?;
        let length: usize = array[..length_end]
            .trim()
            .strip_suffix("usize")
            .ok_or("unexpected glyph array length")?
            .parse()
            .map_err(|_| "invalid glyph array length")?;
        let values = &array[length_end + "] = [".len()..];
        let values_end = values.find(']').ok_or("unterminated glyph pixel array")?;
        let values = &values[..values_end];
        output.push_str(&remaining[..start + PREFIX.len() + length_end + "] = ".len()]);
        output.push_str("*b\"");
        let mut count = 0;
        if !values.trim().is_empty() {
            for value in values.split(',') {
                let value: u8 = value
                    .trim()
                    .strip_suffix("u8")
                    .ok_or("unexpected glyph pixel literal")?
                    .parse()
                    .map_err(|_| "invalid glyph pixel byte")?;
                write!(output, "\\x{value:02x}").expect("writing into a String");
                count += 1;
            }
        }
        if count != length {
            return Err("glyph pixel count differs from its array length".into());
        }
        output.push('"');
        let consumed = start + PREFIX.len() + length_end + "] = [".len() + values.len() + 1;
        remaining = &remaining[consumed..];
    }
    output.push_str(remaining);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::compact;

    #[test]
    fn updates_the_generated_file_without_changing_other_code() {
        let path = std::env::temp_dir().join(format!(
            "degauss-glyph-pixels-{}-{}.rs",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "static DATA : [u8 ; 1usize] = [127u8] ; & DATA").unwrap();
        super::compact_file(&path);
        let result = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(result, "static DATA : [u8 ; 1usize] = *b\"\\x7f\" ; & DATA");
    }

    #[test]
    fn keeps_every_pixel_byte_and_fixed_array_type() {
        let pixels = (0..=255)
            .map(|byte| format!("{byte}u8"))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!("static DATA : [u8 ; 256usize] = [{pixels}] ; & DATA");
        let output = compact(&source).unwrap();
        let expected = (0..=255)
            .map(|byte| format!("\\x{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            output,
            format!("static DATA : [u8 ; 256usize] = *b\"{expected}\" ; & DATA")
        );
        // Include quote, backslash, NUL and every non-ASCII byte, without
        // changing the element values or requiring runtime decoding.
        let decoded = expected
            .as_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|escape| {
                u8::from_str_radix(std::str::from_utf8(&escape[2..]).unwrap(), 16).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(decoded, (0..=255).collect::<Vec<u8>>());
    }

    #[test]
    fn preserves_metadata_empty_glyphs_and_multiple_arrays() {
        let source = "before static DATA : [u8 ;\n 0usize] = [] ; middle static DATA : [u8 ; 2usize] = [1u8 , 255u8] ; after";
        assert_eq!(compact(source).unwrap(), "before static DATA : [u8 ;\n 0usize] = *b\"\" ; middle static DATA : [u8 ; 2usize] = *b\"\\x01\\xff\" ; after");
        assert_eq!(compact("no glyph pixels").unwrap(), "no glyph pixels");
    }

    #[test]
    fn rejects_changed_or_truncated_generator_formats() {
        for source in [
            "static DATA : [u8 ; 2usize] = [1u8] ;",
            "static DATA : [u8 ; 1usize] = [256u8] ;",
            "static DATA : [u8 ; 1usize] = [1] ;",
            "static DATA : [u8 ; 1usize] = [1u8",
        ] {
            assert!(
                compact(source).is_err(),
                "invalid generated pixels were accepted"
            );
        }
    }
}
