use cell_format::{read::read_single_chunk, write::write_cell};
#[test]
fn unrelated_invalid_payload_is_skipped_and_requested_payload_is_strict() {
    let mut bytes = write_cell(0, 0, &[]);
    bytes.extend_from_slice(b"BLDG");
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    let empty = write_cell(0, 0, &[(*b"ROAD", &[])]);
    bytes.extend_from_slice(&empty[10..]);
    assert!(read_single_chunk(&bytes, *b"ROAD").unwrap().is_empty());
    assert!(read_single_chunk(&bytes, *b"BLDG").is_none());
    assert!(read_single_chunk(&bytes[..bytes.len() - 1], *b"ROAD").is_none());
}
