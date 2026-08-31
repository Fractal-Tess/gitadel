import { z } from "zod";

export const publicOidcProviderSchema = z.object({
  id: z.guid(),
  name: z.string(),
});

export const authenticationConfigurationSchema = z.object({
  password_enabled: z.boolean(),
  passkey_enabled: z.boolean(),
  providers: z.array(publicOidcProviderSchema),
});

export const adminOidcProviderSchema = z.object({
  id: z.guid(),
  name: z.string(),
  issuer_url: z.url(),
  client_id: z.string(),
  has_client_secret: z.boolean(),
  enabled: z.boolean(),
  auto_provision: z.boolean(),
  callback_url: z.url(),
  created_at: z.string(),
  updated_at: z.string(),
});

export const adminOidcProvidersSchema = z.array(adminOidcProviderSchema);

export type AuthenticationConfiguration = z.infer<
  typeof authenticationConfigurationSchema
>;
export type PublicOidcProvider = z.infer<typeof publicOidcProviderSchema>;
export type AdminOidcProvider = z.infer<typeof adminOidcProviderSchema>;
