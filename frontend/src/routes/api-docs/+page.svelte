<script lang="ts">
  import { onMount } from "svelte";
  import FileCode from "@lucide/svelte/icons/file-code-2";

  import * as Alert from "$lib/components/ui/alert/index.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  // The viewer's self-contained browser build, emitted as its own asset and
  // fetched only on this page. A relative path bypasses the package's export
  // map, which does not list it.
  import scalarScript from "../../../node_modules/@scalar/api-reference/dist/browser/standalone.js?url";

  type ScalarInstance = { destroy?: () => void };
  type ScalarGlobal = {
    createApiReference: (
      element: HTMLElement,
      configuration: Record<string, unknown>,
    ) => ScalarInstance;
  };

  const SPEC_URL = "/api/v1/openapi.json";
  const app = useAppState();

  let container: HTMLDivElement | undefined = $state();
  let error = $state<string | null>(null);

  function loadScript(): Promise<ScalarGlobal> {
    const existing = (window as unknown as { Scalar?: ScalarGlobal }).Scalar;
    if (existing) return Promise.resolve(existing);
    return new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = scalarScript;
      script.onload = () => {
        const loaded = (window as unknown as { Scalar?: ScalarGlobal }).Scalar;
        if (loaded) resolve(loaded);
        else reject(new Error("The API viewer did not load."));
      };
      script.onerror = () => reject(new Error("The API viewer did not load."));
      document.head.append(script);
    });
  }

  onMount(() => {
    let instance: ScalarInstance | null = null;
    let cancelled = false;
    void loadScript()
      .then((scalar) => {
        if (cancelled || !container) return;
        instance = scalar.createApiReference(container, {
          url: SPEC_URL,
          // Fonts, the request proxy, telemetry, and the AI, MCP, and SDK
          // features all rely on Scalar's hosted services, which this
          // instance does not talk to (and its content security policy
          // would block anyway).
          withDefaultFonts: false,
          proxyUrl: "",
          telemetry: false,
          showDeveloperTools: "never",
          agent: { disabled: true },
          mcp: { disabled: true },
          documentDownloadType: "json",
          darkMode: document.documentElement.classList.contains("dark"),
          hideDarkModeToggle: true,
          defaultHttpClient: { targetKey: "shell", clientKey: "curl" },
          metaData: { title: "Gitadel API" },
        });
      })
      .catch((caught: unknown) => {
        error =
          caught instanceof Error ? caught.message : "The API viewer did not load.";
      });
    return () => {
      cancelled = true;
      instance?.destroy?.();
    };
  });
</script>

<svelte:head>
  <title>API · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<div class="mx-auto max-w-7xl px-5 pt-8 lg:px-8">
  <div class="flex items-start justify-between gap-4">
    <div class="flex items-start gap-3">
      <FileCode class="mt-0.5 size-5 text-foreground/70" />
      <div>
        <h1 class="text-lg font-semibold tracking-tight">API</h1>
        <p class="mt-1.5 text-sm text-muted-foreground">
          Every endpoint under <code class="font-mono text-xs">/api/v1</code>.
          Requests from this page use your session; scripts send an API token
          as <code class="font-mono text-xs">Authorization: Bearer …</code>.
        </p>
      </div>
    </div>
    <a
      class="shrink-0 font-mono text-xs text-muted-foreground underline-offset-4 hover:underline"
      href={SPEC_URL}>openapi.json</a
    >
  </div>
  {#if error}
    <Alert.Root variant="destructive" class="mt-6">
      <Alert.Description>{error}</Alert.Description>
    </Alert.Root>
  {/if}
</div>
<div bind:this={container} class="api-reference"></div>
