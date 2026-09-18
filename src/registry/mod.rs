mod auth;
mod error;
mod http;
pub(crate) mod storage;
pub(crate) mod store;

pub use http::router;

pub(super) struct ImageName<'a> {
    pub(super) namespace: &'a str,
    pub(super) repository: &'a str,
    pub(super) suffix: &'a str,
}

pub(super) fn parse_image_name(value: &str) -> Result<ImageName<'_>, error::RegistryError> {
    if value.len() > 512 || !value.split('/').all(valid_component) {
        return Err(error::RegistryError::bad_request("Invalid image name."));
    }
    let (namespace, rest) = value.split_once('/').ok_or_else(|| {
        error::RegistryError::bad_request(
            "Image names must include a namespace and Git repository.",
        )
    })?;
    let (repository, suffix) = rest.split_once('/').unwrap_or((rest, ""));
    Ok(ImageName {
        namespace,
        repository,
        suffix,
    })
}

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
