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
