<svelte:head>
  <title>Gitadel Actions fixture</title>
  <meta
    name="description"
    content="A minimal SvelteKit repository for exercising Gitadel Actions"
  />
</svelte:head>

<main>
  <p class="eyebrow">Gitadel Actions</p>
  <h1>The workflow passed.</h1>
  <p class="summary">
    This SvelteKit project checks type safety and creates a production build on every push to
    <code>main</code>.
  </p>

  <section aria-label="Workflow stages">
    <div>
      <span>01</span>
      <strong>Clone</strong>
      <p>Checks out the exact pushed commit with a short-lived repository token.</p>
    </div>
    <div>
      <span>02</span>
      <strong>Validate</strong>
      <p>Runs Svelte diagnostics and produces the server build.</p>
    </div>
    <div>
      <span>03</span>
      <strong>Report</strong>
      <p>Passes the commit SHA to a dependent job through a workflow output.</p>
    </div>
  </section>
</main>

<style>
  :global(*) {
    box-sizing: border-box;
  }

  :global(body) {
    margin: 0;
    min-width: 320px;
    min-height: 100vh;
    color: #e8edf7;
    background:
      linear-gradient(rgba(112, 151, 255, 0.08) 1px, transparent 1px),
      linear-gradient(90deg, rgba(112, 151, 255, 0.08) 1px, transparent 1px),
      radial-gradient(circle at 50% -20%, #183872 0, transparent 36rem),
      #070a11;
    background-size: 2rem 2rem, 2rem 2rem, auto, auto;
    font-family: Inter, ui-sans-serif, system-ui, sans-serif;
  }

  main {
    width: min(64rem, calc(100% - 2rem));
    margin: 0 auto;
    padding: clamp(5rem, 14vh, 10rem) 0 4rem;
  }

  .eyebrow {
    margin: 0 0 1.25rem;
    color: #83a8ff;
    font: 600 0.75rem/1 ui-monospace, monospace;
    letter-spacing: 0.16em;
    text-transform: uppercase;
  }

  h1 {
    max-width: 10ch;
    margin: 0;
    font-size: clamp(3.5rem, 10vw, 7.5rem);
    line-height: 0.9;
    letter-spacing: -0.07em;
  }

  .summary {
    max-width: 42rem;
    margin: 2rem 0 4rem;
    color: #a6b0c3;
    font-size: clamp(1rem, 2vw, 1.25rem);
    line-height: 1.6;
  }

  code {
    color: #d9e4ff;
  }

  section {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    border: 1px solid #26334c;
    background: #0b101b;
  }

  section div {
    min-height: 12rem;
    padding: 1.5rem;
    border-right: 1px solid #26334c;
  }

  section div:last-child {
    border-right: 0;
  }

  section span,
  section strong {
    display: block;
  }

  section span {
    margin-bottom: 2.5rem;
    color: #6681b8;
    font: 0.75rem/1 ui-monospace, monospace;
  }

  section strong {
    margin-bottom: 0.75rem;
    font-size: 1.125rem;
  }

  section p {
    margin: 0;
    color: #8793aa;
    line-height: 1.5;
  }

  @media (max-width: 700px) {
    section {
      grid-template-columns: 1fr;
    }

    section div {
      min-height: auto;
      border-right: 0;
      border-bottom: 1px solid #26334c;
    }

    section div:last-child {
      border-bottom: 0;
    }
  }
</style>
