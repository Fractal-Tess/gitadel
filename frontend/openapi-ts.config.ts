import { defineConfig } from '@hey-api/openapi-ts';

export default defineConfig({
  input: process.env.GITADEL_OPENAPI_INPUT ?? '../api/openapi.json',
  output: { path: process.env.GITADEL_SDK_OUTPUT ?? 'src/lib/api/generated', postProcess: [] },
  plugins: ['@hey-api/typescript', '@hey-api/sdk', '@hey-api/client-fetch'],
});
