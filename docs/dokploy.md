# Connect Gitadel to Dokploy

Gitadel implements the Gitea OAuth and repository APIs used by Dokploy. This lets Dokploy discover and clone repositories the authorizing Gitadel account can read.

## Connect the API and repository source

1. In Dokploy, open your profile and create an API key.
2. In Gitadel, open **Account settings → Integrations**, press **Add integration**, and choose Dokploy. Enter a name, the Dokploy URL, and the API key. **Gitadel URL from Dokploy** is the address Dokploy containers use to reach Gitadel; the browser origin is filled in by default, but you can replace it with an internal hostname or IP address.
3. Press **Test connection**. Gitadel enables **Continue** only after Dokploy accepts the URL and API key.
4. Continue to **Repository source**, then choose an existing authorized Dokploy Gitea provider or create one from Gitadel.
5. If you create a provider, Gitadel creates the matching OAuth application and sends its one-time Client ID and Client Secret to Dokploy. Press **Authorize in Dokploy** to finish authorization, then refresh the source status in Gitadel.
6. Wait for the connection card to show **Connected** before adding it to repositories.

The API key lets Gitadel list Dokploy projects and resources, link resources, and trigger deployments. The Gitea provider gives Dokploy repository discovery and clone access through Gitadel OAuth. Both are required, and Gitadel stores the exact Dokploy provider ID used by the connection. Repository setup never falls back to whichever provider happens to list the same repository.

You can complete the same flow from a repository's **Settings → Integrations** page. New connections stay in the setup dialog until their source is authorized. Connections without a ready source are not offered for repository links.

An account or organization can hold multiple named Dokploy connections. Each connection has its own API key and source binding. It can be disabled without deleting its credentials.

### Provider ownership

Choosing an existing provider only records the binding. Gitadel does not own or delete that Dokploy provider or its OAuth application.

Creating a provider from Gitadel makes the source managed. Removing the source or its integration removes the Dokploy provider and the Gitadel OAuth application together. Gitadel blocks direct deletion of the managed OAuth application so the connection cannot be left half-configured. If Dokploy cannot remove its provider, Gitadel keeps the binding and reports the failure instead of discarding the local ownership record.

Deleting or reauthorizing a Dokploy Gitea provider can stop repository discovery and cloning. Gitadel verifies the bound provider before repository setup and reports **Authorization required** when Dokploy no longer lists it as ready.

## Deploy on push without managing webhooks

Dokploy only creates webhooks by itself for GitHub, where its GitHub App installs them. For every other provider, including Gitea, the deploy URL is per application. Each project would otherwise need a webhook added by hand.

Once a repository link is enabled, Gitadel sends push events to that one Dokploy resource. Dokploy remains responsible for branch and watch-path checks.

## Configure a repository deployment

1. Open the repository's **Settings → Integrations** page and add the Dokploy connection you want to use.
2. Open **Configure**. Gitadel groups Dokploy environments by project and shows Applications and Compose resources as cards. The create card comes first at each level.
3. Choose an existing environment or create one inside an existing or new Dokploy project.
4. Choose an existing Application or Compose resource, or create one. New Applications need a name, branch, and optional deployment server. New Compose resources also need a Compose file path.
5. Gitadel connects the resource's Gitea source to this repository. An existing resource already using this repository keeps its source settings. A resource using another source is rejected rather than silently overwritten.
6. Open the exact resource page in Dokploy and finish its build and runtime configuration. Gitadel leaves build type, environment variables, domains, ports, volumes, logs, and deployment history to Dokploy.
7. Return to Gitadel and enable **Deploy on push**. **Deploy now** triggers the linked resource without changing its configuration or push setting.

New resources start with automatic deployment disabled. This avoids a broken deployment between resource creation and the remaining setup in Dokploy. Unlinking removes Gitadel's association and disables automatic deployment, but never deletes the Dokploy resource.

The API key is stored in Gitadel's database and shown masked in configuration. Only the namespace owner can explicitly reveal it, and reveal responses are not cached. Removing a catalog entry also removes its repository links. An unreachable Dokploy instance never fails a push; the outcome is logged.

### Enable repositories explicitly

Repository integrations are opt-in. Link a target before enabling **Deploy on push**; new repository integrations start disabled. Repositories created before explicit configuration existed retain their migrated enabled state until they are reconfigured.

## Trigger deployments with a webhook

To wire a single application by hand instead, open the repository's **Settings** in Gitadel and add the webhook URL supplied by Dokploy. You can optionally configure a signing secret.

Gitadel sends GitHub-style push payloads carrying the pushed commits and their added, modified, and removed paths, so Dokploy's watch paths apply. When a secret is configured, deliveries include an HMAC-SHA256 signature. Use the repository settings page to enable, disable, edit, ping, or remove the webhook.

## Verification fixture

`fixtures/dokploy-sveltekit/` is the manual end-to-end fixture for Dockerfile and Docker Compose deployments. Its README contains local smoke commands, the live Dokploy procedure, verified behaviors, and cleanup steps. It deliberately contains no credentials or instance-specific identifiers.
