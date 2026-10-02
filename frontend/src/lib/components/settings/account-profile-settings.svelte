<script lang="ts">
  import BookOpen from "@lucide/svelte/icons/book-open";
  import UserRound from "@lucide/svelte/icons/user-round";

  import AccountAppearanceSettings from "$lib/components/settings/account-appearance-settings.svelte";
  import AccountAvatarSettings from "$lib/components/settings/account-avatar-settings.svelte";
  import AccountCommitIdentitySettings from "$lib/components/settings/account-commit-identity-settings.svelte";
  import AccountEmailSettings from "$lib/components/settings/account-email-settings.svelte";
  import AccountNotificationSettings from "$lib/components/settings/account-notification-settings.svelte";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import type { ProfileSettingsState } from "$lib/settings/account/profile-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  let { profile }: { profile: ProfileSettingsState } = $props();
  const app = useAppState();
</script>

<AccountAvatarSettings />

<section
  class="grid gap-5 py-(--card-spacing) md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10"
  aria-labelledby="username-heading"
>
  <Card.Header class="flex flex-row items-start gap-3">
    <UserRound class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
    <div>
      <Card.Title id="username-heading" role="heading" aria-level={2}>
        Username
      </Card.Title>
      <Card.Description class="mt-1 max-w-xs leading-5">
        Your sign-in name and personal repository namespace.
      </Card.Description>
    </div>
  </Card.Header>

  <Card.Content>
    <form
      class="grid max-w-2xl gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void profile.updateUsername();
      }}
    >
      <Field.Field>
        <Field.Label for="account-username">Username</Field.Label>
        <Input
          id="account-username"
          autocomplete="username"
          bind:value={profile.username}
          maxlength={39}
          disabled={profile.working}
          required
        />
        <Field.Description>
          Press Enter to save. Repository URLs change with your username, so
          update existing Git remotes afterward.
        </Field.Description>
      </Field.Field>
    </form>
  </Card.Content>
</section>

<section
  class="grid gap-5 py-(--card-spacing) md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10"
  aria-labelledby="repository-defaults-heading"
>
  <Card.Header class="flex flex-row items-start gap-3">
    <BookOpen class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
    <div>
      <Card.Title
        id="repository-defaults-heading"
        role="heading"
        aria-level={2}
      >
        Repository defaults
      </Card.Title>
      <Card.Description class="mt-1 max-w-xs leading-5">
        Choose the initial visibility for repositories you create.
      </Card.Description>
    </div>
  </Card.Header>

  <Card.Content>
    <Field.Field class="max-w-2xl">
      <Field.Label for="account-default-repository-visibility">
        Default visibility
      </Field.Label>
      <Select.Root
        type="single"
        value={profile.defaultRepositoryVisibility}
        onValueChange={(value) => {
          if (value === "public" || value === "private") {
            void profile.updateRepositoryVisibility(value);
          }
        }}
      >
        <Select.Trigger
          id="account-default-repository-visibility"
          class="w-full"
          disabled={profile.working}
        >
          {profile.defaultRepositoryVisibility === "private"
            ? "Private"
            : "Public"}
        </Select.Trigger>
        <Select.Content>
          <Select.Item value="private">Private</Select.Item>
          <Select.Item value="public">Public</Select.Item>
        </Select.Content>
      </Select.Root>
      <Field.Description>
        You can choose a different visibility when creating a repository.
      </Field.Description>
    </Field.Field>
  </Card.Content>
</section>

<AccountCommitIdentitySettings />
<AccountAppearanceSettings />

{#if app.authStatus?.email_enabled}
  <AccountEmailSettings />
  <AccountNotificationSettings />
{/if}
