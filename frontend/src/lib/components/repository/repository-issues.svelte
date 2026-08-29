<script lang="ts">
  import { resolve } from "$app/paths";
  import {
    ArrowLeft,
    CheckCircle2,
    CircleDot,
    MessageSquare,
    Pencil,
    Plus,
    Search,
    Tag,
    Trash2,
    UserRound,
  } from "lucide-svelte";

  import {
    avatarUrl,
    type Issue,
    type IssueComment,
    type IssueUser,
  } from "$lib/api.js";
  import * as Avatar from "$lib/components/ui/avatar/index.js";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import IssueComposerDialog from "$lib/components/repository/issue-composer-dialog.svelte";
  import IssueLabelDialog from "$lib/components/repository/issue-label-dialog.svelte";
  import LabelChip from "$lib/components/repository/label-chip.svelte";
  import { trustedHtml } from "$lib/repository/format.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();
  const dateFormatter = new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });

  let filterState = $state<"open" | "closed">("open");
  let search = $state("");
  let labelFilter = $state<string | null>(null);
  let composerOpen = $state(false);
  let composerIssue = $state<Issue | null>(null);
  let labelsOpen = $state(false);
  let editingComment = $state<{ issueNumber: number; id: string } | null>(null);
  let commentDrafts = $state<Record<number, string>>({});
  let deleteCommentDialogOpen = $state(false);
  let pendingDeleteComment = $state<{ issueNumber: number; id: string } | null>(
    null,
  );
  let deleteIssueDialogOpen = $state(false);
  let pendingDeleteIssue = $state<number | null>(null);

  const filteredIssues = $derived.by(() => {
    const query = search.trim().toLocaleLowerCase();
    return repository.issues.filter(
      (issue) =>
        issue.state === filterState &&
        (!labelFilter ||
          issue.labels.some((label) => label.id === labelFilter)) &&
        (!query ||
          issue.title.toLocaleLowerCase().includes(query) ||
          issue.body.toLocaleLowerCase().includes(query) ||
          String(issue.number) === query.replace(/^#/, "")),
    );
  });

  async function toggleIssueState(issue: Issue) {
    try {
      await repository.updateIssue(issue.number, {
        state: issue.state === "open" ? "closed" : "open",
      });
    } catch {
      // The page-level error explains the failure.
    }
  }

  async function submitComment(event: SubmitEvent) {
    event.preventDefault();
    const issue = repository.selectedIssue;
    if (!issue) return;
    const commentDraft = commentDrafts[issue.number] ?? "";
    try {
      if (editingComment?.issueNumber === issue.number) {
        await repository.updateIssueComment(
          issue.number,
          editingComment.id,
          commentDraft,
        );
      } else {
        await repository.createIssueComment(issue.number, commentDraft);
      }
      commentDrafts = { ...commentDrafts, [issue.number]: "" };
      editingComment = null;
    } catch {
      // The page-level error keeps the comment draft available.
    }
  }

  function openComposer() {
    if (!repository.authStatus?.authenticated) {
      const returnTo = encodeURIComponent(
        `/${repository.namespace}/${repository.name}?view=issues`,
      );
      window.location.assign(`/login?returnTo=${returnTo}`);
      return;
    }
    composerIssue = null;
    composerOpen = true;
  }

  function issueDate(value: string) {
    return dateFormatter.format(new Date(value));
  }

  function beginCommentEdit(issueNumber: number, comment: IssueComment) {
    editingComment = { issueNumber, id: comment.id };
    commentDrafts = { ...commentDrafts, [issueNumber]: comment.body };
  }

  function beginIssueEdit(issue: Issue) {
    composerIssue = issue;
    composerOpen = true;
  }
</script>

{#snippet userAvatar(
  account: IssueUser,
  externalAuthor: Issue["external_author"],
  size = "size-8",
)}
  <Avatar.Root class={size}>
    {#if !externalAuthor && avatarUrl(account.id, account.avatar_updated_at)}
      <Avatar.Image
        src={avatarUrl(account.id, account.avatar_updated_at) ?? undefined}
        alt=""
      />
    {/if}
    <Avatar.Fallback class="text-[10px] font-medium uppercase">
      {(externalAuthor?.username ?? account.username).slice(0, 2)}
    </Avatar.Fallback>
  </Avatar.Root>
{/snippet}

{#snippet authorName(
  account: IssueUser,
  externalAuthor: Issue["external_author"],
)}
  {#if externalAuthor}
    <a
      class="font-medium text-foreground hover:underline"
      href={externalAuthor.profile_url}
      target="_blank"
      rel="noreferrer">{externalAuthor.username}</a
    >
  {:else}
    <strong class="font-medium text-foreground">{account.username}</strong>
  {/if}
{/snippet}

<div class="mx-auto max-w-6xl">
  {#if repository.issueNumber && !repository.selectedIssue}
    <div class="py-20 text-center text-sm text-muted-foreground">
      {#if repository.error}
        <p>This issue could not be loaded.</p>
        <Button
          variant="link"
          class="mt-2"
          onclick={() => repository.selectIssue(null)}>Back to issues</Button
        >
      {:else}
        <p>Loading issue…</p>
      {/if}
    </div>
  {:else if repository.issueNumber && repository.selectedIssue}
    {@const issue = repository.selectedIssue}
    <Button
      variant="ghost"
      size="sm"
      class="-ml-2 mb-4 gap-2 text-muted-foreground"
      onclick={() => repository.selectIssue(null)}
    >
      <ArrowLeft class="size-4" />Back to issues
    </Button>

    <header class="border-b pb-5">
      <div class="flex flex-wrap items-start justify-between gap-4">
        <div class="min-w-0">
          <h1 class="text-2xl font-semibold tracking-tight">
            {issue.title}
            <span class="ml-2 font-normal text-muted-foreground"
              >#{issue.number}</span
            >
          </h1>
          <div
            class="mt-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground"
          >
            <Badge
              class={issue.state === "open"
                ? "gap-1.5 bg-emerald-600 text-white"
                : "gap-1.5 bg-violet-600 text-white"}
            >
              {#if issue.state === "open"}<CircleDot
                  class="size-3.5"
                />Open{:else}<CheckCircle2 class="size-3.5" />Closed{/if}
            </Badge>
            <span>
              {@render authorName(issue.author, issue.external_author)}
              opened this issue on {issueDate(issue.created_at)}
            </span>
            {#if issue.external_url}
              <a href={issue.external_url} target="_blank" rel="noreferrer">
                <Badge variant="outline">Imported from GitHub</Badge>
              </a>
            {/if}
            <span class="whitespace-nowrap"
              >· {issue.comment_count} comment{issue.comment_count === 1
                ? ""
                : "s"}</span
            >
          </div>
        </div>
        {#if issue.can_edit && composerIssue?.number !== issue.number}
          <Button
            variant="outline"
            size="sm"
            class="gap-2"
            onclick={() => beginIssueEdit(issue)}
          >
            <Pencil class="size-3.5" />Edit
          </Button>
        {/if}
      </div>
    </header>

    <div class="grid gap-7 py-7 lg:grid-cols-[minmax(0,1fr)_14rem]">
      <div class="min-w-0 space-y-5">
        <article class="overflow-hidden rounded-md border">
          <header
            class="flex items-center gap-3 border-b bg-muted/20 px-4 py-3 text-sm"
          >
            {@render userAvatar(issue.author, issue.external_author)}
            <span>
              {@render authorName(issue.author, issue.external_author)}
              opened this issue on {issueDate(issue.created_at)}
            </span>
          </header>
          {#if issue.rendered_body}
            <div
              class="prose max-w-none p-5 text-sm prose-code:before:content-none prose-code:after:content-none dark:prose-invert"
              {@attach trustedHtml(issue.rendered_body)}
            ></div>
          {:else}
            <p class="p-5 text-sm italic text-muted-foreground">
              No description provided.
            </p>
          {/if}
        </article>

        {#each repository.issueComments as comment (comment.id)}
          <article class="overflow-hidden rounded-md border">
            <header
              class="flex items-center gap-3 border-b bg-muted/20 px-4 py-3 text-sm"
            >
              {@render userAvatar(comment.author, comment.external_author)}
              <span class="min-w-0 flex-1 truncate">
                {@render authorName(comment.author, comment.external_author)}
                commented on {issueDate(comment.created_at)}
              </span>
              {#if comment.can_edit}
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Edit comment"
                  onclick={() => beginCommentEdit(issue.number, comment)}
                >
                  <Pencil class="size-3" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  class="text-muted-foreground hover:text-destructive"
                  aria-label="Delete comment"
                  onclick={() => {
                    pendingDeleteComment = {
                      issueNumber: issue.number,
                      id: comment.id,
                    };
                    deleteCommentDialogOpen = true;
                  }}
                >
                  <Trash2 class="size-3" />
                </Button>
              {/if}
            </header>
            <div
              class="prose max-w-none p-5 text-sm prose-code:before:content-none prose-code:after:content-none dark:prose-invert"
              {@attach trustedHtml(comment.rendered_body)}
            ></div>
          </article>
        {/each}

        {#if issue.external_url}
          <div
            class="rounded-md border p-5 text-center text-sm text-muted-foreground"
          >
            This issue is synchronized from GitHub.
            <a
              class="font-medium text-foreground underline-offset-4 hover:underline"
              href={issue.external_url}
              target="_blank"
              rel="noreferrer">Open it on GitHub</a
            >
            to comment or change its state.
          </div>
        {:else if repository.authStatus?.authenticated}
          <form
            class="overflow-hidden rounded-md border"
            onsubmit={submitComment}
          >
            <Textarea
              bind:value={commentDrafts[issue.number]}
              class="min-h-36 rounded-none border-0 font-mono text-sm focus-visible:ring-0"
              maxlength={1_000_000}
              placeholder="Leave a comment in Markdown…"
              required
            />
            <div
              class="flex flex-wrap items-center justify-between gap-3 border-t bg-muted/15 p-3"
            >
              {#if issue.can_edit}
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={repository.issuePending}
                  onclick={() => void toggleIssueState(issue)}
                >
                  {issue.state === "open" ? "Close issue" : "Reopen issue"}
                </Button>
              {:else}
                <span></span>
              {/if}
              <div class="flex gap-2">
                {#if editingComment?.issueNumber === issue.number}
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onclick={() => {
                      editingComment = null;
                      commentDrafts = {
                        ...commentDrafts,
                        [issue.number]: "",
                      };
                    }}>Cancel edit</Button
                  >
                {/if}
                <Button
                  type="submit"
                  size="sm"
                  disabled={repository.commentPending ||
                    !(commentDrafts[issue.number] ?? "").trim()}
                >
                  {repository.commentPending
                    ? "Saving…"
                    : editingComment?.issueNumber === issue.number
                      ? "Update comment"
                      : "Comment"}
                </Button>
              </div>
            </div>
          </form>
        {:else}
          {@const returnTo = encodeURIComponent(
            `/${repository.namespace}/${repository.name}?view=issues&issue=${issue.number}`,
          )}
          <div
            class="rounded-md border p-5 text-center text-sm text-muted-foreground"
          >
            <Button
              variant="link"
              class="h-auto p-0"
              href={`${resolve("/login")}?returnTo=${returnTo}`}>Sign in</Button
            >
            to comment.
          </div>
        {/if}
      </div>

      <aside class="space-y-5 text-sm">
        <section class="border-b pb-5">
          <h2
            class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground"
          >
            <UserRound class="size-3.5" />Assignee
          </h2>
          {#if issue.assignee}
            <div class="mt-3 flex items-center gap-2">
              {@render userAvatar(issue.assignee, null, "size-6")}
              <span class="font-medium">{issue.assignee.username}</span>
            </div>
          {:else}
            <p class="mt-3 text-xs text-muted-foreground">No one assigned</p>
          {/if}
        </section>
        <section class="border-b pb-5">
          <h2
            class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground"
          >
            <Tag class="size-3.5" />Labels
          </h2>
          <div class="mt-3 flex flex-wrap gap-1.5">
            {#each issue.labels as label (label.id)}
              <LabelChip {label} />
            {:else}
              <span class="text-xs text-muted-foreground">None yet</span>
            {/each}
          </div>
        </section>
        {#if issue.can_manage}
          <Button
            variant="ghost"
            size="sm"
            class="w-full justify-start gap-2 text-destructive"
            onclick={() => {
              pendingDeleteIssue = issue.number;
              deleteIssueDialogOpen = true;
            }}
          >
            <Trash2 class="size-3.5" />Delete issue
          </Button>
        {/if}
      </aside>
    </div>
  {:else}
    <header
      class="flex flex-wrap items-center justify-between gap-4 border-b pb-4"
    >
      <div>
        <h1 class="text-xl font-semibold tracking-tight">Issues</h1>
        <p class="mt-1 text-sm text-muted-foreground">
          Track bugs, tasks, ideas, and decisions with your collaborators.
        </p>
      </div>
      <div class="flex gap-2">
        {#if repository.repository?.can_manage}
          <Button
            variant="outline"
            class="gap-2"
            onclick={() => (labelsOpen = true)}
          >
            <Tag class="size-4" />Labels
          </Button>
        {/if}
        <Button class="gap-2" onclick={openComposer}
          ><Plus class="size-4" />New issue</Button
        >
      </div>
    </header>

    <div class="mt-5 overflow-hidden rounded-md border">
      <div class="flex flex-wrap items-center gap-3 border-b bg-muted/15 p-3">
        <div class="flex items-center gap-1">
          <Button
            variant={filterState === "open" ? "secondary" : "ghost"}
            size="sm"
            class="gap-2"
            aria-pressed={filterState === "open"}
            onclick={() => (filterState = "open")}
          >
            <CircleDot class="size-3.5" />Open
          </Button>
          <Button
            variant={filterState === "closed" ? "secondary" : "ghost"}
            size="sm"
            class="gap-2"
            aria-pressed={filterState === "closed"}
            onclick={() => (filterState = "closed")}
          >
            <CheckCircle2 class="size-3.5" />Closed
          </Button>
        </div>
        <div class="relative min-w-48 flex-1">
          <Search
            class="pointer-events-none absolute left-3 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
          />
          <Input
            bind:value={search}
            class="pl-9"
            placeholder="Search issues"
            aria-label="Search issues"
          />
        </div>
      </div>
      {#if repository.issueLabels.length}
        <div class="flex flex-wrap items-center gap-2 border-b px-4 py-2">
          <button
            type="button"
            class={labelFilter === null
              ? "text-xs font-medium"
              : "text-xs text-muted-foreground hover:text-foreground"}
            aria-pressed={labelFilter === null}
            onclick={() => (labelFilter = null)}>All labels</button
          >
          {#each repository.issueLabels as label (label.id)}
            <button
              type="button"
              class="opacity-80 hover:opacity-100"
              aria-pressed={labelFilter === label.id}
              onclick={() =>
                (labelFilter = labelFilter === label.id ? null : label.id)}
            >
              <LabelChip {label} />
            </button>
          {/each}
        </div>
      {/if}
      <ul class="divide-y">
        {#if repository.issuesLoading && !repository.issuesLoaded}
          <li class="py-16 text-center text-sm text-muted-foreground">
            Loading issues…
          </li>
        {:else if repository.error && !repository.issuesLoaded}
          <li class="py-16 text-center text-sm text-muted-foreground">
            <p>Issues could not be loaded.</p>
            <Button
              variant="link"
              class="mt-2"
              onclick={() => void repository.loadView()}>Try again</Button
            >
          </li>
        {:else}
          {#each filteredIssues as issue (issue.id)}
            <li class="flex gap-3 px-4 py-4 hover:bg-muted/10">
              {#if issue.state === "open"}
                <CircleDot class="mt-0.5 size-4 shrink-0 text-emerald-500" />
              {:else}
                <CheckCircle2 class="mt-0.5 size-4 shrink-0 text-violet-500" />
              {/if}
              <div class="min-w-0 flex-1">
                <div class="flex flex-wrap items-center gap-2">
                  <button
                    class="text-left text-sm font-semibold hover:underline"
                    onclick={() => repository.selectIssue(issue.number)}
                  >
                    {issue.title}
                  </button>
                  {#if issue.external_url}
                    <a
                      href={issue.external_url}
                      target="_blank"
                      rel="noreferrer"
                    >
                      <Badge variant="outline">Imported from GitHub</Badge>
                    </a>
                  {/if}
                  {#each issue.labels as label (label.id)}
                    <LabelChip {label} />
                  {/each}
                </div>
                <p class="mt-1 text-xs text-muted-foreground">
                  #{issue.number} opened on {issueDate(issue.created_at)} by {issue
                    .external_author?.username ?? issue.author.username}
                  {#if issue.assignee}
                    · assigned to {issue.assignee.username}{/if}
                </p>
              </div>
              {#if issue.comment_count}
                <span
                  class="flex shrink-0 items-center gap-1 text-xs text-muted-foreground"
                >
                  <MessageSquare class="size-3.5" />{issue.comment_count}
                </span>
              {/if}
            </li>
          {:else}
            <li class="py-16 text-center">
              <CircleDot class="mx-auto size-8 text-muted-foreground" />
              <p class="mt-3 text-sm font-medium">
                No {filterState} issues match
              </p>
              <p class="mt-1 text-xs text-muted-foreground">
                Try another search, label, or state.
              </p>
            </li>
          {/each}
        {/if}
      </ul>
    </div>
  {/if}
</div>

<IssueComposerDialog
  bind:open={composerOpen}
  state={repository}
  issue={composerIssue}
/>

{#if repository.repository?.can_manage}
  <IssueLabelDialog bind:open={labelsOpen} state={repository} mode="manage" />
{/if}

<AlertDialog.Root bind:open={deleteCommentDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Delete this comment?</AlertDialog.Title>
      <AlertDialog.Description>This cannot be undone.</AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        onclick={() => {
          const target = pendingDeleteComment;
          if (!target) return;
          deleteCommentDialogOpen = false;
          pendingDeleteComment = null;
          void repository.deleteIssueComment(target.issueNumber, target.id);
        }}>Delete comment</AlertDialog.Action
      >
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<AlertDialog.Root bind:open={deleteIssueDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title
        >Permanently delete issue #{pendingDeleteIssue ??
          ""}?</AlertDialog.Title
      >
      <AlertDialog.Description>This cannot be undone.</AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        onclick={() => {
          const id = pendingDeleteIssue;
          if (id == null) return;
          deleteIssueDialogOpen = false;
          pendingDeleteIssue = null;
          void repository.deleteIssue(id);
        }}>Delete issue</AlertDialog.Action
      >
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
