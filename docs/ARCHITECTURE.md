# miseo Architecture

This document describes how `miseo` works: its behavior, constraints, and design tradeoffs.

## Purpose

`miseo` provides stable global CLI tools on top of `mise` by isolating each installed CLI into its own mini mise project that pins its own runtime environment.

Core idea:

- one global CLI tool = one isolated tool project
- each tool project has its own pinned runtime
- public commands are symlinks into that tool project

## Design Goals

- Keep global tools stable until explicitly upgraded.
- Keep behavior explicit and inspectable on disk.
- Keep command resolution PATH-native.
- Reuse `mise` for runtime resolution, activation, and default package installation.
- Allow npm global installs explicitly when requested with `install -g`.

## Non-Goals

- Volta-style smart shims that inspect project manifests at runtime.
- Interception of `npm install -g`, `pnpm add -g`, etc.

## Behavioral Guarantees

- Every managed tool has an isolated directory for runtime pins, wrappers, and metadata.
- Every managed tool has an explicit runtime pin.
- Public commands in `~/.miseo/.bin` point to a specific owning tool directory.
- `miseo` does not intercept shell resolution; precedence is decided by PATH order.

## Command Surface

- `miseo install <tool-spec>`
- `miseo install -g <tool-spec>`
- `miseo install <tool-spec> --use <runtime@selector>` (repeatable)
- `miseo upgrade <tool-spec>`
- `miseo upgrade <tool-spec> --use <runtime@selector>` (repeatable)
- `miseo uninstall <tool-spec>`
- `miseo remove <tool-spec>` (alias of `uninstall`)

## Tool Identity

Tool request spec (user input):

- `<backend>:<name>[@version]`

Examples:

- `npm:http-server`
- `npm:http-server@14.1.1`
- `cargo:ripgrep`

Canonical `tool_id` (manifest identity):

- always unversioned `<backend>:<name>` (for example `npm:http-server`, `cargo:ripgrep`)
- `upgrade` and `uninstall` resolve by `tool_id` and reject `@version` input

Canonical `tool_key` (filesystem key):

- `tool_key` is derived from `tool_id` using the same scheme mise uses for backend install directories: kebab-case normalization of the short backend string (`to_kebab_case`)
- examples: `npm:http-server -> npm-http-server`, `npm:@antfu/ni -> npm-antfu-ni`, `vfox:version-fox/vfox-nodejs -> vfox-version-fox-vfox-nodejs`
- `tool_key` is treated as opaque; behavior is driven by manifest metadata, not reverse parsing filesystem names

Canonical `install_variant` (concrete install target):

- default mode: `<pkg-exact-version>+<runtime-tuple>`
- global npm mode: `global+<runtime-tuple>`
- `runtime-tuple` is one or more `<runtime>-<runtime-pin>` segments, sorted by runtime and joined by `+`
- examples: `14.1.1+node-22.13.1`, `global+node-22.13.1`, `1.2.3+node-22.13.1+ruby-3.3.0`

## Filesystem Model

Root:

- `~/.miseo/`

Per-tool directory:

- `~/.miseo/<tool-key>/`

Per-install directory:

- `~/.miseo/<tool-key>/<pkg-version>+<runtime-tuple>`
- `~/.miseo/<tool-key>/global+<runtime-tuple>` for `install -g`
- common example: `~/.miseo/npm-http-server/14.1.1+node-22.13.1`

Current pointer:

- `~/.miseo/<tool-key>/current -> <pkg-version>+<runtime-tuple>`

Public command symlink:

- `~/.miseo/.bin/<command>`

Layout example:

```text
~/.miseo/
  .miseo-installs.toml
  .bin/
    http-server -> ~/.miseo/npm-http-server/current/.miseo/http-server
  npm-http-server/
    current -> 14.1.1+node-22.13.1
    14.1.1+node-22.13.1/
      mise.toml
      .miseo/
        http-server
    global+node-22.13.1/
      mise.toml
      .miseo/
        http-server
```

