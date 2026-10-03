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

/**
 * One address a user writes commits under. `pending` addresses are waiting
 * for their emailed link and count for nothing yet; `not_required` ones were
 * accepted as entered because the instance cannot send mail.
 */
export const commitEmailSchema = z.object({
  email: z.string(),
  verified_at: z.string().nullable(),
  verification: z.enum(["verified", "pending", "not_required"]),
});

/** How the signed-in user's commits are recognised and authored. */
export const commitIdentitySchema = z.object({
  name: z.string().nullable(),
  primary_email: z.string().nullable(),
  author_name: z.string(),
  author_email: z.string(),
  emails: z.array(commitEmailSchema),
  account_email: z.string().nullable(),
  verification_required: z.boolean(),
});

export type CommitEmail = z.infer<typeof commitEmailSchema>;
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
