<script lang="ts">
  import { page } from "$app/state";
  import Building2 from "@lucide/svelte/icons/building-2";
  import FolderGit2 from "@lucide/svelte/icons/folder-git-2";
  import ShieldAlert from "@lucide/svelte/icons/shield-alert";
  import UserRound from "@lucide/svelte/icons/user-round";

  import { Button } from "$lib/components/ui/button/index.js";
  import BrandMark from "$lib/components/brand-mark.svelte";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();

  const consentToken = page.url.searchParams.get("consent_token");
  const application = page.url.searchParams.get("application");
  const username = page.url.searchParams.get("username");
  const scopes = (page.url.searchParams.get("scope") ?? "")
    .split(" ")
    .filter(Boolean);

  const scopeDetails = {
    "read:repository": {
      icon: FolderGit2,
      label: "Repositories",
      description: "View and clone repositories you can access",
    },
    "read:user": {
      icon: UserRound,
      label: "Account",
      description: "Read your account identity",
    },
    "read:organization": {
      icon: Building2,
      label: "Organizations",
      description: "Read your organization memberships",
    },
  } as const;

  const grantedScopes = scopes
    .filter((scope) => scope in scopeDetails)
    .map((scope) => ({ name: scope, ...scopeDetails[scope as keyof typeof scopeDetails] }));
  const unknownScopes = scopes.filter((scope) => !(scope in scopeDetails));
  const valid = Boolean(consentToken && application && username);
  const initials = (username ?? "?")
    .split(/[-_.]/)
    .map((part) => part[0])
    .filter(Boolean)
    .slice(0, 2)
    .join("")
    .toUpperCase();
</script>

<svelte:head>
  <title>
    Authorize {application ?? "application"} ·
    {app.instance?.site_name ?? "Gitadel"}
  </title>
</svelte:head>

<main class="grid min-h-screen place-items-center bg-background px-5 py-12">
  <section class="w-full max-w-md rounded-md border bg-card/25 p-6 shadow-sm">
    <div class="flex items-center gap-2">
      <BrandMark />
      <span class="text-sm font-bold tracking-[-0.035em]">
        {app.instance?.site_name ?? "GITADEL"}
      </span>
    </div>

    {#if valid}
      <p
        class="mt-8 text-xs font-medium uppercase tracking-wider text-muted-foreground"
      >
        OAuth authorization
      </p>
      <h1 class="mt-2 text-2xl font-semibold">
        Authorize {application}
      </h1>

      <div class="mt-5 flex items-center gap-3 rounded-md border bg-background p-3">
        <span
          class="grid size-9 shrink-0 place-items-center rounded-full bg-primary/10 text-xs font-semibold text-primary"
        >
          {initials}
        </span>
        <div class="min-w-0 text-sm">
          <p class="font-medium">{username}</p>
          <p class="truncate text-muted-foreground">
            Signed in — authorizing on this account's behalf
          </p>
        </div>
      </div>

      <p class="mt-6 text-sm text-muted-foreground">
        {application} is requesting permission to:
      </p>
      <ul class="mt-3 grid gap-2">
        {#each grantedScopes as detail (detail.name)}
          <li class="flex items-start gap-3 rounded-md border bg-background p-3">
            <span
              class="grid size-8 shrink-0 place-items-center rounded-md border bg-muted"
            >
              <detail.icon class="size-4 text-muted-foreground" />
            </span>
            <div class="min-w-0 text-sm">
              <p class="font-medium">{detail.label}</p>
              <p class="text-muted-foreground">{detail.description}</p>
            </div>
          </li>
        {/each}
        {#each unknownScopes as scope (scope)}
          <li class="flex items-start gap-3 rounded-md border bg-background p-3">
            <span
              class="grid size-8 shrink-0 place-items-center rounded-md border bg-muted"
            >
              <ShieldAlert class="size-4 text-muted-foreground" />
            </span>
            <div class="min-w-0 text-sm">
              <p class="font-medium">{scope}</p>
              <p class="text-muted-foreground">Access your account</p>
            </div>
          </li>
        {/each}
      </ul>

      <form class="mt-6 grid grid-cols-2 gap-3" method="POST" action="/login/oauth/authorize">
        <input type="hidden" name="consent_token" value={consentToken} />
        <Button type="submit" name="decision" value="deny" variant="outline">
          Cancel
        </Button>
        <Button type="submit" name="decision" value="allow">
          Authorize
        </Button>
      </form>

      <p class="mt-5 text-xs leading-relaxed text-muted-foreground">
        You can revoke this access at any time by deleting the OAuth application
        in your account settings.
      </p>
    {:else}
      <p
        class="mt-8 text-xs font-medium uppercase tracking-wider text-muted-foreground"
      >
        OAuth authorization
      </p>
      <h1 class="mt-2 text-2xl font-semibold">Invalid request</h1>
      <p class="mt-2 text-sm text-muted-foreground">
        This authorization link is missing or incomplete. Start again from the
        application that sent you here.
      </p>
      <Button class="mt-6" variant="outline" onclick={() => history.back()}>
        Go back
      </Button>
    {/if}
  </section>
</main>
