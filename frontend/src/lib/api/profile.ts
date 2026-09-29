import { z } from "zod";

import {
  repositoryActivitySchema,
  repositoryOverviewItemSchema,
} from "$lib/api/repositories.js";

/** Commits per day across a namespace's repositories the viewer can read. */
export const namespaceActivitySchema = repositoryActivitySchema.extend({
  repository_count: z.number().int().nonnegative(),
});

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
