//! Registry payload storage, metadata persistence, usage, and legacy upgrades.

pub mod storage;
pub mod store;
pub mod upgrade;
pub mod usage;

#[cfg(test)]
mod test_support;

// Registry image components use the same grammar as the HTTP image names.
fn valid_component(value: &str) -> bool {
    let mut bytes = value.as_bytes();
    loop {
        let length = bytes
            .iter()
            .take_while(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            .count();
        if length == 0 {
            return false;
        }
        bytes = &bytes[length..];
        let Some(first) = bytes.first() else {
            return true;
        };
        let separator = match first {
            b'.' => 1,
            b'_' => {
                if bytes.get(1) == Some(&b'_') {
                    2
                } else {
                    1
                }
            }
            b'-' => bytes.iter().take_while(|byte| **byte == b'-').count(),
            _ => return false,
        };
        bytes = &bytes[separator..];
    }
}

/// Shared persisted/OCI image-component grammar. Splitting namespace and Git
/// repository coordinates remains the transport adapter's responsibility.
pub fn valid_image_path_components(value: &str) -> bool {
    value.len() <= 512 && value.split('/').all(valid_component)
}
