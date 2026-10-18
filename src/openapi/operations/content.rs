//! Repository content.

use crate::openapi::{Auth, Body, Method, Operation, json, json_list};
use crate::repository::{
    analysis::{ActivityResponse, LanguageStatResponse},
    browser::{
        BlobResponse, CommitResponse, DiffResponse, HistoryResponse, RefsResponse, TreeResponse,
    },
    files::{CreateFileRequest, CreateFileResponse},
    issues::{PreviewMarkdownRequest, RenderedMarkdownResponse},
};

const TAG: &str = "Repository content";

const REV: (&str, &str) = (
    "rev",
    "Branch, tag, or commit; defaults to the default branch",
);
const PATH: (&str, &str) = ("path", "Path within the repository");

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/activity",
        TAG,
        "Count the repository's commits per day",
    )
    .auth(Auth::Optional)
    .response(json::<ActivityResponse>())
    .query(&[(
        "days",
        "Days to cover, ending today, up to 365 (default 14)",
    )]),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/blob",
        TAG,
        "Read a file for display",
    )
    .auth(Auth::Optional)
    .response(json::<BlobResponse>())
    .query(&[REV, PATH]),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/commits/{revision}",
        TAG,
        "Get a commit",
    )
    .auth(Auth::Optional)
    .response(json::<CommitResponse>()),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/commits/{revision}/diff",
        TAG,
        "Get a commit's patch against its first parent",
    )
    .auth(Auth::Optional)
    .response(json::<DiffResponse>()),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/files",
        TAG,
        "Commit a new file",
    )
    .request(json::<CreateFileRequest>())
    .response(json::<CreateFileResponse>()),
    Operation::new(
        Method::Put,
        "/repositories/{namespace}/{name}/files",
        TAG,
        "Commit a change to an existing file",
    )
    .request(json::<CreateFileRequest>())
    .response(json::<CreateFileResponse>()),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/history",
        TAG,
        "List commits reachable from a revision",
    )
    .auth(Auth::Optional)
    .response(json::<HistoryResponse>())
    .query(&[
        REV,
        ("page", "Page number, starting at 1"),
        ("per_page", "Commits per page, 1 to 100 (default 30)"),
    ]),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/image",
        TAG,
        "Render an image file as PNG",
    )
    .auth(Auth::Optional)
    .response(Body::Binary("image/png"))
    .query(&[REV, PATH]),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/markdown-preview",
        TAG,
        "Render Markdown as HTML",
    )
    .auth(Auth::Optional)
    .request(json::<PreviewMarkdownRequest>())
    .response(json::<RenderedMarkdownResponse>()),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/raw",
        TAG,
        "Download a file's contents",
    )
    .auth(Auth::Optional)
    .response(Body::Binary("application/octet-stream"))
    .query(&[REV, PATH]),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/refs",
        TAG,
        "List branches and tags",
    )
    .auth(Auth::Optional)
    .response(json::<RefsResponse>()),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/source",
        TAG,
        "Download a source archive",
    )
    .auth(Auth::Optional)
    .response(Body::Binary("application/octet-stream"))
    .query(&[REV, ("format", "Archive format: `zip` or `tar.gz`")]),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/stats",
        TAG,
        "Count lines of code per language",
    )
    .auth(Auth::Optional)
    .response(json_list::<LanguageStatResponse>())
    .query(&[REV]),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/tree",
        TAG,
        "List a directory",
    )
    .auth(Auth::Optional)
    .response(json::<TreeResponse>())
    .query(&[REV, PATH]),
];
