//! Webhooks.

use crate::{
    openapi::{Body, Method, Operation, json, json_list},
    repository::webhooks::{
        CreateWebhookRequest, UpdateWebhookRequest, WebhookDeliveryResponse, WebhookResponse,
    },
};

const TAG: &str = "Webhooks";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/hooks",
        TAG,
        "List the repository's webhooks",
    )
    .response(json_list::<WebhookResponse>()),
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/hooks",
        TAG,
        "Create a webhook",
    )
    .request(json::<CreateWebhookRequest>())
    .response(json::<WebhookResponse>())
    .created(),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/hooks/{id}",
        TAG,
        "Get a webhook",
    )
    .response(json::<WebhookResponse>()),
    Operation::new(
        Method::Patch,
        "/repos/{namespace}/{name}/hooks/{id}",
        TAG,
        "Update a webhook",
    )
    .request(json::<UpdateWebhookRequest>())
    .response(json::<WebhookResponse>()),
    Operation::new(
        Method::Delete,
        "/repos/{namespace}/{name}/hooks/{id}",
        TAG,
        "Delete a webhook",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/hooks/{id}/deliveries",
        TAG,
        "List a webhook's recent deliveries",
    )
    .response(json_list::<WebhookDeliveryResponse>()),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/hooks/{id}/deliveries/{delivery_id}",
        TAG,
        "Get a webhook delivery",
    )
    .response(json::<WebhookDeliveryResponse>()),
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/hooks/{id}/deliveries/{delivery_id}/attempts",
        TAG,
        "Redeliver a webhook delivery",
    )
    .request(Body::Empty)
    .response(Body::Empty)
    .accepted(),
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/hooks/{id}/pings",
        TAG,
        "Ping a webhook",
    )
    .request(Body::Empty)
    .response(Body::Empty),
];
