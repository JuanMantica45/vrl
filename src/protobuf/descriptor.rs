use prost_reflect::{DescriptorPool, MessageDescriptor};
use std::io::Read;
use std::path::Path;

/// Maximum size of a protobuf descriptor set this will read (OBE-10728).
///
/// A descriptor set is normally a few KiB; the cap only exists so that an
/// oversized or attacker-chosen path cannot be turned into an unbounded read.
const MAX_DESCRIPTOR_FILE_BYTES: u64 = 64 * 1024 * 1024; // 64 MiB

pub fn get_message_descriptor(
    descriptor_set_path: &Path,
    message_type: &str,
) -> std::result::Result<MessageDescriptor, String> {
    // Stat before opening, and reject anything that is not a regular file
    // (OBE-10728). Reading a character device such as `/dev/zero` never
    // terminates, and merely *opening* a FIFO blocks until a writer appears, so
    // this check has to happen before the file is opened rather than after.
    // `metadata` follows symlinks, so a symlink to a device is rejected too.
    let metadata = std::fs::metadata(descriptor_set_path).map_err(|e| {
        format!("Failed to open protobuf desc file '{descriptor_set_path:?}': {e}")
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "protobuf desc file '{descriptor_set_path:?}' is not a regular file"
        ));
    }
    if metadata.len() > MAX_DESCRIPTOR_FILE_BYTES {
        return Err(format!(
            "protobuf desc file '{descriptor_set_path:?}' is larger than the {MAX_DESCRIPTOR_FILE_BYTES} byte limit"
        ));
    }

    // Bound the read itself as well: the file could have grown, or been
    // swapped, between the stat above and the open below.
    let mut b = Vec::new();
    std::fs::File::open(descriptor_set_path)
        .map_err(|e| format!("Failed to open protobuf desc file '{descriptor_set_path:?}': {e}"))?
        .take(MAX_DESCRIPTOR_FILE_BYTES + 1)
        .read_to_end(&mut b)
        .map_err(|e| {
            format!("Failed to open protobuf desc file '{descriptor_set_path:?}': {e}")
        })?;
    if b.len() as u64 > MAX_DESCRIPTOR_FILE_BYTES {
        return Err(format!(
            "protobuf desc file '{descriptor_set_path:?}' is larger than the {MAX_DESCRIPTOR_FILE_BYTES} byte limit"
        ));
    }
    let pool = DescriptorPool::decode(b.as_slice()).map_err(|e| {
        format!("Failed to parse protobuf desc file '{descriptor_set_path:?}': {e}")
    })?;
    pool.get_message_by_name(message_type).ok_or_else(|| {
        format!("The message type '{message_type}' could not be found in '{descriptor_set_path:?}'")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("tests/data/protobuf")
    }

    // OBE-10728: a valid descriptor set must still load — the new file-type and
    // size guards must not reject legitimate input.
    #[test]
    fn valid_descriptor_still_loads() {
        let path = descriptor_dir().join("test_protobuf.desc");
        get_message_descriptor(&path, "test_protobuf.Person")
            .expect("a real descriptor set must still load");
    }

    #[test]
    fn missing_file_returns_error() {
        let err = get_message_descriptor(Path::new("/nonexistent-descriptor-set"), "X")
            .expect_err("a missing path must be an error, not a panic");
        assert!(
            err.contains("Failed to open protobuf desc file"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn unknown_message_type_returns_error() {
        let path = descriptor_dir().join("test_protobuf.desc");
        let err = get_message_descriptor(&path, "no.Such.Message")
            .expect_err("an unknown message type must be an error");
        assert!(err.contains("could not be found"), "unexpected error: {err}");
    }

    #[test]
    fn undecodable_file_returns_error() {
        // A real regular file that is not a FileDescriptorSet.
        let path = descriptor_dir().join("test_protobuf.proto");
        let err = get_message_descriptor(&path, "X")
            .expect_err("a non-descriptor file must be an error");
        assert!(
            err.contains("Failed to parse protobuf desc file"),
            "unexpected error: {err}"
        );
    }

    // The important one: reading a character device never terminates, and even
    // *opening* a FIFO blocks until a writer appears. Both must be rejected on
    // the strength of their file type alone, without being opened.
    #[cfg(unix)]
    #[test]
    fn character_device_is_rejected_without_reading() {
        let err = get_message_descriptor(Path::new("/dev/zero"), "X")
            .expect_err("/dev/zero must be rejected, not read forever");
        assert!(
            err.contains("is not a regular file"),
            "must be rejected on file type: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_is_rejected() {
        let err = get_message_descriptor(&descriptor_dir(), "X")
            .expect_err("a directory must be rejected");
        assert!(
            err.contains("is not a regular file"),
            "unexpected error: {err}"
        );
    }
}
