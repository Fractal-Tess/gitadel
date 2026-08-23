# Actions runner and control-plane footprint

## Question

How much code is involved in adding GitHub Actions-style automation to Gitadel, especially if job execution is delegated to an existing runner and the preferred native runner is written in Rust?

## Method

The repositories below were shallow-cloned into `/tmp/gitadel-runner-analysis` and inspected at the exact commits listed below. Counts use `tokei 14.0.0` **code lines**, not physical lines. They are based on Git-tracked files. Documentation, screenshots, fixture repositories, vendored `node_modules`, and generated files are excluded unless stated otherwise. Production, tests, runner protocol, server control plane, and UI are separated where the repository layout permits it.

The classification script and exact file manifests used for the detailed Forgejo count are in `/tmp/gitadel-runner-analysis/count-footprint.py` and `/tmp/gitadel-runner-analysis/footprint-manifests/` for the duration of this working session.

## Repositories inspected

| Project | Commit | Role | License |
| --- | --- | --- | --- |
| [Forgejo Runner](https://code.forgejo.org/forgejo/runner) | `cc052b63e34b427d62684cef31f152cf373af701` | Forgejo Actions runner | MIT |
| [Forgejo](https://codeberg.org/forgejo/forgejo) | `0a2e35b8ba2a633341f0787dc64f73bb99412a19` | Forge and Actions control plane | MIT |
| [Forgejo Actions protocol](https://code.forgejo.org/forgejo/actions-proto) | `cd808c6046186d5852cc24441f71e5e08e5f1644` | ConnectRPC/protobuf contract | MIT |
| [Gitea Runner](https://gitea.com/gitea/runner) | `b97c61aa1496065855e1ad301d9610ee43019345` | Gitea Actions runner | MIT |
| [Gitea](https://github.com/go-gitea/gitea) | `e6af4c341ca84242ab11fec0b46ceb3e2c8f5eca` | Forge and Actions control plane | MIT |
| [Gitea actionslib](https://gitea.com/gitea/actionslib) | `b43d318f1760d5a23bf2b5cfdbb87b3cd99884e2` (`v0.7.0`) | Shared workflow models, expressions, and generated protocol | MIT |
| [minact](https://github.com/fastforgedev/minact) | `ada2f299c266ce0dc82f954b5ee14b1eea9adb01` | Rust local GitHub Actions-compatible engine | MIT |
| [enact-rs](https://github.com/hyperfinitism/enact-rs) | `224446cf17119ae5872234cea26ccfe00bd94368` | Rust workflow emulator | Apache-2.0 |
| [Pernosco gha-runner](https://github.com/Pernosco/gha-runner) | `ac8d18f98230440a32775c1e61b6044fe859a176` | Rust pluggable workflow executor | MIT |
| [GitBundle](https://github.com/gitbundle/gitbundle) | `09a888dc9e4fdb243e4740666c4abe937e9c1d5b` | Advertised Rust forge and workflow system | Elastic License 2.0 |

## Forgejo footprint

### Runner

| Area | Production code | Tests | Notes |
| --- | ---: | ---: | --- |
| Integrated `act` execution engine | 16,787 | included below | Workflow model, expressions, containers, steps, actions, cache support |
| Runner orchestration outside `act` | 3,852 | included below | Polling, execution, reporting, configuration, CLI |
| Narrow generated-client wrapper | 133 | — | HTTP/ConnectRPC client setup only |
| **First-party runner production total** | **20,772** | **22,418** | Generated mocks and generated protobuf/plugin files excluded |

There are about 2,070 production lines at call sites directly coupled to ConnectRPC or the Actions protocol, but these overlap the totals above. The protocol transport is not the large part of the runner; execution semantics are.

### Server

| Area | Production code | Tests | Notes |
| --- | ---: | ---: | --- |
| Runner-facing APIs | 1,976 | included below | ConnectRPC plus job runtime endpoints for artifacts, tokens, and related services |
| Of which: six core runner RPCs | 471 | — | Register, Declare, FetchTask, FetchSingleTask, UpdateTask, UpdateLog |
| Actions control plane | 13,689 | 9,531 unit | Models, scheduling, workflow expansion, jobs, secrets, variables, cleanup, statuses, logs |
| Actions UI | 2,175 | 559 | Templates, Vue/TypeScript, CSS |
| Counted Actions-related server tests | — | 20,807 | Unit, integration, end-to-end, and UI tests; fixtures excluded |
| **Server production subtotal** | **17,840** | **20,807** | Excludes 915 generated artifact-protocol lines |

### Protocol

The actual Forgejo runner contract is small:

- 6 RPC methods;
- 139 lines of protobuf definitions across the runner and ping protocols;
- 2,235 generated Go lines, which a Rust implementation would replace with generated Rust types.

A task contains an expanded workflow/job YAML payload, GitHub context, secrets, dependency outputs, variables, and task identity. The server therefore performs workflow discovery, expansion, dependency scheduling, and job assignment before the runner executes anything.

### Whole Forgejo Actions subsystem

The identified Actions-related production footprint is approximately:

- **20.8k** runner lines;
- **17.8k** server and UI lines;
- **38.6k production lines total**, excluding generated protocol code and general Forgejo infrastructure;
- **43.2k identified Actions-related test lines**.

This is a mature-feature upper reference, not the minimum Gitadel must copy.

## Gitea footprint

Gitea has moved shared workflow models, expression evaluation, and generated runner protocol code into `gitea.dev/actionslib`.

| Area | Production code | Tests | Notes |
| --- | ---: | ---: | --- |
| Gitea runner repository | 16,693 | 17,161 | Generated files not separately removed in this headline count |
| Shared `actionslib` | 4,506 | 2,397 | Shared by runner/server; includes generated protocol code |
| Direct server Actions Go directories | 17,406 | 8,117 | Models, modules, APIs, services, web handlers |
| Actions UI | 2,524 | — | Templates, Vue, CSS |
| **Ecosystem subtotal** | **about 41.1k** | **at least 27.7k** | Some cross-cutting integration tests and generic infrastructure are not included |

The current Gitea and Forgejo implementations land in the same broad range: about **39–42k production lines** for runner, platform, and UI together.

## Rust candidates

### minact

`minact` is the strongest source candidate found for Gitadel's preferred direction.

| Area | Code lines |
| --- | ---: |
| Core engine source | 10,264 |
| Core integration tests | 3,128 |
| Studio/server Rust source | 2,409 |
| Studio Rust tests | 509 |
| CLI | 312 |
| Studio web UI | 4,400 |

The core implements workflow parsing, expressions, matrices, job dependencies, shell steps, JavaScript/composite/container actions, remote action checkout, Docker/local/SSH executors, workflow command files, caches, and artifacts. It honors `GITHUB_SERVER_URL` when fetching actions, which is useful for a forge other than GitHub.

At the inspected commit, `cargo test --all-targets` passed **263 tests with one ignored**. The project is young and small, so passing unit/integration tests do not establish production compatibility with the GitHub Actions corpus.

Most importantly, `minact` is an **execution engine**, not a daemon that registers with a forge. It has no durable platform queue or Forgejo/Gitea runner protocol client.

### enact-rs

| Area | Code lines |
| --- | ---: |
| Core plus inline tests | 4,672 |
| CLI | 144 |

It supports workflow parsing, expressions, matrices, dependencies, built-in checkout/cache/artifact behavior, composite and Node actions, and local execution. `cargo test --all-targets` passed 87 tests. It is smaller but has a narrower execution and isolation story than `minact` and is likewise not a forge-connected runner daemon.

### Pernosco `gha-runner`

| Area | Code lines |
| --- | ---: |
| Library source | 2,326 |
| Tests and example | 130 |

Its own README says only very simple workflows are supported and lists missing container actions, expression syntax, pre/post actions, and Docker-in-workflow support. The inspected revision also failed to build because dependency drift between `octocrab` and `hyper-rustls` removed a method it expects. It is useful architectural prior art for a pluggable backend, but not a suitable base today.

### GitBundle

The cloned repository contains documentation, Compose configuration, screenshots, and release metadata, but **no Rust server or runner source** at the inspected commit. Its Elastic License 2.0 is also materially more restrictive than Gitadel's MIT license. It cannot currently be audited or reused as a source dependency.

## What the numbers mean for Gitadel

Reusing a runner only removes the execution-engine portion. It does not remove the control plane.

A Forgejo-compatible Gitadel implementation can keep the wire surface small—six RPCs and roughly 140 lines of protocol definitions—but still needs persistent state and behavior for:

- runner registration, credentials, labels, heartbeat, and task claiming;
- workflow discovery and event filtering;
- run/job/step persistence and dependency scheduling;
- context and short-lived repository token construction;
- secrets and variables;
- logs, cancellation, retry, and crash recovery;
- commit statuses and the run UI;
- optionally caches, artifacts, OIDC, concurrency groups, and retention.

### Plausible footprint targets

These are code-footprint targets, not calendar estimates:

| Gitadel scope | New first-party product code | External/reused engine |
| --- | ---: | ---: |
| Minimal Forgejo-runner-compatible control plane, push/manual events, logs/status, no cache/artifacts/OIDC | **8–12k** | Forgejo/Gitea runner binary |
| Useful Gitadel Actions control plane and UI | **14–20k** | Forgejo/Gitea runner binary |
| Native Rust runner around `minact` plus focused control plane/UI | **12–19k new Gitadel integration code** | About **10–13k minact core source/test code**; **22–32k integrated feature footprint** |
| Mature Forgejo/Gitea-like subsystem | **about 39–42k production code total** | None assumed external |

The native Rust-runner estimate breaks down roughly as:

- 1.5–3k Rust for runner daemon, registration, polling, reporting, cancellation, and configuration;
- 10–13k reused/forked Rust execution-engine footprint;
- 10–16k Rust/Svelte for the durable Gitadel control plane and UI.

## Recommended architecture

Use the small Forgejo/Gitea-style protobuf task contract as the boundary, while keeping GitHub-compatible workflow files and action semantics:

```text
.github/workflows/*.yml
        |
        v
Gitadel event parser + durable scheduler
        |
        | ConnectRPC-compatible task protocol
        v
Forgejo Runner today  OR  Gitadel Rust Runner + minact-derived engine
```

This gives Gitadel an immediate path to an established runner while preserving a clean route to a native Rust runner. The protocol should be treated as a compatibility target rather than copied blindly: pin a tested version and add contract tests against real Forgejo and Gitea runner binaries.

The first implementation should defer artifact/cache services, service containers, OIDC, reusable workflows, approvals, and broad GitHub event parity. They account for much of the difference between an 8–12k control plane and the roughly 18k mature Forgejo server/UI footprint.
