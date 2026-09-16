import { z } from "zod";

export const repositorySchema = z.object({
  id: z.guid(),
  namespace: z.string(),
  name: z.string(),
  description: z.string().nullable(),
  visibility: z.enum(["public", "private"]),
  topics: z.array(z.string()).default([]),
  object_format: z.enum(["sha1", "sha256"]),
  mirrored: z.boolean(),
  default_branch: z.string(),
  archived_at: z.string().nullable(),
  icon_updated_at: z.string().nullable(),
  icon_source: z.enum(["manual", "detected"]).nullable(),
  created_at: z.string(),
  updated_at: z.string(),
  favorited: z.boolean(),
  ssh_clone_url: z.string(),
  can_manage: z.boolean(),
});

export const topicsSchema = z.object({
  topics: z.array(z.string()),
});

export const renderedMarkdownSchema = z.object({
  rendered_html: z.string(),
});

export const repositoryActivitySchema = z.object({
  start_date: z.iso.date(),
  end_date: z.iso.date(),
  total_commits: z.number().int().nonnegative(),
  days: z.array(
    z.object({
      date: z.iso.date(),
      count: z.number().int().positive(),
    }),
  ),
});

export const repositoryOverviewItemSchema = repositorySchema.extend({
  branch_count: z.number().int().nonnegative(),
  commit_count: z.number().int().nonnegative(),
  total_lines: z.number().int().nonnegative(),
  languages: z.array(
    z.object({
      language: z.string(),
      lines: z.number().int().nonnegative(),
    }),
  ),
  activity: repositoryActivitySchema,
});

export const repositoryOverviewSchema = z.object({
  repositories: z.array(repositoryOverviewItemSchema),
  page: z.number().int().positive(),
  per_page: z.number().int().positive(),
  has_next: z.boolean(),
});

export const refSchema = z.object({
  name: z.string(),
  oid: z.string(),
  commit_oid: z.string(),
});

export const refsSchema = z.object({
  branches: z.array(refSchema),
  tags: z.array(refSchema),
  size_bytes: z.number().int().nonnegative().nullable(),
  lfs_size_bytes: z.number().int().nonnegative().nullable(),
});

export const treeEntrySchema = z.object({
  name: z.string(),
  path: z.string(),
  oid: z.string(),
  kind: z.enum(["tree", "blob", "symlink", "submodule"]),
  mode: z.number(),
  size: z.number().nullable(),
  lfs_size: z.number().int().nonnegative().nullable(),
});

export const treeSchema = z.object({
  revision: z.string(),
  commit_oid: z.string(),
  commit_timestamp: z.number(),
  commit_count: z.number().int().nonnegative().nullable(),
  path: z.string(),
  entries: z.array(treeEntrySchema),
});

export const blobSchema = z.object({
  revision: z.string(),
  commit_oid: z.string(),
  path: z.string(),
  oid: z.string(),
  size: z.number(),
  binary: z.boolean(),
  too_large: z.boolean(),
  content: z.string().nullable(),
  rendered_html: z.string().nullable(),
});

export const signatureSchema = z.object({
  name: z.string(),
  email: z.string(),
  timestamp: z.number(),
  timezone_offset_minutes: z.number(),
});
export const commitVerificationSchema = z.object({
  verified: z.boolean(),
  reason: z.enum(["verified", "unknown_key", "invalid"]),
  signer: z.string().nullable(),
  fingerprint: z.string().nullable(),
});

export const commitRefSchema = z.object({
  kind: z.enum(["tag", "release"]),
  name: z.string(),
  prerelease: z.boolean(),
  latest: z.boolean(),
  published_at: z.string().nullable(),
  commits_since_previous: z.number().int().nonnegative().nullable(),
});

export const commitSchema = z.object({
  oid: z.string(),
  short_oid: z.string(),
  tree_oid: z.string(),
  parents: z.array(z.string()),
  author: signatureSchema,
  committer: signatureSchema,
  title: z.string(),
  message: z.string(),
  insertions: z.number(),
  deletions: z.number(),
  refs: z.array(commitRefSchema),
  verification: commitVerificationSchema.nullable(),
});

export const historySchema = z.object({
  commits: z.array(commitSchema),
  page: z.number(),
  per_page: z.number(),
  has_next: z.boolean(),
});

export const diffSchema = z.object({
  patch: z.string(),
  truncated: z.boolean(),
});

export const languageStatSchema = z.object({
  language: z.string(),
  files: z.number(),
  code: z.number(),
  comments: z.number(),
  blanks: z.number(),
});

export type Repository = z.infer<typeof repositorySchema>;

// Cache busting is keyed on the timestamp so a replaced icon shows up without
// the endpoint having to forbid caching outright.
export function repositoryIconUrl(
  namespace: string,
  name: string,
  updatedAt: string | null,
): string | null {
  return updatedAt
    ? `/api/v1/repositories/${encodeURIComponent(namespace)}/${encodeURIComponent(
        name,
      )}/icon?v=${encodeURIComponent(updatedAt)}`
    : null;
}

export type RepositoryOverviewItem = z.infer<
  typeof repositoryOverviewItemSchema
>;
export type RepositoryActivity = z.infer<typeof repositoryActivitySchema>;
export type RepositoryRefs = z.infer<typeof refsSchema>;
export type Tree = z.infer<typeof treeSchema>;
export type TreeEntry = z.infer<typeof treeEntrySchema>;
export type Blob = z.infer<typeof blobSchema>;
export type Commit = z.infer<typeof commitSchema>;
export type CommitRef = z.infer<typeof commitRefSchema>;
export type History = z.infer<typeof historySchema>;
export type Diff = z.infer<typeof diffSchema>;
export type LanguageStat = z.infer<typeof languageStatSchema>;
