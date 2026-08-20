<script lang="ts">
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { onMount } from 'svelte';
	import { z } from 'zod';
	import { Compass, GitBranch, GitFork, LockKeyhole, Search, Settings2, Sparkles, Star } from 'lucide-svelte';

	import { Button } from '$lib/components/ui/button/index.js';

	import {
		ApiFailure,
		repositorySchema,
		requestJson,
		type Repository
	} from '$lib/api.js';
	import { useAppState } from '$lib/state/app-state.svelte.js';

	const app = useAppState();
	let repositories = $state.raw<Repository[]>([]);
	let search = $state(page.url.searchParams.get('q') ?? '');
	let loading = $state(true);
	let error = $state<string | null>(null);
	let filter = $state<'all' | 'favorites' | 'recent'>('all');

	let visibleRepositories = $derived.by(() => {
		const query = search.trim().toLowerCase();
		let filtered = filter === 'favorites'
			? repositories.filter((repository) => repository.favorited)
			: repositories;
		if (query) {
			filtered = filtered.filter((repository) =>
				`${repository.namespace}/${repository.name} ${repository.description ?? ''}`
					.toLowerCase()
					.includes(query)
			);
		}
		return filter === 'recent'
			? [...filtered].sort((left, right) => right.updated_at.localeCompare(left.updated_at))
			: filtered;
	});

	function message(caught: unknown): string {
		if (caught instanceof ApiFailure || caught instanceof Error) return caught.message;
		return 'Could not load repositories.';
	}

	function updatedAt(value: string): string {
		return new Intl.DateTimeFormat(undefined, {
			month: 'short',
			day: 'numeric',
			year: 'numeric'
		}).format(new Date(value));
	}

	onMount(async () => {
		try {
			repositories = await requestJson('/api/v1/repositories', z.array(repositorySchema));
		} catch (caught) {
			error = message(caught);
		} finally {
			loading = false;
		}
	});
</script>

<svelte:head>
	<title>{app.instance?.site_name ?? 'Gitadel'} · Project archive</title>
	<meta name="description" content={app.instance?.site_description ?? 'A small Git server for projects worth keeping.'} />
</svelte:head>