Ownership model:

- command ownership is the symlink target
- to find owner of `http-server`, resolve `~/.miseo/.bin/http-server`
- tool identity and metadata are resolved from `~/.miseo/.miseo-installs.toml` (source of truth)

## Runtime Pinning

Each tool has a local `mise.toml` pin for its runtime(s).

Example:

```toml
[tools]
node = "20.18.1"
```

Policy when `--use` is omitted:

1. Map backend to required runtime set (`npm -> [node]`, etc.; see mapping table below).
2. Resolve the effective global runtime selector for each required runtime from mise configuration.
3. Require that each selected runtime is already installed (non-floating install policy).
4. Resolve and persist runtime pin values in tool-local `mise.toml`.

If no global runtime is configured for that runtime, installation fails with a clear instruction to either configure runtime globally in `mise` or pass `--use`.

`--use` behavior:

- if `--use` is omitted (Mode A), runtime auto-install is not allowed; failure to resolve a working installed runtime is an error
- if backend has no runtime mapping in `miseo` v1 and `--use` is omitted, fail explicitly (otherwise `miseo` is not adding pinning value)
- `--use` may be provided multiple times in Mode B; each value contributes one runtime selector (duplicate runtimes are rejected)
- `miseo` resolves `--use` selectors to concrete versions and persists those concrete pins in the tool-local `mise.toml`
- if `--use` is explicitly provided (Mode B), installing requested runtime(s) if missing is allowed

Backend-to-runtime mapping used by `miseo` for runtime selection:

| Backend                                                                                   | Runtime set    | Pinning mode                                                         |
| ----------------------------------------------------------------------------------------- | -------------- | -------------------------------------------------------------------- |
| `npm`                                                                                     | `node`         | pinned runtime required                                              |
| `gem`                                                                                     | `ruby`         | pinned runtime required                                              |
| `pipx`                                                                                    | `python`       | pinned runtime required                                              |
| `cargo`                                                                                   | `rust`         | pinned toolchain (mostly build-time relevance for produced binaries) |
| `go`                                                                                      | `go`           | pinned toolchain (mostly build-time relevance for produced binaries) |
| `aqua`, `github`, `gitlab`, `http`, `ubi`, `s3`, `spm`, `dotnet`, `conda`, `asdf`, `vfox` | unmapped in v1 | `--use` required; otherwise install/upgrade fails                    |

The concrete global install implementation currently supports npm packages.

## Integration with mise

`miseo` uses `mise` as the runtime resolver/executor while keeping `miseo` state under `~/.miseo`.

Default install shape:

```bash
mise exec <runtime@version> -- mise install-into <tool-spec@exact-version> <variant-dir>
```

Global npm install shape, only when `install -g` is requested:

```bash
mise exec --cd ~/.miseo/<tool-key>/<variant> -- npm install -g <package>@<exact-version>
```

The tool-local directory contains a generated `mise.toml` with exact runtime pins before npm runs in global mode, so the npm global install lands in the activated runtime's global package location instead of relying on the caller's current project.
Global mode still installs the resolved exact package version. The variant key uses `global` instead of the package version because the runtime global npm install can be changed outside of `miseo`; package metadata remains a record of what `miseo` installed, not a content-addressed directory name. Current checks activate the same runtime and read the installed global package version from npm rather than trusting that recorded metadata.

Package version resolution policy:

1. Resolve package target to an exact version before install.
2. Use the resolved exact spec for `install-into` by default.
3. In global npm mode, strip the `npm:` backend prefix and pass the resolved package spec to `npm install -g`.

## Bin Discovery Contract

Default installs determine exported commands by asking `mise` for backend-resolved bin directories for the installed path:

```bash
mise --no-config bin-paths "<tool-spec>@path:<absolute-install-dir>"
```

Then `miseo` scans only those returned directories and uses executable file names as command names.

