import { z } from "zod";

export const releaseAssetSchema = z.object({
  id: z.guid(),
  name: z.string(),
  content_type: z.string(),
  size_bytes: z.number().int().nonnegative(),
  download_count: z.number().int().nonnegative(),
  created_at: z.string(),
  download_url: z.string(),
  external_url: z.string().nullable(),
});

export const releaseSchema = z.object({
  id: z.guid(),
  target_revision: z.string(),
  target_oid: z.string(),
  title: z.string(),
  body: z.string(),
  rendered_body: z.string(),
  prerelease: z.boolean(),
  latest: z.boolean(),
  author: z.string(),
  published_at: z.string(),
  created_at: z.string(),
  updated_at: z.string(),
  assets: z.array(releaseAssetSchema),
  external_url: z.string().nullable(),
  external_author: z
    .object({ username: z.string(), profile_url: z.string().nullable() })
    .nullable(),
});

export type Release = z.infer<typeof releaseSchema>;
export type ReleaseAsset = z.infer<typeof releaseAssetSchema>;
