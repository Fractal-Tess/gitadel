import { z } from "zod";

/** Events a webhook may subscribe to, in the order the server lists them. */
export const webhookEvents = [
  { id: "push", label: "Push", description: "Commits pushed to a branch or tag." },
  { id: "create", label: "Create", description: "A branch or tag is created." },
  { id: "delete", label: "Delete", description: "A branch or tag is deleted." },
  { id: "release", label: "Releases", description: "Published, updated, or deleted." },
  { id: "issues", label: "Issues", description: "Opened, edited, closed, labeled, assigned." },
  { id: "issue_comment", label: "Issue comments", description: "Created, edited, or deleted." },
  { id: "workflow_run", label: "Workflow runs", description: "Actions runs requested or completed." },
] as const;

export type WebhookEvent = (typeof webhookEvents)[number]["id"];

export function webhookEventLabel(id: string): string {
  return webhookEvents.find((event) => event.id === id)?.label ?? id;
}

export const webhookSchema = z.object({
  id: z.guid(),
  type: z.literal("Repository"),
  name: z.literal("web"),
  active: z.boolean(),
  events: z.array(z.string()),
  config: z.object({
    url: z.url(),
    content_type: z.literal("json"),
    insecure_ssl: z.literal("0"),
  }),
  url: z.url(),
  ping_url: z.url(),
  created_at: z.string(),
  updated_at: z.string(),
  last_delivery_at: z.string().nullable(),
  last_response: z.object({
    code: z.number().int().nullable(),
    status: z.enum(["ok", "failed", "unused"]),
    message: z.string().nullable(),
  }),
});

export const webhookDeliverySchema = z.object({
  id: z.guid(),
  event: z.string(),
  status_code: z.number().int().nullable(),
  status: z.enum(["ok", "failed", "pending"]),
  delivered_at: z.string(),
  duration_ms: z.number().int(),
  payload: z.unknown(),
  response_body: z.string().nullable(),
});

export type Webhook = z.infer<typeof webhookSchema>;
export type WebhookDelivery = z.infer<typeof webhookDeliverySchema>;
