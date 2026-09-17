use std::{
    collections::VecDeque,
    io::{Cursor, Read, Write},
    process::{Command, Stdio},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};
use axum_extra::extract::cookie::CookieJar;
use bytes::Bytes;
use image::{ImageDecoder, ImageFormat, ImageReader};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::{io::AsyncReadExt, sync::Semaphore};

use super::{
    RepositoryState,
    browser::{BrowseQuery, normalize_browse_path, read_git, readable_repository},
};
use crate::{entity::repository, identity::ApiError};

pub(super) const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_EDGE: u32 = 16384;
const MAX_OUTPUT: usize = 80 * 1024 * 1024;
const CACHE_BYTES: usize = 32 * 1024 * 1024;
const RENDER_TIMEOUT: Duration = Duration::from_secs(8);
static RENDER_ADMISSION: Semaphore = Semaphore::const_new(8);
static RENDER_GATE: Semaphore = Semaphore::const_new(1);

#[derive(Clone, Serialize)]
pub(super) struct ImageMetadata {
    pub width: u32,
    pub height: u32,
    pub mime_type: String,
}

pub(super) struct RenderedImage {
    pub content: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub mime_type: String,
}

#[derive(Clone)]
pub(super) struct CachedImage {
    pub content: Bytes,
    pub metadata: ImageMetadata,
}

type ImageCache = VecDeque<([u8; 32], CachedImage)>;
static CACHE: LazyLock<Mutex<ImageCache>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

pub(super) fn is_image_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|part| part.to_str())
        .is_some_and(|extension| {
            [
                "svg", "png", "jpg", "jpeg", "webp", "gif", "avif", "bmp", "ico",
            ]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}
fn is_svg_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

pub(super) async fn render_thumbnail(
    bytes: Vec<u8>,
    path: String,
) -> Result<RenderedImage, ApiError> {
    render(bytes, path, true).await
}

async fn render(bytes: Vec<u8>, path: String, thumbnail: bool) -> Result<RenderedImage, ApiError> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ApiError::bad_request(
            "Image previews are limited to 16 MiB. Download the original instead.",
        ));
    }
    let deadline = Instant::now() + RENDER_TIMEOUT;
    let admission = RENDER_ADMISSION
        .try_acquire()
        .map_err(|_| ApiError::bad_request("The image renderer is busy. Try again shortly."))?;
    let worker = tokio::time::timeout(RENDER_TIMEOUT, RENDER_GATE.acquire())
        .await
        .map_err(|_| ApiError::bad_request("The image renderer is busy. Try again shortly."))?
        .map_err(ApiError::internal)?;
    tokio::task::spawn_blocking(move || {
        // Keep both permits until the process exits, even if the HTTP request is cancelled.
        let (_admission, _worker) = (admission, worker);
        render_isolated(&bytes, &path, thumbnail, deadline)
    })
    .await
    .map_err(ApiError::internal)?
}

