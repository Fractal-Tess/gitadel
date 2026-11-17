//! PNG decoding shared by account pictures and instance favicons.

pub fn valid_png(bytes: &[u8]) -> bool {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let Ok(mut reader) = decoder.read_info() else {
        return false;
    };
    let Some(size) = reader.output_buffer_size() else {
        return false;
    };
    reader.next_frame(&mut vec![0; size]).is_ok()
}
