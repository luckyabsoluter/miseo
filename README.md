# miseo

Stable global CLI tools on top of [mise](https://mise.jdx.dev/).

`miseo` (pronounced "miso") manages each global CLI tool as its own isolated environment, pinned to a specific runtime version, so commands stay reliable as you move between projects and upgrade default runtime versions on your system.

## Install

```bash
mise use -g cargo:miseo
```

Add `~/.miseo/.bin` to your `$PATH`.

## Usage

```bash
miseo install npm:http-server
miseo install -g npm:http-server
miseo install npm:http-server --use node@22
miseo upgrade npm:http-server
miseo upgrade npm:http-server --use node@22
miseo uninstall npm:http-server
```

## Why `miseo` exists

`mise` is excellent for managing runtimes and toolchains _within the context of a project_. But sometimes you are just trying to install a global tool.

Consider [`http-server`](https://www.npmjs.com/package/http-server): a simple CLI that serves the current directory over HTTP. It is useful when prototyping static sites, demos, or quick local previews. You want it available globally so you can run it from anywhere.

`http-server` happens to be implemented in Node and distributed via npm (`npm install -g http-server`), but those are implementation details. Your mental model is that you are installing a global CLI tool, you just want it to work and keep working as you switch between projects and upgrade things on your system.

In that mode, you probably don't care which version of Node it uses. The system default at installation time is usually fine. Once installed, you want it to keep working the same way until you have a reason to upgrade. And when you do upgrade, you might choose to repin its runtime to your current default.

This is where you run into friction with mise. When you install a tool globally (`mise use -g npm:http-server`), the package is installed once, but execution still follows mise's active runtime context. In practice, that means the same global tool can run with different runtimes in different directories. Things may break, native extensions may stop working, you may see unexpected deprecation warnings – not ideal for "install once and keep stable until I explicitly change it" global utilities.

`miseo` exists to close that gap while staying in the mise ecosystem.

Coming from [Volta](https://volta.sh) ([R.I.P.](https://github.com/volta-cli/volta/issues/2080)), this is a problem it solved nicely, and it is the feature I missed the most when moving to mise. The goal of `miseo` is to be a lightweight layer on top of mise for a "Volta lite" global tools experience.

## Core model

The core idea is simple: every global utility _is_ its own mini mise project.

Instead of `npm install -g http-server` or `mise use -g npm:http-server`, you run:

```bash
miseo install npm:http-server
```

Under the hood, `miseo` creates a dedicated tool root at `~/.miseo/npm-http-server` and tracks it in a `miseo`-owned manifest (`~/.miseo/.miseo-installs.toml`).

Each installed target gets a versioned runtime-scoped directory, typically:

- `~/.miseo/npm-http-server/14.1.1+node-22.13.1`
- `~/.miseo/npm-http-server/global+node-22.13.1` for `miseo install -g npm:http-server`

By default, package content is installed directly into that variant directory with `mise install-into`. When `-g`/`--global` is provided, npm packages are instead installed with `npm install -g` inside the activated variant environment.

Then `miseo` links the shim from `~/.miseo/.bin` to the currently active install:

- `~/.miseo/.bin/http-server -> ~/.miseo/npm-http-server/current/.miseo/http-server`

All you have to do is to put `~/.miseo/.bin` in your `PATH` and this all works. Because each tool has its own pinned runtime, upgrades in unrelated projects and global default cannot accidentally break your global utility.

## Local precedence

Some tools are dual-purpose: they can act as a global utility, but specific projects may also want to install a local version. For example, you might have a global `http-server`, while one project pins a different version for its demo.

Volta handles this with smart, project-aware shims. The `http-server` shim in Volta would first check whether you are currently in a Node project with a `http-server` dependency and, if so, transparently execute the local version instead. That is a great experience, but it depends on deep understanding of ecosystem-specific logic and conventions (package manifests, local install conventions, etc.). Volta is a JavaScript-specific manager; `mise`/`miseo` are trying to stay more general across ecosystems, so `miseo` does not currently implement that style of interception.

By default, `miseo` behaves like ordinary global installs: no implicit local/global switching logic. In our example, the global `http-server` shim always executes the globally installed version, just like `npm install -g http-server` would. Usually, in the Node ecosystem, you solve this by using launchers like `npx http-server`, `pnpm http-server`, etc. If that works for you, you can keep doing that; `miseo` does not get in the way.

The cleaner cross-ecosystem approach is to let `PATH` ordering handle precedence by configuring `mise` to add local bin directories when you enter a project. This requires configuring `mise` PATH behavior for local package bins where relevant. For example, in a Node project:

```toml
[env]
_.path = ["{{config_root}}/node_modules/.bin"]
```

When that is present and `mise activate` is used, mise adds the local `node_modules/.bin` into an earlier `PATH` position, naturally overriding the global shim in `~/.miseo/.bin`.

If I find myself really missing the intelligent shims from Volta, maybe I'll revisit this design.

## Install strategy

The default path uses `mise install-into`, keeping package content under the versioned variant directory:

```bash
mise exec --cd / node@22.13.1 -- \
  mise install-into npm:http-server@14.1.1 ~/.miseo/npm-http-server/14.1.1+node-22.13.1
```

For npm packages that need the runtime's normal global install layout, use `-g`:

```bash
miseo install -g npm:http-server

mise exec --cd ~/.miseo/npm-http-server/14.1.1+node-22.13.1 -- \
  npm install -g http-server@14.1.1
```

In global mode, npm command discovery reads the installed package's `bin` metadata. When a bin entry points to a Node JS entrypoint, `miseo` resolves the pinned Node with `mise which node -C <variant>` and runs that entrypoint directly instead of exporting the whole tool-local environment. If the bin entry is not a Node script, the wrapper falls back to the executable created in that runtime's global npm bin directory.
The npm package is still installed with the resolved exact version. The variant directory uses `global+<runtime>` because that runtime's global npm install can later be changed outside of `miseo`; current checks look up the installed global package version instead of trusting the manifest record.

## Limitations and trade-offs

The explicit trade-off we made with `miseo` approach is to prioritize stability for global tools and make upgrading, including its runtime environment, an explicit choice.

From an implementation standpoint, this means giving each CLI tool explicit runtime pins and then generating a command launch plan. A launch plan can either activate the tool-local environment before running an executable, or resolve one pinned runtime and run a known entrypoint directly. Runtime-specific decisions live in launch extensions; npm global installs use the Node launch extension for package bins so the install runtime does not leak into project commands spawned by the tool.

More importantly, from a functional standpoint, this strategy means ordinary global utilities keep their stable install runtime, while agent-style tools that spawn project commands do not force that runtime onto child processes when backend discovery can identify a direct runtime entrypoint.

However, for devtools like [tsx](https://www.npmjs.com/package/tsx), this might not be what you want. For those kinds of tools, the floating approach of `mise use -g` may work better. Arguably, the more correct fix is to add `tsx` to your project's dev dependencies and let the project-local version take precedence.