fn render_isolated(
    bytes: &[u8],
    path: &str,
    thumbnail: bool,
    deadline: Instant,
) -> Result<RenderedImage, ApiError> {
    if Instant::now() >= deadline {
        return Err(ApiError::bad_request(
            "The image exceeded the rendering time limit.",
        ));
    }
    let mut command = Command::new(std::env::current_exe().map_err(ApiError::internal)?);
    command.arg("image-render");
    if thumbnail {
        command.arg("--thumbnail");
    }
    command
        .arg("--")
        .arg(path)
        .env("RUST_LOG", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(ApiError::internal)?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| ApiError::internal("Image worker stdin unavailable"))?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::internal("Image worker stdout unavailable"))?;
    let result = std::thread::scope(|scope| {
        let writer = scope.spawn(move || input.write_all(bytes));
        let reader = scope.spawn(move || {
            let mut bytes = Vec::new();
            output
                .take((MAX_OUTPUT + 1) as u64)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(ApiError::bad_request(
                        "The image exceeded the rendering time limit.",
                    ));
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(ApiError::internal(error));
                }
            }
        };
        let written = writer
            .join()
            .map_err(|_| ApiError::internal("Image input worker failed"))?;
        let output = reader
            .join()
            .map_err(|_| ApiError::internal("Image output worker failed"))?
            .map_err(ApiError::internal)?;
        if !status?.success() || written.is_err() || output.len() > MAX_OUTPUT {
            return Err(ApiError::bad_request(
                "This image is corrupt, unsupported, or exceeds the image safety limits.",
            ));
        }
        let mut cursor = Cursor::new(output);
        let mut header = [0_u8; 12];
        Read::read_exact(&mut cursor, &mut header).map_err(ApiError::internal)?;
        let width = u32::from_be_bytes(header[0..4].try_into().map_err(ApiError::internal)?);
        let height = u32::from_be_bytes(header[4..8].try_into().map_err(ApiError::internal)?);
        let mime_len =
            u32::from_be_bytes(header[8..12].try_into().map_err(ApiError::internal)?) as usize;
        if mime_len > 64 {
            return Err(ApiError::internal("Invalid image worker metadata"));
        }
        let mut mime = vec![0; mime_len];
        Read::read_exact(&mut cursor, &mut mime).map_err(ApiError::internal)?;
        let offset = cursor.position() as usize;
        let mut content = cursor.into_inner();
        content.drain(..offset);
        Ok(RenderedImage {
            content,
            width,
            height,
            mime_type: String::from_utf8(mime).map_err(ApiError::internal)?,
        })
    });
    result
}