<div class="min-h-screen bg-background">
	<header class="sticky top-0 z-30 border-b bg-background/95 backdrop-blur">
		<div class="flex h-[64px] items-center gap-4 px-4 sm:px-5">
			<a class="flex shrink-0 items-center gap-3" href={resolve('/')} aria-label={`${app.instance?.site_name ?? 'Gitadel'} home`}>
				<strong class="text-sm font-bold tracking-[-0.035em]">{app.instance?.site_name ?? 'GITADEL'}</strong>
				<span class="text-muted-foreground">/</span>
				<span class="text-sm font-medium">Explore</span>
			</a>

			<label class="relative hidden w-full max-w-xl lg:block">
				<span class="sr-only">Search repositories</span>
				<Search class="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
				<input
					class="h-10 w-full rounded-md border bg-input/45 pl-10 pr-3 text-sm outline-none placeholder:text-muted-foreground/70 focus:border-ring focus:ring-2 focus:ring-ring/15"
					bind:value={search}
					placeholder="Search repositories..."
				/>
			</label>

			<nav class="ml-auto flex items-center gap-1" aria-label="Primary navigation">
				<a class="hidden items-center gap-2 rounded-md px-3 py-2 text-sm font-medium hover:bg-accent sm:flex" href={resolve('/')}>
					<Compass class="size-4" />
					Explore
				</a>
				<Button href={resolve('/settings')} variant="ghost" class="gap-2 text-muted-foreground hover:text-foreground">
					<Settings2 class="size-4" />
					{app.authStatus?.authenticated ? app.authStatus.user?.username : 'Sign in'}
				</Button>
			</nav>
		</div>
	</header>

	<div class="grid min-h-[calc(100vh-64px)] md:grid-cols-[15rem_minmax(0,1fr)]">
		<aside class="hidden border-r px-5 py-6 md:block">
			<p class="mb-3 text-xs font-medium text-muted-foreground">Browse</p>
			<nav class="space-y-1" aria-label="Repository filters">
				<button
					class={filter === 'all' ? 'flex h-9 w-full items-center gap-3 rounded-md bg-accent px-3 text-sm font-medium' : 'flex h-9 w-full items-center gap-3 rounded-md px-3 text-sm text-muted-foreground hover:bg-accent hover:text-foreground'}
					onclick={() => (filter = 'all')}
				>
					<Compass class="size-4" />
					All repositories
				</button>
				<button
					class={filter === 'favorites' ? 'flex h-9 w-full items-center gap-3 rounded-md bg-accent px-3 text-sm font-medium' : 'flex h-9 w-full items-center gap-3 rounded-md px-3 text-sm text-muted-foreground hover:bg-accent hover:text-foreground'}
					onclick={() => (filter = 'favorites')}
				>
					<Star class="size-4" />
					Favorites
				</button>
				<button
					class={filter === 'recent' ? 'flex h-9 w-full items-center gap-3 rounded-md bg-accent px-3 text-sm font-medium' : 'flex h-9 w-full items-center gap-3 rounded-md px-3 text-sm text-muted-foreground hover:bg-accent hover:text-foreground'}
					onclick={() => (filter = 'recent')}
				>
					<Sparkles class="size-4" />
					Recently updated
				</button>
			</nav>
		</aside>

		<main class="min-w-0 px-5 py-7 lg:px-8 lg:py-8">
			<div class="mx-auto max-w-5xl">
				<label class="relative mb-5 block lg:hidden">
					<span class="sr-only">Search repositories</span>
					<Search class="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
					<input
						class="h-10 w-full rounded-md border bg-input/45 pl-10 pr-3 text-sm outline-none placeholder:text-muted-foreground/70 focus:border-ring focus:ring-2 focus:ring-ring/15"
						bind:value={search}
						placeholder="Search repositories..."
					/>
				</label>

				<section id="repositories" aria-labelledby="repositories-heading">
					<div class="mb-5 flex flex-wrap items-end justify-between gap-4">
						<div>
							<h1 id="repositories-heading" class="flex items-center gap-3 text-lg font-semibold tracking-tight">
								<Star class="size-5 text-amber-500" />
								Repositories
							</h1>
							<p class="mt-1.5 text-sm text-muted-foreground">
								{app.authStatus?.authenticated ? 'Public repositories and projects shared with you.' : 'Public projects available on this server.'}
							</p>
						</div>
						<p class="text-xs tabular-nums text-muted-foreground">{visibleRepositories.length} total</p>
					</div>

					{#if loading}
						<div class="overflow-hidden rounded-md border bg-card" aria-label="Loading repositories">
							{#each Array(6) as _, index (index)}
								<div class="h-[4.75rem] animate-pulse border-b bg-muted/20 last:border-b-0"></div>
							{/each}
						</div>
					{:else if error}
						<div class="rounded-md border border-destructive/30 bg-destructive/5 p-5 text-sm text-destructive">{error}</div>
					{:else}
						<div class="overflow-hidden rounded-md border bg-card/35">
							<ul class="divide-y">
								{#each visibleRepositories as repository, index (repository.id)}
									<li>
										<a
											class="group grid min-h-[4.75rem] grid-cols-[2.5rem_minmax(0,1fr)] items-center gap-3 px-4 hover:bg-accent/55 sm:grid-cols-[2.5rem_minmax(0,1fr)_auto]"
											href={resolve('/[namespace]/[name]', { namespace: repository.namespace, name: repository.name })}
										>
											<span class="font-mono text-sm tabular-nums text-muted-foreground">{index + 1}</span>
											<div class="min-w-0 py-3">
												<div class="flex items-center gap-2">
													<h2 class="truncate text-sm font-semibold">
														<span class="text-muted-foreground">{repository.namespace}/</span>{repository.name}
													</h2>
													{#if repository.visibility === 'private'}
														<span class="inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground"><LockKeyhole class="size-2.5" /> Private</span>
													{/if}
													{#if repository.favorited}
														<Star class="size-3 fill-amber-400 text-amber-400" aria-label="Favorited" />
													{/if}
													{#if repository.archived_at}
														<span class="rounded border px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">Archived</span>
													{/if}
												</div>
												<p class="mt-1 truncate text-sm text-muted-foreground">{repository.description ?? 'No description provided.'}</p>
											</div>
											<div class="col-start-2 flex items-center gap-4 pb-3 text-xs text-muted-foreground sm:col-start-auto sm:pb-0">
												<span class="inline-flex items-center gap-1.5"><GitBranch class="size-3.5" />{repository.default_branch}</span>
												<span class="hidden items-center gap-1.5 md:inline-flex"><GitFork class="size-3.5" />{repository.object_format.toUpperCase()}</span>
												<span class="hidden lg:inline">Updated {updatedAt(repository.updated_at)}</span>
											</div>
										</a>
									</li>
								{:else}
									<li class="grid place-items-center px-6 py-16 text-center">
										<GitBranch class="size-7 text-muted-foreground/60" strokeWidth={1.4} />
										<p class="mt-3 text-sm font-medium">{search ? 'No matching repositories' : 'No repositories yet'}</p>
										<p class="mt-1 text-xs text-muted-foreground">{search ? 'Try a different search.' : 'Create one with the Gitadel CLI, then push over SSH.'}</p>
									</li>
								{/each}
							</ul>
						</div>
					{/if}
				</section>
			</div>
		</main>
	</div>
</div>
