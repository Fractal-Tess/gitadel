<script lang="ts">
  let { data } = $props();
</script>

<svelte:head>
  <title>Gitadel Dokploy fixture</title>
  <meta
    name="description"
    content="Live deployment fixture for Gitadel's Dokploy integration"
  />
</svelte:head>

<main>
  <p class="eyebrow">Gitadel integration fixture</p>
  <h1>Dokploy deployment is live</h1>
  <dl>
    <div>
      <dt>Deployment kind</dt>
      <dd data-testid="deployment-kind">{data.deploymentKind}</dd>
    </div>
    <div>
      <dt>Fixture value</dt>
      <dd data-testid="fixture-value">{data.fixtureValue}</dd>
    </div>
    <div>
      <dt>Revision</dt>
      <dd data-testid="fixture-revision">{data.fixtureRevision}</dd>
    </div>
  </dl>
</main>

<style>
  :global(*) {
    box-sizing: border-box;
  }

  :global(body) {
    margin: 0;
    min-width: 320px;
    min-height: 100vh;
    color: #edf7f2;
    background:
      radial-gradient(circle at 50% 0%, #174b3a 0, transparent 42rem),
      #07110e;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }

  main {
    width: min(44rem, calc(100% - 2rem));
    margin: 0 auto;
    padding: 18vh 0 4rem;
  }

  .eyebrow {
    margin: 0 0 1rem;
    color: #72d7ad;
    font-size: 0.75rem;
    letter-spacing: 0.16em;
    text-transform: uppercase;
  }

  h1 {
    max-width: 12ch;
    margin: 0 0 3rem;
    font: 600 clamp(2.5rem, 8vw, 5.5rem) / 0.95 ui-sans-serif, sans-serif;
    letter-spacing: -0.06em;
  }

  dl {
    display: grid;
    gap: 1px;
    overflow: hidden;
    border: 1px solid #285b49;
    border-radius: 0.75rem;
    background: #285b49;
  }

  dl div {
    display: grid;
    grid-template-columns: minmax(9rem, 1fr) 2fr;
    gap: 1rem;
    padding: 1rem 1.25rem;
    background: #0d1d18;
  }

  dt {
    color: #85a89a;
  }

  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
</style>