pub(crate) fn worker(path: &str, thumbnail: bool) -> anyhow::Result<()> {
    // A separate process makes decoder allocation failures and pathological SVGs
    // killable without leaving runaway blocking tasks in the web process.
    #[cfg(unix)]
    unsafe {
        let memory = libc::rlimit {
            rlim_cur: 768 * 1024 * 1024,
            rlim_max: 768 * 1024 * 1024,
        };
        let cpu = libc::rlimit {
            rlim_cur: 6,
            rlim_max: 6,
        };
        ensure!(
            libc::setrlimit(libc::RLIMIT_AS, &memory) == 0,
            "Could not bound image worker memory"
        );
        ensure!(
            libc::setrlimit(libc::RLIMIT_CPU, &cpu) == 0,
            "Could not bound image worker CPU"
        );
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((MAX_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_IMAGE_BYTES, "Image exceeds input limit");
    let rendered = render_native(&bytes, path, thumbnail)?;
    let mut output = std::io::stdout().lock();
    output.write_all(&rendered.width.to_be_bytes())?;
    output.write_all(&rendered.height.to_be_bytes())?;
    output.write_all(&(rendered.mime_type.len() as u32).to_be_bytes())?;
    output.write_all(rendered.mime_type.as_bytes())?;
    output.write_all(&rendered.content)?;
    Ok(())
}

fn check_dimensions(width: u32, height: u32) -> anyhow::Result<()> {
    ensure!(
        width > 0
            && height > 0
            && width <= MAX_EDGE
            && height <= MAX_EDGE
            && u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "Image dimensions exceed safety limits"
    );
    Ok(())
}

fn render_native(bytes: &[u8], path: &str, thumbnail: bool) -> anyhow::Result<RenderedImage> {
    if let Ok(format) = image::guess_format(bytes) {
        ensure!(
            matches!(
                format,
                ImageFormat::Png
                    | ImageFormat::Jpeg
                    | ImageFormat::WebP
                    | ImageFormat::Gif
                    | ImageFormat::Avif
                    | ImageFormat::Bmp
                    | ImageFormat::Ico
            ),
            "Unsupported image format"
        );
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_EDGE);
        limits.max_image_height = Some(MAX_EDGE);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        let mut decoder = reader.into_decoder()?;
        let (width, height) = decoder.dimensions();
        check_dimensions(width, height)?;
        let orientation = decoder.orientation()?;
        let mut decoded = image::DynamicImage::from_decoder(decoder)?;
        decoded.apply_orientation(orientation);
        let width = decoded.width();
        let height = decoded.height();
        if thumbnail {
            decoded = decoded.thumbnail(512, 512);
        }
        let mut png = Cursor::new(Vec::new());
        decoded.write_to(&mut png, ImageFormat::Png)?;
        return Ok(RenderedImage {
            content: png.into_inner(),
            width,
            height,
            mime_type: format.to_mime_type().to_owned(),
        });
    }
    ensure!(is_svg_path(path), "Unsupported image content");
    let mut options = resvg::usvg::Options::default();
    options.image_href_resolver = resvg::usvg::ImageHrefResolver {
        resolve_data: Box::new(|_, data, _| {
            if data.len() > MAX_IMAGE_BYTES {
                return None;
            }
            let format = image::guess_format(&data).ok()?;
            let (width, height) = ImageReader::with_format(Cursor::new(data.as_slice()), format)
                .into_dimensions()
                .ok()?;
            check_dimensions(width, height).ok()?;
            match format {
                ImageFormat::Png => Some(resvg::usvg::ImageKind::PNG(data)),
                ImageFormat::Jpeg => Some(resvg::usvg::ImageKind::JPEG(data)),
                ImageFormat::Gif => Some(resvg::usvg::ImageKind::GIF(data)),
                ImageFormat::WebP => Some(resvg::usvg::ImageKind::WEBP(data)),
                _ => None,
            }
        }),
        resolve_string: Box::new(|_, _| None),
    };
    options.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_data(bytes, &options)?;
    let size = tree.size().to_int_size();
    let (width, height) = (size.width(), size.height());
    check_dimensions(width, height)?;
    let scale = if thumbnail {
        (512.0 / width.max(height) as f32).min(1.0)
    } else {
        1.0
    };
    let mut pixmap = resvg::tiny_skia::Pixmap::new(
        (width as f32 * scale).ceil() as u32,
        (height as f32 * scale).ceil() as u32,
    )
    .context("Could not allocate SVG surface")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(RenderedImage {
        content: pixmap.encode_png()?,
        width,
        height,
        mime_type: "image/svg+xml".to_owned(),
    })
}

pub(super) fn lfs_image_oid(content: &[u8]) -> Result<Option<&str>, ApiError> {
    if !content.starts_with(b"version https://git-lfs.github.com/spec/v1") {
        return Ok(None);
    }
    let Ok(pointer) = std::str::from_utf8(content) else {
        return Ok(None);
    };
    if pointer.lines().next() != Some("version https://git-lfs.github.com/spec/v1") {
        return Ok(None);
    }
    let oid = pointer
        .lines()
        .find_map(|line| line.strip_prefix("oid sha256:"))
        .ok_or_else(|| ApiError::bad_request("Invalid LFS image pointer."))?;
    if oid.len() != 64 || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request("Invalid LFS image identifier."));
    }
    Ok(Some(oid))
}

pub(super) async fn resolve_image_content(
    state: &RepositoryState,
    repository: &repository::Model,
    content: Vec<u8>,
) -> Result<Vec<u8>, ApiError> {
    if let Some(oid) = lfs_image_oid(&content)? {
        let key = state.lfs_object_key(repository, oid)?;
        let store = state.lfs_store();
        let metadata = store
            .stat(&key)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::bad_request("The LFS image has not been uploaded."))?;
        if metadata.size > MAX_IMAGE_BYTES as u64 {
            return Err(ApiError::bad_request(
                "Image previews are limited to 16 MiB. Download the original instead.",
            ));
        }
        let reader = store.read(&key).await.map_err(ApiError::internal)?;
        let mut resolved = Vec::new();
        reader
            .take((MAX_IMAGE_BYTES + 1) as u64)
            .read_to_end(&mut resolved)
            .await
            .map_err(ApiError::internal)?;
        if resolved.len() > MAX_IMAGE_BYTES {
            return Err(ApiError::bad_request("Image exceeds preview size limits."));
        }
        return Ok(resolved);
    }
    if content.len() > MAX_IMAGE_BYTES {
        return Err(ApiError::bad_request(
            "Image previews are limited to 16 MiB. Download the original instead.",
        ));
    }
    Ok(content)
}

