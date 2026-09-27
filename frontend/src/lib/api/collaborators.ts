import { z } from "zod";

export const collaboratorRoleSchema = z.enum(["read", "write"]);

export const collaboratorSchema = z.object({
  username: z.string(),
  role: collaboratorRoleSchema,
  created_at: z.string(),
});

export type CollaboratorRole = z.infer<typeof collaboratorRoleSchema>;
export type Collaborator = z.infer<typeof collaboratorSchema>;
