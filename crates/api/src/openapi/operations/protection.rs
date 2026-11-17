//! Branch protection.

use crate::openapi::{Body, Method, Operation, json, json_list};
use crate::repository::protection::{
    CreateProtectionRuleRequest, ProtectionRuleResponse, UpdateProtectionRuleRequest,
};

const TAG: &str = "Branch protection";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/protection-rules",
        TAG,
        "List the repository's protection rules",
    )
    .response(json_list::<ProtectionRuleResponse>()),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/protection-rules",
        TAG,
        "Create a protection rule",
    )
    .request(json::<CreateProtectionRuleRequest>())
    .response(json::<ProtectionRuleResponse>())
    .created(),
    Operation::new(
        Method::Patch,
        "/repositories/{namespace}/{name}/protection-rules/{id}",
        TAG,
        "Update a protection rule",
    )
    .request(json::<UpdateProtectionRuleRequest>())
    .response(json::<ProtectionRuleResponse>()),
    Operation::new(
        Method::Delete,
        "/repositories/{namespace}/{name}/protection-rules/{id}",
        TAG,
        "Delete a protection rule",
    )
    .response(Body::Empty),
];