pub(super) async fn cached_image(content: Vec<u8>, path: String) -> Result<CachedImage, ApiError> {
    let key: [u8; 32] = Sha256::new()
        .chain_update([u8::from(is_svg_path(&path))])
        .chain_update(&content)
        .finalize()
        .into();
    let cache = &CACHE;
    if let Some(found) = cache
        .lock()
        .map_err(ApiError::internal)?
        .iter()
        .find(|(hash, _)| hash == &key)
        .map(|(_, image)| image.clone())
    {
        return Ok(found);
    }
    let rendered = render(content, path, false).await?;
    let result = CachedImage {
        metadata: ImageMetadata {
            width: rendered.width,
            height: rendered.height,
            mime_type: rendered.mime_type,
        },
        content: Bytes::from(rendered.content),
    };
    if result.content.len() <= CACHE_BYTES {
        let mut cache = cache.lock().map_err(ApiError::internal)?;
        while cache.len() >= 64
            || cache
                .iter()
                .map(|(_, image)| image.content.len())
                .sum::<usize>()
                + result.content.len()
                > CACHE_BYTES
        {
            cache.pop_front();
        }
        cache.push_back((key, result.clone()));
    }
    Ok(result)
}

pub(super) async fn preview(
    State(state): State<RepositoryState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<BrowseQuery>,
) -> Result<Response, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let revision = query
        .rev
        .or_else(|| repository.default_branch.clone())
        .ok_or_else(|| ApiError::bad_request("This repository has no default branch yet."))?;
    let path = normalize_browse_path(&query.path)?;
    if !is_image_path(&path) {
        return Err(ApiError::bad_request(
            "This file format has no image preview.",
        ));
    }
    let git_path = state.repository_path(&repository);
    let requested_path = path.clone();
    let content = read_git(git_path, move |git| {
        let resolved = git.resolve_path(&revision, &requested_path)?;
        let size = git.read_object_header(&resolved.oid)?.map(|(_, size)| size);
        if !size.is_some_and(|size| size <= MAX_IMAGE_BYTES as u64) {
            return Err(sley::GitError::InvalidPath(
                "Image exceeds preview size limits".to_owned(),
            ));
        }
        git.blobs().read(resolved.oid)
    })
    .await?;
    let content = resolve_image_content(&state, &repository, content).await?;
    let image = cached_image(content, path).await?;
    let mut response = Response::new(Body::from(image.content));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_external_images_are_not_loaded() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><image href="file:///etc/passwd" width="2" height="2"/><image href="https://example.invalid/private" width="2" height="2"/><script>alert(1)</script></svg>"#;
        let rendered = render_native(svg, "logo.svg", false).unwrap();
        let decoded = image::load_from_memory(&rendered.content)
            .unwrap()
            .to_rgba8();
        assert!(decoded.pixels().all(|pixel| pixel.0 == [0, 0, 0, 0]));
    }

    #[test]
    fn svg_dimensions_are_bounded_before_surface_allocation() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="100000"/>"#;
        assert!(render_native(svg, "logo.svg", false).is_err());
    }

    #[test]
    fn thumbnails_preserve_wide_logo_proportions() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="300"><rect width="1200" height="300" fill="red"/></svg>"#;
        let rendered = render_native(svg, "logo.svg", true).unwrap();
        let decoded = image::load_from_memory(&rendered.content).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (512, 128));
        assert_eq!((rendered.width, rendered.height), (1200, 300));
    }
}
