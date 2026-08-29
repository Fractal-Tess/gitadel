# Dokploy integration fixture

A minimal SvelteKit service used to exercise Gitadel's Dokploy integration with both Dockerfile and Docker Compose resources.

## Local checks

```bash
bun install
bun run check
bun run build
docker build -t gitadel-dokploy-fixture:test .
docker run --rm -p 4179:3000 \
  -e DEPLOYMENT_KIND=local \
  -e FIXTURE_VALUE=local-check \
  -e FIXTURE_REVISION=fixture-v1 \
  gitadel-dokploy-fixture:test
```

Check `http://127.0.0.1:4179/api/health` and `http://127.0.0.1:4179/api/status`. The status response exposes the build marker and the three runtime variables so a deployment can be verified without relying only on Dokploy's status.

`docker-compose.yml` uses the same image and passes the variables through from the Compose environment. Keep that standard filename: Gitadel configures newly created Dokploy Compose resources with `./docker-compose.yml`.

## Manual end-to-end run

1. Push this directory to a temporary Gitadel repository.
2. Connect that Gitadel instance as a Gitea provider in Dokploy.
3. Connect the Dokploy API key under Gitadel account integrations and enable Dokploy on the temporary repository.
4. Create and link a Dockerfile application. Set environment values and a domain, then run **Sync & deploy**. Confirm `/api/status`, change `src/lib/build.ts`, push, and confirm the new build marker appears.
5. Create and link a Compose resource. Sync environment values, confirm a successful Compose deployment, then push another build-marker change and confirm Dokploy records that commit as a successful deployment.
6. Change the application environment and domain list, sync again, and confirm Dokploy reconciles both blocks.
7. Delete the temporary Dokploy applications, Compose resources, project, and Gitadel repository.

## Verified result

Manual end-to-end verification on 2026-08-24 covered:

- Gitea provider repository and branch discovery through Gitadel;
- Dockerfile resource creation, build, HTTPS routing, environment propagation, push-triggered deployment, and domain/environment reconciliation;
- Compose resource creation with Gitea-specific source fields, `docker-compose.yml`, environment propagation, initial deployment, and push-triggered redeployment;
- cleanup of every temporary Dokploy project and resource.

The fixture contains no credentials or environment-specific IDs.