Global npm installs determine exported command names from the installed package metadata. They ask npm for the activated global package root:

```bash
mise exec --cd ~/.miseo/<tool-key>/<variant> -- npm root -g
```

Then `miseo` reads the installed package's `package.json` and uses the `bin` field to determine command names. Object-form `bin` keys are used directly. String-form `bin` exports the unscoped package name.

`miseo` also asks npm for the activated global prefix and turns each command name into an absolute executable path in that runtime's global npm bin directory. Shims execute that absolute global path after activating the tool-local `mise.toml`.

This contract is important because command names are not always equal to package names, and packages may export multiple binaries.

## Lifecycle Flows

Install flow:

1. Parse tool spec and resolve backend.
2. Resolve runtime pin set (from `--use` or global runtime policy).
3. Resolve exact package version.
4. Compute install target `~/.miseo/<tool-key>/<pkg-version>+<runtime-tuple>`.
5. Install via `mise exec ... mise install-into ...` by default, or initialize/trust the variant and run `npm install -g` when `-g` was requested.
6. Persist/verify tool-local runtime pins.
7. Discover exported command names via the selected install mode's bin discovery contract.
8. Validate conflicts (only against bins owned by other `tool_id`s).
9. Atomically upgrade `current` pointer.
10. Create/upgrade `~/.miseo/.bin` symlinks for current exported commands.
11. Remove stale `~/.miseo/.bin` symlinks previously owned by this `tool_id` but not exported by new current variant.
12. Write/upgrade `~/.miseo/.miseo-installs.toml`.

Upgrade flow:

1. Resolve tool from `~/.miseo/.miseo-installs.toml`.
2. Choose package/runtime target (preserve pin unless explicit repin/upgrade intent).
3. Install the new variant using the current variant's install mode.
4. Rediscover commands and validate conflicts against other owners.
5. Atomically switch `current`.
6. Refresh public symlinks (including stale-bin removal) and manifest metadata.

Uninstall flow:

1. Resolve tool from `~/.miseo/.miseo-installs.toml`.
2. For each recorded global npm variant, activate its tool-local `mise.toml` and run `npm uninstall -g <package>`.
3. Remove public symlinks that target it.
4. Remove tool dir and manifest entry.

Uninstall `--force` behavior:

- if the tool is not present in manifest but a matching unmanaged tool directory exists,
  `miseo uninstall --force` removes that orphan directory and matching stale public symlinks.
- without `--force`, this condition returns an error with a cleanup hint.

## Precedence Model

`miseo` is PATH-native.

Precedence is expected to be:

1. project-local bins (if configured)
2. `~/.miseo/.bin`

Node example:

```toml
[env]
_.path = ["{{config_root}}/node_modules/.bin"]
```

With `mise activate`, local bins can naturally override global `~/.miseo/.bin` entries.

## Conflict Policy

Binary name conflicts are explicit.

Default behavior:

- if a command is already owned by a different `tool_id`, install/upgrade fails with a clear ownership message
- if a command is already owned by the same `tool_id` (typical upgrade), relinking is allowed

## Atomicity and Crash Behavior

- Tool metadata and wrappers are isolated by variant directory (`~/.miseo/<tool-key>/<install-variant>`), so incomplete installs do not replace the active `miseo` variant.
- Default package content write is isolated by variant directory. Global npm mode installs package content into the activated runtime's global npm location.
- User-visible activation is controlled by pointer/symlink updates (`current` and `~/.miseo/.bin/*`).
- `current` switch is atomic (single rename/symlink replacement operation).
- Public bin updates are per-command operations; a crash mid-update can leave partial bin state temporarily.
- Recovery is deterministic: next successful `install`/`upgrade` for that `tool_id` rewrites expected public links and removes stale owned links.

## Portability

- Unix/macOS: symlink-based public command entries
- Windows: wrapper/junction-compatible strategy when symlink behavior differs
