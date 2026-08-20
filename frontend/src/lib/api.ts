import { z } from 'zod';

export class ApiFailure extends Error {
	constructor(
		message: string,
		readonly status: number,
		readonly code: string
	) {
		super(message);
	}
}

const errorSchema = z.object({
	error: z.object({
		code: z.string(),
		message: z.string()
	})
});

const userSchema = z.object({
	id: z.uuid(),
	username: z.string(),
	is_admin: z.boolean()
});

export const authStatusSchema = z.object({
	setup_required: z.boolean(),
	authenticated: z.boolean(),
	user: userSchema.nullable()
});

export const authResponseSchema = z.object({ user: userSchema });

export const instanceSettingsSchema = z.object({
	site_name: z.string(),
	site_description: z.string().nullable(),
	default_repository_visibility: z.enum(['public', 'private']),
	updated_at: z.string()
});

export const invitationSchema = z.object({
	token: z.string(),
	expires_at: z.string()
});

export const sshKeySchema = z.object({
	id: z.uuid(),
	name: z.string(),
	fingerprint: z.string(),
	public_key: z.string(),
	created_at: z.string(),
	last_used_at: z.string().nullable()
});

export const tokenSchema = z.object({
	id: z.uuid(),
	name: z.string(),
	scopes: z.array(z.enum(['read', 'write', 'ssh_keys'])),
	expires_at: z.string().nullable(),
	created_at: z.string(),
	last_used_at: z.string().nullable()
});

export const createdTokenSchema = z.object({
	token: z.string(),
	details: tokenSchema
});

export const passkeySchema = z.object({
	id: z.uuid(),
	name: z.string(),
	created_at: z.string(),
	last_used_at: z.string().nullable()
});

export const organizationSchema = z.object({
	id: z.uuid(),
	slug: z.string(),
	display_name: z.string(),
	role: z.enum(['owner', 'member'])
});

export const memberSchema = z.object({
	username: z.string(),
	role: z.enum(['owner', 'member']),
	created_at: z.string()
});

export const auditEventSchema = z.object({
	id: z.number(),
	actor_user_id: z.uuid().nullable(),
	action: z.string(),
	target: z.string().nullable(),
	created_at: z.string()
});

export const webauthnCreationSchema = z.object({
	challenge_id: z.string(),
	options: z.object({ publicKey: z.record(z.string(), z.unknown()) })
});

export const webauthnRequestSchema = z.object({
	challenge_id: z.string(),
	options: z.object({ publicKey: z.record(z.string(), z.unknown()) })
});

export const repositorySchema = z.object({
	id: z.uuid(),
	namespace: z.string(),
	name: z.string(),
	description: z.string().nullable(),
	visibility: z.enum(['public', 'private']),
	object_format: z.enum(['sha1', 'sha256']),
	default_branch: z.string(),
	archived_at: z.string().nullable(),
	created_at: z.string(),
	updated_at: z.string(),
	favorited: z.boolean(),
	ssh_clone_url: z.string()
});

export const refSchema = z.object({
	name: z.string(),
	oid: z.string()
});

export const refsSchema = z.object({
	branches: z.array(refSchema),
	tags: z.array(refSchema)
});

export const treeEntrySchema = z.object({
	name: z.string(),
	path: z.string(),
	oid: z.string(),
	kind: z.enum(['tree', 'blob', 'symlink', 'submodule']),
	mode: z.number(),
	size: z.number().nullable()
});

export const treeSchema = z.object({
	revision: z.string(),
	commit_oid: z.string(),
	path: z.string(),
	entries: z.array(treeEntrySchema)
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
	rendered_html: z.string().nullable()
});

export const signatureSchema = z.object({
	name: z.string(),
	email: z.string(),
	timestamp: z.number(),
	timezone_offset_minutes: z.number()
});

export const commitSchema = z.object({
	oid: z.string(),
	short_oid: z.string(),
	tree_oid: z.string(),
	parents: z.array(z.string()),
	author: signatureSchema,
	committer: signatureSchema,
	title: z.string(),
	message: z.string()
});

export const historySchema = z.object({
	commits: z.array(commitSchema),
	page: z.number(),
	per_page: z.number(),
	has_next: z.boolean()
});

export const diffSchema = z.object({
	patch: z.string(),
	truncated: z.boolean()
});

export const languageStatSchema = z.object({
	language: z.string(),
	files: z.number(),
	code: z.number(),
	comments: z.number(),
	blanks: z.number()
});

export type AuthStatus = z.infer<typeof authStatusSchema>;
export type User = z.infer<typeof userSchema>;
export type InstanceSettings = z.infer<typeof instanceSettingsSchema>;
export type SshKey = z.infer<typeof sshKeySchema>;
export type ApiToken = z.infer<typeof tokenSchema>;
export type PasskeySummary = z.infer<typeof passkeySchema>;
export type Organization = z.infer<typeof organizationSchema>;
export type Member = z.infer<typeof memberSchema>;
export type AuditEvent = z.infer<typeof auditEventSchema>;
export type Repository = z.infer<typeof repositorySchema>;
export type RepositoryRefs = z.infer<typeof refsSchema>;
export type Tree = z.infer<typeof treeSchema>;
export type TreeEntry = z.infer<typeof treeEntrySchema>;
export type Blob = z.infer<typeof blobSchema>;
export type Commit = z.infer<typeof commitSchema>;
export type History = z.infer<typeof historySchema>;
export type Diff = z.infer<typeof diffSchema>;
export type LanguageStat = z.infer<typeof languageStatSchema>;

export async function requestJson<T>(
	path: string,
	schema: z.ZodType<T>,
	init: RequestInit = {}
): Promise<T> {
	const headers = new Headers(init.headers);
	headers.set('accept', 'application/json');
	if (init.body !== undefined) {
		headers.set('content-type', 'application/json');
	}
	const response = await fetch(path, {
		...init,
		headers,
		credentials: 'same-origin'
	});
	const payload: unknown = await response.json().catch(() => null);
	if (!response.ok) {
		const parsed = errorSchema.safeParse(payload);
		throw new ApiFailure(
			parsed.success ? parsed.data.error.message : `Request failed with status ${response.status}.`,
			response.status,
			parsed.success ? parsed.data.error.code : 'request_failed'
		);
	}
	const parsed = schema.safeParse(payload);
	if (!parsed.success) {
		throw new ApiFailure('The server returned an invalid response.', response.status, 'invalid_response');
	}
	return parsed.data;
}

export async function requestEmpty(path: string, init: RequestInit = {}): Promise<void> {
	const headers = new Headers(init.headers);
	headers.set('accept', 'application/json');
	if (init.body !== undefined) {
		headers.set('content-type', 'application/json');
	}
	const response = await fetch(path, {
		...init,
		headers,
		credentials: 'same-origin'
	});
	if (!response.ok) {
		const payload: unknown = await response.json().catch(() => null);
		const parsed = errorSchema.safeParse(payload);
		throw new ApiFailure(
			parsed.success ? parsed.data.error.message : `Request failed with status ${response.status}.`,
			response.status,
			parsed.success ? parsed.data.error.code : 'request_failed'
		);
	}
}

export function jsonBody(value: unknown): string {
	return JSON.stringify(value);
}
