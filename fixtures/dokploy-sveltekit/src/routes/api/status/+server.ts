import { env } from "$env/dynamic/private";
import { json } from "@sveltejs/kit";
import { BUILD_MARKER } from "$lib/build.js";

export function GET({ request }) {
  return json({
    build_marker: BUILD_MARKER,
    deployment_kind: env.DEPLOYMENT_KIND ?? null,
    fixture_value: env.FIXTURE_VALUE ?? null,
    fixture_revision: env.FIXTURE_REVISION ?? "initial",
    host: request.headers.get("host"),
  });
}
