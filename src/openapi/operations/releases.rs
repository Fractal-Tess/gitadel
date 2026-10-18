//! Releases.

use crate::{
    openapi::{Auth, Body, Method, Operation, json, json_list},
    repository::releases::{
        CreateReleaseRequest, ForgejoCreateReleaseRequest, ForgejoReleaseAssetResponse,
        ForgejoReleaseResponse, ForgejoUpdateReleaseRequest, ReleaseAssetResponse, ReleaseResponse,
        UpdateReleaseRequest,
    },
};

const TAG: &str = "Releases";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/releases",
        TAG,
        "Create a release (Forgejo compatible)",
    )
    .request(json::<ForgejoCreateReleaseRequest>())
    .response(json::<ForgejoReleaseResponse>())
    .created(),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/releases/tags/{tag}",
        TAG,
        "Get the release for a tag (Forgejo compatible)",
    )
    .response(json::<ForgejoReleaseResponse>()),
    Operation::new(
        Method::Patch,
        "/repos/{namespace}/{name}/releases/{id}",
        TAG,
        "Update a release (Forgejo compatible)",
    )
    .request(json::<ForgejoUpdateReleaseRequest>())
    .response(json::<ForgejoReleaseResponse>()),
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/releases/{id}/assets",
        TAG,
        "Upload a release asset (Forgejo compatible)",
    )
    .request(Body::Binary("multipart/form-data"))
    .response(json::<ForgejoReleaseAssetResponse>())
    .created(),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/releases",
        TAG,
        "List the repository's releases",
    )
    .auth(Auth::Optional)
    .response(json_list::<ReleaseResponse>())
    .query(&[
        ("page", "Page number, starting at 1"),
        ("per_page", "Releases per page, at most 100"),
    ]),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/releases",
        TAG,
        "Create a release",
    )
    .request(json::<CreateReleaseRequest>())
    .response(json::<ReleaseResponse>())
    .created(),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/releases/{id}",
        TAG,
        "Get a release",
    )
    .auth(Auth::Optional)
    .response(json::<ReleaseResponse>()),
    Operation::new(
        Method::Patch,
        "/repositories/{namespace}/{name}/releases/{id}",
        TAG,
        "Update a release",
    )
    .request(json::<UpdateReleaseRequest>())
    .response(json::<ReleaseResponse>()),
    Operation::new(
        Method::Delete,
        "/repositories/{namespace}/{name}/releases/{id}",
        TAG,
        "Delete a release",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Put,
        "/repositories/{namespace}/{name}/releases/{id}/assets",
        TAG,
        "Upload a release asset",
    )
    .request(Body::Binary("application/octet-stream"))
    .response(json::<ReleaseAssetResponse>())
    .query(&[("name", "File name of the asset")])
    .created(),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/releases/{release_id}/assets/{asset_id}",
        TAG,
        "Download a release asset",
    )
    .auth(Auth::Optional)
    .response(Body::Binary("application/octet-stream")),
    Operation::new(
        Method::Delete,
        "/repositories/{namespace}/{name}/releases/{release_id}/assets/{asset_id}",
        TAG,
        "Delete a release asset",
    )
    .response(Body::Empty),
];
