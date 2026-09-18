import { z } from "zod";

export const registryReferenceSchema = z.object({
  tag: z.string().nullable(),
  digest: z.string(),
  updated_at: z.string().nullable(),
});

export const registryImageSchema = z.object({
  name: z.string(),
  size_bytes: z.number().int().nonnegative(),
  updated_at: z.string().nullable(),
  references: z.array(registryReferenceSchema),
});

export const registrySchema = z.object({
  registry_host: z.string(),
  image_prefix: z.string(),
  images: z.array(registryImageSchema),
});

export type RegistryReference = z.infer<typeof registryReferenceSchema>;
export type RegistryImage = z.infer<typeof registryImageSchema>;
export type Registry = z.infer<typeof registrySchema>;
