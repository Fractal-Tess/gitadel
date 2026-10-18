//! Collaborators.

use crate::openapi::{Auth, Body, Method, Operation, json, json_list};
use crate::repository::{
    deploy_keys::{CreateDeployKeyRequest, DeployKeyResponse},
    issues::IssueUserResponse,
    resources::{AddCollaboratorRequest, CollaboratorResponse},
};

const TAG: &str = "Collaborators";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/assignable-users",
        TAG,
        "List users who can be assigned to issues",
    )
    .auth(Auth::Optional)
    .response(json_list::<IssueUserResponse>()),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/collaborators",
        TAG,
        "List the repository's collaborators",
    )
    .response(json_list::<CollaboratorResponse>()),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/collaborators",
        TAG,
        "Add a collaborator",
    )
    .request(json::<AddCollaboratorRequest>())
    .response(json::<CollaboratorResponse>())
    .created(),
    Operation::new(
        Method::Delete,
        "/repositories/{namespace}/{name}/collaborators/{username}",
        TAG,
        "Remove a collaborator",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/deploy-keys",
        TAG,
        "List the repository's deploy keys",
    )
    .response(json_list::<DeployKeyResponse>()),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/deploy-keys",
        TAG,
        "Add a deploy key",
    )
    .request(json::<CreateDeployKeyRequest>())
    .response(json::<DeployKeyResponse>())
    .created(),
    Operation::new(
        Method::Delete,
        "/repositories/{namespace}/{name}/deploy-keys/{id}",
        TAG,
        "Delete a deploy key",
    )
    .response(Body::Empty),
];
