mod auth;
mod error;
mod http;
pub(crate) use gitadel_storage::registry::{storage, store};

#[cfg(test)]
pub(crate) mod test_support;

pub use http::router;

pub(super) struct ImageName<'a> {
    pub(super) namespace: &'a str,
    pub(super) repository: &'a str,
    pub(super) suffix: &'a str,
}

pub(super) fn parse_image_name(value: &str) -> Result<ImageName<'_>, error::RegistryError> {
    if !gitadel_storage::registry::valid_image_path_components(value) {
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
