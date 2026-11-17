# Workspace responsibilities

Gitadel uses short crate directories and service-prefixed package names.
The server remains `gitadel`, and the remote client remains `gtd`.

Keep the split at the level of existing responsibilities:

| Crate | Responsibility |
| --- | --- |
| `config` | Settings, defaults and command-line configuration |
| `core` | Shared filesystem, outbound-network and schedule helpers |
| `db` | Database connections, entities and migrations |
| `secrets` | Stored credential encryption |
| `mail` | Mail transport and delivery |
| `storage` | Blob backends, LFS and registry persistence |
| `identity` | Accounts, authentication and authorization |
| `instance` | Installation settings, branding and administration |
| `repositories` | Git operations and repository services |
| `actions` | Workflow planning and execution |
| `integrations` | External provider clients and deployment operations |
| `backups` | Archive creation, inspection, restore and backup providers |
| `api` | HTTP/protocol adapters, transport DTOs, registered routes and OpenAPI |
| `server` | Executable, service construction and process lifecycle |
| `cli` | Remote command-line client |

This is an incremental extraction. Moving the original modules into `api`
was an intermediate step; it does not complete the responsibility split.
Extract service operations with their tests, leaving request parsing and HTTP
responses in `api`. Domain crates must not depend on `api` or `server`.
Keep issues, releases and webhooks with repository services for now rather
than creating a separate crate for every feature.

The current refinement and its acceptance rules are in
[API-BOUNDARIES.md](API-BOUNDARIES.md).

The current extraction moves the main credential, workflow, Git, storage,
integration and backup operations into these libraries. API still contains
diff/blame operations, repository cache orchestration, account administration,
and passkey/OAuth and enrollment orchestration. These are remaining boundaries
to refine; the workspace layout alone does not imply complete separation.

Split oversized state objects where they cross those boundaries. Pass the
database, storage handles and configuration a service actually needs. Use a
narrow interface only where a real dependency cycle requires one; do not
introduce a global service container or a new event framework.

Preserve configuration, ports, authentication, routes, package versions,
database migrations, Git refs, archive compatibility and stored objects.
Compare the registered OpenAPI document, run workspace and frontend checks,
exercise the affected behavior in a throwaway instance, and build the actual
production package before deployment.
