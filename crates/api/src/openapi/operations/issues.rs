//! Issues.

use crate::{
    openapi::{Auth, Body, Method, Operation, json, json_list},
    repository::issues::{
        CommentRequest, CreateIssueRequest, CreateLabelRequest, IssueAttachmentResponse,
        IssueCommentResponse, IssueLabelResponse, IssueResponse, UpdateIssueRequest,
        UpdateLabelRequest,
    },
};

const TAG: &str = "Issues";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Put,
        "/repositories/{namespace}/{name}/issue-attachments",
        TAG,
        "Upload an issue attachment",
    )
    .request(Body::Binary("application/octet-stream"))
    .response(json::<IssueAttachmentResponse>())
    .query(&[("name", "File name of the attachment")])
    .created(),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/issue-attachments/{attachment_id}",
        TAG,
        "Download an issue attachment",
    )
    .auth(Auth::Optional)
    .response(Body::Binary("application/octet-stream")),
    Operation::new(
        Method::Delete,
        "/repositories/{namespace}/{name}/issue-attachments/{attachment_id}",
        TAG,
        "Delete an issue attachment",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/issue-labels",
        TAG,
        "List the repository's issue labels",
    )
    .auth(Auth::Optional)
    .response(json_list::<IssueLabelResponse>()),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/issue-labels",
        TAG,
        "Create an issue label",
    )
    .request(json::<CreateLabelRequest>())
    .response(json::<IssueLabelResponse>())
    .created(),
    Operation::new(
        Method::Patch,
        "/repositories/{namespace}/{name}/issue-labels/{id}",
        TAG,
        "Update an issue label",
    )
    .request(json::<UpdateLabelRequest>())
    .response(json::<IssueLabelResponse>()),
    Operation::new(
        Method::Delete,
        "/repositories/{namespace}/{name}/issue-labels/{id}",
        TAG,
        "Delete an issue label",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/issues",
        TAG,
        "List the repository's issues",
    )
    .auth(Auth::Optional)
    .response(json_list::<IssueResponse>())
    .query(&[
        ("state", "Only issues in this state: `open` or `closed`"),
        ("q", "Only issues whose title or body contains this text"),
        ("page", "Page number, starting at 1"),
        ("per_page", "Issues per page, at most 100"),
    ]),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/issues",
        TAG,
        "Create an issue",
    )
    .request(json::<CreateIssueRequest>())
    .response(json::<IssueResponse>())
    .created(),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/issues/{number}",
        TAG,
        "Get an issue",
    )
    .auth(Auth::Optional)
    .response(json::<IssueResponse>()),
    Operation::new(
        Method::Patch,
        "/repositories/{namespace}/{name}/issues/{number}",
        TAG,
        "Update an issue",
    )
    .request(json::<UpdateIssueRequest>())
    .response(json::<IssueResponse>()),
    Operation::new(
        Method::Delete,
        "/repositories/{namespace}/{name}/issues/{number}",
        TAG,
        "Delete an issue",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/issues/{number}/comments",
        TAG,
        "List an issue's comments",
    )
    .auth(Auth::Optional)
    .response(json_list::<IssueCommentResponse>()),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/issues/{number}/comments",
        TAG,
        "Comment on an issue",
    )
    .request(json::<CommentRequest>())
    .response(json::<IssueCommentResponse>())
    .created(),
    Operation::new(
        Method::Patch,
        "/repositories/{namespace}/{name}/issues/{number}/comments/{id}",
        TAG,
        "Edit an issue comment",
    )
    .request(json::<CommentRequest>())
    .response(json::<IssueCommentResponse>()),
    Operation::new(
        Method::Delete,
        "/repositories/{namespace}/{name}/issues/{number}/comments/{id}",
        TAG,
        "Delete an issue comment",
    )
    .response(Body::Empty),
];
