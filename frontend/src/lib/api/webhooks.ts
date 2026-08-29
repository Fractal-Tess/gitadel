import { z } from "zod";

export const webhookSchema = z.object({
  id: z.guid(),
  type: z.literal("Repository"),
  name: z.literal("web"),
  active: z.boolean(),
  events: z.tuple([z.literal("push")]),
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
  status: z.enum(["ok", "failed"]),
  delivered_at: z.string(),
  duration_ms: z.number().int(),
  payload: z.unknown(),
  response_body: z.string().nullable(),
});

export type Webhook = z.infer<typeof webhookSchema>;
export type WebhookDelivery = z.infer<typeof webhookDeliverySchema>;
