//! Actions.

use crate::openapi::{Auth, Method, Operation};

const TAG: &str = "Actions";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Post,
        "/admin/actions/runner-registration-tokens",
        TAG,
        "Issue system registration",
    ),
    Operation::new(
        Method::Get,
        "/admin/actions/runners",
        TAG,
        "List system runners",
    ),
    Operation::new(
        Method::Delete,
        "/admin/actions/runners/{runner_id}",
        TAG,
        "Remove system runner",
    ),
    Operation::new(
        Method::Post,
        "/namespaces/{namespace}/actions/runner-registration-tokens",
        TAG,
        "Issue namespace registration",
    ),
    Operation::new(
        Method::Get,
        "/namespaces/{namespace}/actions/runners",
        TAG,
        "List namespace runners",
    ),
    Operation::new(
        Method::Delete,
        "/namespaces/{namespace}/actions/runners/{runner_id}",
        TAG,
        "Remove namespace runner",
    ),
    Operation::new(
        Method::Get,
        "/namespaces/{namespace}/actions/secrets",
        TAG,
        "List namespace secrets",
    ),
    Operation::new(
        Method::Put,
        "/namespaces/{namespace}/actions/secrets/{secret}",
        TAG,
        "Set namespace secret",
    ),
    Operation::new(
        Method::Delete,
        "/namespaces/{namespace}/actions/secrets/{secret}",
        TAG,
        "Delete namespace secret",
    ),
    Operation::new(
        Method::Get,
        "/namespaces/{namespace}/actions/variables",
        TAG,
        "List namespace variables",
    ),
    Operation::new(
        Method::Put,
        "/namespaces/{namespace}/actions/variables/{variable}",
        TAG,
        "Set namespace variable",
    ),
    Operation::new(
        Method::Delete,
        "/namespaces/{namespace}/actions/variables/{variable}",
        TAG,
        "Delete namespace variable",
    ),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/actions/runs",
        TAG,
        "List runs",
    )
    .auth(Auth::Optional),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/actions/runs/{run_id}",
        TAG,
        "Run detail",
    )
    .auth(Auth::Optional),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/actions/runs/{run_id}/artifacts",
        TAG,
        "List run artifacts",
    )
    .auth(Auth::Optional),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/actions/runs/{run_id}/artifacts/{artifact_id}",
        TAG,
        "Download run artifact",
    )
    .auth(Auth::Optional),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/actions/runs/{run_id}/cancel",
        TAG,
        "Cancel run",
    ),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/actions/runs/{run_id}/jobs/{job_id}/logs",
        TAG,
        "Job logs",
    )
    .auth(Auth::Optional),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/actions/runs/{run_id}/rerun",
        TAG,
        "Rerun run",
    ),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/actions/secrets",
        TAG,
        "List repository secrets",
    )
    .auth(Auth::Optional),
    Operation::new(
        Method::Put,
        "/repositories/{namespace}/{name}/actions/secrets/{secret}",
        TAG,
        "Set repository secret",
    ),
    Operation::new(
        Method::Delete,
        "/repositories/{namespace}/{name}/actions/secrets/{secret}",
        TAG,
        "Delete repository secret",
    ),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/actions/statuses",
        TAG,
        "Statuses",
    )
    .auth(Auth::Optional),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/actions/variables",
        TAG,
        "List repository variables",
    )
    .auth(Auth::Optional),
    Operation::new(
        Method::Put,
        "/repositories/{namespace}/{name}/actions/variables/{variable}",
        TAG,
        "Set repository variable",
    ),
    Operation::new(
        Method::Delete,
        "/repositories/{namespace}/{name}/actions/variables/{variable}",
        TAG,
        "Delete repository variable",
    ),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/actions/workflows",
        TAG,
        "List workflows",
    )
    .auth(Auth::Optional),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/actions/workflows/dispatch",
        TAG,
        "Dispatch",
    ),
];
