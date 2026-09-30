import { z } from "zod";

import {
  repositoryActivitySchema,
  repositoryOverviewItemSchema,
} from "$lib/api/repositories.js";

/**
 * Commits per day on a profile. A person's counts the commits they wrote in
 * any repository the viewer can read; an organization's counts every commit
 * in its own repositories.
 */
export const namespaceActivitySchema = repositoryActivitySchema.extend({
  scope: z.enum(["person", "organization"]).default("organization"),
  repository_count: z.number().int().nonnegative(),
});

/** How the signed-in user's commits are recognised and authored. */
export const commitIdentitySchema = z.object({
  name: z.string().nullable(),
  primary_email: z.string().nullable(),
  author_name: z.string(),
  author_email: z.string(),
  emails: z.array(z.string()),
  account_email: z.string().nullable(),
});

export type CommitIdentity = z.infer<typeof commitIdentitySchema>;

export const pinnedRepositoriesSchema = z.object({
  repositories: z.array(repositoryOverviewItemSchema),
  limit: z.number().int().positive(),
});

export type NamespaceActivity = z.infer<typeof namespaceActivitySchema>;
export type PinnedRepositories = z.infer<typeof pinnedRepositoriesSchema>;

/** The commits behind one day of the activity graph, by repository. */
export const namespaceDaySchema = z.object({
  date: z.string(),
  total_commits: z.number().int().nonnegative(),
  repositories: z.array(
    z.object({
      namespace: z.string(),
      name: z.string(),
      commits: z.array(
        z.object({
          oid: z.string(),
          short_oid: z.string(),
          title: z.string(),
          author_name: z.string(),
          timestamp: z.number().int(),
        }),
      ),
    }),
  ),
});

export type NamespaceDay = z.infer<typeof namespaceDaySchema>;
