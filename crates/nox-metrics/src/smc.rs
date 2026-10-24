// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/FanControl/FanControlSupport.swift (SMCValueCodec), commit e916961.
//! SMC value decoding (pure) and key helpers.

pub fn fourcc(s: &str) -> u32 {
    s.bytes()
        .take(4)
        .fold(0, |acc, b| (acc << 8) | u32::from(b))
}

pub fn fourcc_str(v: u32) -> String {
    v.to_be_bytes()
        .iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '?'
            }
        })
        .collect()
}

/// Decodes an SMC payload of type `kind`. `None` for unknown types or sizes.
///
/// Apple-silicon SMCs publish sensors as little-endian `flt `; Intel-era types
/// (`sp78`, `fpe2`, big-endian integers) are kept for completeness.
pub fn decode(bytes: &[u8], kind: &[u8; 4]) -> Option<f64> {
    let v = match (kind, bytes.len()) {
        (b"flt ", 4) => f64::from(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        (b"fpe2", 2) => f64::from(u16::from_be_bytes([bytes[0], bytes[1]])) / 4.0,
        (b"sp78", 2) => f64::from(i16::from_be_bytes([bytes[0], bytes[1]])) / 256.0,
        (b"ui8 ", 1) => f64::from(bytes[0]),
        (b"ui16", 2) => f64::from(u16::from_be_bytes([bytes[0], bytes[1]])),
        (b"ui32", 4) => f64::from(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        (b"ioft", 8) => {
            let raw = u64::from_le_bytes(bytes.try_into().ok()?);
            raw as f64 / 65_536.0
        }
        _ => return None,
    };
    v.is_finite().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourcc_roundtrip() {
        assert_eq!(fourcc("#KEY"), 0x234B_4559);
        assert_eq!(fourcc_str(fourcc("Tp01")), "Tp01");
        assert_eq!(fourcc_str(fourcc("ui8 ")), "ui8 ");
    }

    #[test]
    fn decodes_known_types() {
        assert_eq!(decode(&42.5f32.to_le_bytes(), b"flt "), Some(42.5));
        // sp78: 0x2E80 = 46.5 °C
        assert_eq!(decode(&[0x2E, 0x80], b"sp78"), Some(46.5));
        // negative sp78
        assert_eq!(decode(&[0xFF, 0x00], b"sp78"), Some(-1.0));
        // fpe2: 0x1F40 / 4 = 2000 rpm
        assert_eq!(decode(&[0x1F, 0x40], b"fpe2"), Some(2000.0));
        assert_eq!(decode(&[0], b"ui8 "), Some(0.0));
        assert_eq!(decode(&[0x01, 0x02], b"ui16"), Some(258.0));
        assert_eq!(decode(&[0, 0, 1, 0], b"ui32"), Some(256.0));
        let ioft = (3u64 * 65_536 + 32_768).to_le_bytes();
        assert_eq!(decode(&ioft, b"ioft"), Some(3.5));
    }

    #[test]
    fn rejects_bad_sizes_and_nan() {
        assert_eq!(decode(&[1, 2, 3], b"flt "), None);
        assert_eq!(decode(&[1, 2], b"xxxx"), None);
        assert_eq!(decode(&f32::NAN.to_le_bytes(), b"flt "), None);
    }
}
