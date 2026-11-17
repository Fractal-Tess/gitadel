//! Organizations.

use crate::{
    identity::{avatar, resources},
    openapi::{Auth, Body, Method, Operation, json, json_list},
};

const TAG: &str = "Organizations";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Get,
        "/organizations",
        TAG,
        "List the organizations you belong to",
    )
    .response(json_list::<resources::OrganizationResponse>()),
    Operation::new(
        Method::Post,
        "/organizations",
        TAG,
        "Create an organization",
    )
    .request(json::<resources::CreateOrganizationRequest>())
    .response(json::<resources::OrganizationResponse>())
    .created(),
    Operation::new(
        Method::Put,
        "/organizations/{slug}",
        TAG,
        "Rename or update an organization",
    )
    .request(json::<resources::UpdateOrganizationRequest>())
    .response(json::<resources::OrganizationResponse>()),
    Operation::new(
        Method::Delete,
        "/organizations/{slug}",
        TAG,
        "Delete an empty organization",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Get,
        "/organizations/{slug}/avatar",
        TAG,
        "Download an organization's avatar",
    )
    .auth(Auth::Public)
    .response(Body::Binary("image/png")),
    Operation::new(
        Method::Put,
        "/organizations/{slug}/avatar",
        TAG,
        "Upload an organization avatar",
    )
    .request(json::<avatar::UpdateAvatarRequest>())
    .response(Body::Empty),
    Operation::new(
        Method::Delete,
        "/organizations/{slug}/avatar",
        TAG,
        "Remove an organization avatar",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Get,
        "/organizations/{slug}/member-suggestions",
        TAG,
        "Suggest users to add as members",
    )
    .response(json_list::<resources::MemberSuggestionResponse>())
    .query(&[(
        "q",
        "Username substring to match; at most 39 lowercase letters, digits, or hyphens",
    )]),
    Operation::new(
        Method::Get,
        "/organizations/{slug}/members",
        TAG,
        "List an organization's members",
    )
    .response(json_list::<resources::MemberResponse>()),
    Operation::new(
        Method::Post,
        "/organizations/{slug}/members",
        TAG,
        "Add a member to an organization",
    )
    .request(json::<resources::AddMemberRequest>())
    .response(json::<resources::MemberResponse>())
    .created(),
    Operation::new(
        Method::Delete,
        "/organizations/{slug}/members/{username}",
        TAG,
        "Remove a member from an organization",
    )
    .response(Body::Empty),
];
