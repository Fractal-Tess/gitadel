import { env } from "$env/dynamic/private";

export function load() {
  return {
    deploymentKind: env.DEPLOYMENT_KIND ?? "unset",
    fixtureValue: env.FIXTURE_VALUE ?? "unset",
    fixtureRevision: env.FIXTURE_REVISION ?? "initial",
  };
}
