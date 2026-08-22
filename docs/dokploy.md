# Connect Gitadel to Dokploy

Gitadel implements the Gitea OAuth and repository APIs used by Dokploy. This lets Dokploy discover and clone repositories the authorizing Gitadel account can read.

## Connect the provider

1. In Dokploy, open **Settings → Git Providers**, add a **Gitea** provider, and copy its Redirect URI.
2. In Gitadel, open **Account settings → Applications** and create an OAuth application with that exact Redirect URI.
3. Copy the generated Client ID and Client Secret into Dokploy.
4. Use Gitadel's public URL as the Gitea URL. If the services share a private network, optionally set Dokploy's Internal URL as well.
5. Finish authorization in the Gitadel window.

Dokploy can now list accessible repositories and branches and clone them with a repository-scoped OAuth token. Deleting the OAuth application in Gitadel revokes that access immediately.

## Trigger deployments on push

In Gitadel, open the repository's **Settings** and add the webhook URL supplied by Dokploy. You can optionally configure a signing secret.

Gitadel sends GitHub-style push payloads. When a secret is configured, deliveries include an HMAC-SHA256 signature. Use the repository settings page to enable, disable, edit, ping, or remove the webhook.
