pub const FOOTER_LEN: usize = 52;
const MAGIC: &[u8; 8] = b"BOOTLEG1";

#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub src_width: u32,
    pub src_height: u32,
    pub ref_cols: u32,
    pub ref_rows: u32,
    pub fps: f64,
    pub frame_count: u32,
}

pub fn append(stub: &[u8], frames: &[u8], meta: &Meta) -> Vec<u8> {
    let mut out = Vec::with_capacity(stub.len() + frames.len() + FOOTER_LEN);
    out.extend_from_slice(stub);
    out.extend_from_slice(frames);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&meta.src_width.to_le_bytes());
    out.extend_from_slice(&meta.src_height.to_le_bytes());
    out.extend_from_slice(&meta.ref_cols.to_le_bytes());
    out.extend_from_slice(&meta.ref_rows.to_le_bytes());
    out.extend_from_slice(&meta.fps.to_le_bytes());
    out.extend_from_slice(&meta.frame_count.to_le_bytes());
    out.extend_from_slice(&(frames.len() as u64).to_le_bytes());
    out.extend_from_slice(MAGIC);
    out
}

pub fn read(file: &[u8]) -> Option<(Meta, std::ops::Range<usize>)> {
    if file.len() < FOOTER_LEN { return None; }
    let start = file.len() - FOOTER_LEN;
    let f = &file[start..];
    if &f[0..8] != MAGIC || &f[44..52] != MAGIC { return None; }
    let src_width = u32::from_le_bytes(f[8..12].try_into().ok()?);
    let src_height = u32::from_le_bytes(f[12..16].try_into().ok()?);
    let ref_cols = u32::from_le_bytes(f[16..20].try_into().ok()?);
    let ref_rows = u32::from_le_bytes(f[20..24].try_into().ok()?);
    let fps = f64::from_le_bytes(f[24..32].try_into().ok()?);
    let frame_count = u32::from_le_bytes(f[32..36].try_into().ok()?);
    let payload_len = u64::from_le_bytes(f[36..44].try_into().ok()?) as usize;
    if fps <= 0.0 || ref_cols == 0 || ref_rows == 0 || frame_count == 0 { return None; }
    let expect = frame_count as usize * ref_cols as usize * ref_rows as usize * 3;
    if payload_len != expect || start < payload_len { return None; }
    let payload = start - payload_len..start;
    Some((Meta { src_width, src_height, ref_cols, ref_rows, fps, frame_count }, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_reject_a_plain_stub() {
        let stub = b"not-a-real-elf-but-long-enough";
        let payload = vec![7u8; 2 * 2 * 2 * 3];
        let meta = Meta { src_width: 64, src_height: 36, ref_cols: 2, ref_rows: 2, fps: 8.0, frame_count: 2 };
        let file = append(stub, &payload, &meta);
        let (got, range) = read(&file).unwrap();
        assert_eq!(got.frame_count, 2);
        assert_eq!(got.fps, 8.0);
        assert_eq!(&file[range], payload.as_slice());
        assert!(read(stub).is_none());
    }

    #[test]
    fn reject_a_short_payload_length() {
        let mut file = append(b"stub", &[0u8; 12], &Meta {
            src_width: 1, src_height: 1, ref_cols: 2, ref_rows: 2, fps: 8.0, frame_count: 2,
        });
        let len_at = file.len() - 8 - 8;
        file[len_at..len_at + 8].copy_from_slice(&1u64.to_le_bytes());
        assert!(read(&file).is_none());
    }
}
