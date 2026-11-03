// ConnReader::recv's HEAD: the next packet in the client's version, or the read as it is.
fn backcompat_recv(reader: &mut ConnReader) -> core::ops::ControlFlow<Result<(i32, Vec<u8>)>> {
    crate::connection::backcompat::on_recv(reader)
}
