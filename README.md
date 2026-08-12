# ohpm-rs

A Rust reimplementation of the **ohpm** (OpenHarmony package manager)
command-line tool, based on the ohpm source code bundled with DevEco Studio
6.1.1.280.

The headline feature: **`ohpm-rs publish` authenticates entirely from environment
variables — no TUI prompts.** This makes CI/CD publishing unattended, which the
original Node implementation cannot do (it prompts for the private-key
passphrase interactively).

```
ohpm-rs (OpenHarmony package manager), a Rust reimplementation of ohpm

Commands:
  install     Install package(s) from the registry or local sources
  update      Update package(s) to their latest version
  uninstall   Uninstall package(s)
  publish     Publish a package to the registry
  prepublish  Pre-verify package content without publishing
  init        Create an oh-package.json5 file
  config      Manage the ohpm configuration file
  login       Log in with a private key and store the access token
  unpublish   Unpublish a package from the registry
  info        Display information about a package
  list        Display the dependency graph (basic)
  ping        Test network connectivity to the registry
  root        Print the effective oh_modules folder
  version     Bump the package version
  cache       Manage the ohpm cache folder
```

## Architecture

A cargo workspace with two crates:

- **`crates/ohpm-core`** — library: configuration (`.ohpmrc` + env overrides),
  package manifest/archive handling, integrity hashing (sha1 + sha512 in ssri
  format), the registry client, RSA-signature login, the publish orchestrator,
  and the install pipeline. Fully unit- and integration-tested against a mock
  registry.
- **`crates/ohpm-core/src/install/`** — the install subsystem (mirrors
  `lib/core/install/`, architected like pnpm's Rust engine): dependency
  resolution (`spec`/`semver`/`packument`/`resolver`), the
  `oh-package-lock.json5` lockfile (`lockfile`), the dependency graph
  (`graph`/`node`), the content-addressed store + extraction (`store`), the
  `oh_modules` symlink phase (`symlink`), the `oh_modules/.ohpm/lock.json5`
  install record (`lock_record`) and the pipeline orchestration (`root`/
  `mod`). The lockfile, install record, `oh_modules` layout and manifest
  rewrites are byte-compatible with the reference (verified against ohpm 6.0.1
  on the public registry). `install`, `update` and `uninstall` share the same
  pipeline (`run_pipeline`), like the reference's `installModules`; `update`
  clears/deletes the matching lockfile specifiers before re-resolving, and
  `uninstall` drops the packages from the root requirements and rewrites the
  manifest.

Version conflicts are resolved to the max-satisfying version by default
(`resolveVersionConflict2LockFile` + the flattened graph view), the lockfile
specifiers/packages are rewritten accordingly, and the strict strategy
(`resolve_conflict_strict`) is implemented including the strict alarms; the
local dependency-name inconsistency alarm (enforced by
`enforce_dependency_key` / the build-profile `useNormalizedOHMUrl` config) and
the registry name case-consistency alarm mirror `lib/core/alarm/`. The
`overrides` / `overrideDependencyMap` / `exclusions` fields of
`oh-package.json5` are applied during the graph build (mask + exclusion, the
`maskedByOverrideDependencyMap` tag written to the lockfile and the install
record), parameterized installs (`@param:` markers via `--parameter-file` or
the manifest `parameterFile` field) substitute the project manifest, and
target installs (`--target_path` + `dependencyMap.json5`) switch the module
roots, the lockfile name (`oh-package-<target>-lock.json5`) and write the
resolved module manifests into `<target>/resolve-conflict/<module>`.

Protocol extensions beyond the reference (which rejects them), mirroring
pnpm/pacquet:

- **git specs** (`install/git.rs`): `git+https://...`, scp-style and
  `https://...git` URLs with `#commit|branch|tag|semver:<range>|path:<dir>`
  fragments; the commit is pinned in the lockfile and re-installs work with
  the fixture repo deleted. Pure-Rust via the gix crate (no system git).
- **`ohpm:` aliases**: `"foo": "ohpm:bar@^1.0.0"` installs bar under the
  alias key foo; the specifier keeps the alias, the packages key and the
  store dir use the real name.
- **`workspace:` protocol**: `workspace:*|^|~|<range>|./path|<member>@*`
  resolves against `ohpm-workspace.yaml` members as links; the publish flow
  rewrites them to versions (`^`/`~` prefixes kept, alias form becomes
  `ohpm:<member>@<version>`).
- **`crates/ohpm-cli`** — the `ohpm-rs` binary: clap command definitions and thin
  command handlers.

Reference layout: the ohpm source in `lib/` of the DevEco Studio 6.1.1.280
installation (`config/`, `core/registry/`, `core/publish/`, `core/package/`).

## Build & test

```sh
cargo build --workspace        # binary: target/debug/ohpm-rs
cargo test  --workspace        # 201 tests: unit + mock-registry integration
```

## Environment-variable authentication (CI)

All auth inputs are read from `OHPM_*` environment variables, which override
`.ohpmrc` files. **Precedence: CLI flags > env vars > `.ohpmrc` > defaults.**

| Variable            | Purpose                                                                                                 |
| ------------------- | ------------------------------------------------------------------------------------------------------- |
| `OHPM_ACCESS_TOKEN` | Read-write access token — used directly, skips login. Highest priority.                                 |
| `OHPM_PUBLISH_ID`   | Publish id for the SSH-key login flow.                                                                  |
| `OHPM_KEY_PATH`     | Path to the encrypted private key for the login flow.                                                   |
| `OHPM_KEY_CONTENT`  | The private key PEM **content** directly — no file needed (CI secrets). Alternative to `OHPM_KEY_PATH`. |

Key formats: PKCS#8 encrypted (`BEGIN ENCRYPTED PRIVATE KEY`), unencrypted
PKCS#8/PKCS#1, and **traditional OpenSSL encrypted PKCS#1** (`BEGIN RSA
PRIVATE KEY` with `Proc-Type: 4,ENCRYPTED` / `DEK-Info:` — AES-128/192/256-CBC,
DES-EDE3-CBC, DES-CBC), all decrypted in pure Rust.
| `OHPM_KEY_PASSPHRASE` | Private-key passphrase. **No interactive prompt is ever shown.** |
| `OHPM_READ_ACCESS_TOKEN` | Read-only token (used by `info` / `ping`). |
| `OHPM_REGISTRY` | Default registry override. |
| `OHPM_PUBLISH_REGISTRY` | Publish registry override. |
| `OHPM_STRICT_SSL`, `OHPM_CA_FILES`, `OHPM_HTTP(S)_PROXY`, `OHPM_NO_PROXY`, `OHPM_LOG_LEVEL`, `OHPM_FETCH_TIMEOUT` | Network / logging overrides. |

The SSH-login values can also be passed as CLI flags: `publish`, `login` and
`unpublish` accept `--publish_id`, `--key_path` (or `--key_content` for the
PEM text itself), and `--passphrase` (prefer the `OHPM_*` env vars in CI —
command-line arguments are visible in the process list).

### Auth resolution for `publish` (write)

1. `OHPM_ACCESS_TOKEN`
2. `{registry}:_auth` stored in `.ohpmrc` (e.g. `"//repo.harmonyos.com/ohpm/:_auth"`)
3. SSH-key login: `OHPM_PUBLISH_ID` + `OHPM_KEY_PATH` + `OHPM_KEY_PASSPHRASE`
   → RSA-PSS signature over `v1-{publishId}-{timestamp}-{nonce}`
   → `POST {registry}login_pss` → access token. Falls back to the default
   `login` endpoint on HTTP 400/404, exactly like the reference.

If a value (or the passphrase) is missing, publish **fails fast with a clear
error** instead of prompting — the key behavioral difference from the original
ohpm.

## Publishing in CI

```sh
# Fast path: you already have a token. Publish from source by default —
# no argument packs the current package directory automatically.
export OHPM_ACCESS_TOKEN="<token>"
ohpm-rs publish --publish_registry https://repo.example.com/ohpm/

# Tokenless path: private-key login, fully from env.
export OHPM_PUBLISH_ID="<publish-id>"
export OHPM_KEY_PATH="/path/to/key.pem"       # or OHPM_KEY_CONTENT="<pem>"
export OHPM_KEY_PASSPHRASE="<passphrase>"
ohpm-rs publish

# Pre-built packages: pass the har/tgz explicitly (or a source directory).
ohpm-rs publish my-lib.har
ohpm-rs publish ./src/mylib

# Obtain and persist a token once (stored in ~/.ohpm/.ohpmrc).
ohpm-rs login --publish_id <id> --key_path /path/to/key.pem
```

`publish` (and `prepublish`) input resolution: no argument = current package
directory, packed on the fly; a directory argument = pack that directory; a
`.har`/`.tgz` argument = publish the pre-built package as-is. In a workspace,
`--workspace` publishes every publishable member (or `--filter <pkgs>` a
selected subset); `publish: false` members are skipped. Before each real
upload, `publish` queries the target publish registry for the exact
`name@version`. An already-published version is skipped with a warning, while
other versions of the same package continue normally. Registry/auth/protocol
errors fail the command instead of being treated as an unpublished version.

`publish --dry-run` validates everything locally — packing, metadata, and the
auth configuration (the key is parsed, the token/env/config inputs are
checked) — but makes **no network requests** and uploads nothing:

```sh
ohpm-rs publish --dry-run            # [DRY RUN] +name version (size, files, auth)
ohpm-rs publish --workspace --dry-run
```

## Building HAR packages

`ohpm-rs pack` builds a `<name>-<version>.har` from a source directory
(npm-pack style):

```sh
# Pack the current package directory (nearest oh-package.json5).
ohpm-rs pack

# Pack a specific module; --output controls the destination dir.
ohpm-rs pack native_ability --output dist/

# Source directory → har → publish in one step.
ohpm-rs publish native_ability --publish_registry https://repo.example.com/ohpm/
```

Packaging rules (hvigor conventions):

- gzip tar with a `package/` prefix; entries sorted for deterministic output.
- Always excluded: `oh_modules`, `node_modules`, `.hvigor`, `.git`, `.idea`,
  `.ohpm`, `.tmp`, `.cxx`, `build`, `target`, `.DS_Store`, nested `*.har`,
  `oh-package-lock.json5`.
- A gitignore-style [`.ohpmignore`](https://developer.huawei.com/consumer/cn/forum/topic/0201145899734297171)
  at the module root filters out additional files (`#` comments, `!` negation,
  `* ? **` globs; a pattern without `/` also matches the file name at any
  depth).
- `publish`/`prepublish` accept a directory input and pack it first.

## Workspace mode

A monorepo is identified by an **`ohpm-workspace.yaml`** at the workspace root
(schema modeled on `pnpm-workspace.yaml`):

```yaml
packages:
  - "packages/*"
  - "modules/**"
exclude: # optional: drop members by path glob or package name
  - "packages/internal"
  - "@scope/private"
version: # optional versioning policy
  mode: unified # unified | independent (default independent)
```

When publishing a package that lives inside a workspace, `file:` protocol
dependencies are **resolved and rewritten to the target package's version**
before the metadata is uploaded, so published manifests never reference local
paths:

```json5
// source oh-package.json5
{
  name: "@app/app",
  version: "0.9.0",
  dependencies: { "@demo/lib": "file:../lib" },
}
```

```json
// uploaded metadata dependencies
{ "@demo/lib": "1.2.0" }
```

Rules:

- A `file:` spec matches `/^file:(\s+)?/i`; the target path is resolved
  relative to the published package's source root (`~` expands to `$HOME`).
- Targets may be a workspace member directory or any local package with an
  `oh-package.json5` (also `.har`/`.tgz` archives).
- Unresolvable `file:` entries in `dependencies` fail the publish; the same in
  `devDependencies` is kept with a warning.
- Outside a workspace (no `ohpm-workspace.yaml`), no rewriting happens.

### Workspace support per command

| Command                                            | Workspace mode                                                                                                                                                        |
| -------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `publish` / `prepublish`                           | `file:` deps rewritten to member versions before upload; accepts a member directory (auto-pack); `--workspace`/`--filter` publish every (selected) publishable member |
| `pack`                                             | `--workspace` packs every publishable member; `--filter <pkgs>` selects; `publish: false` skipped                                                                     |
| `list`                                             | `-r/--recursive` lists every member's graph; at the workspace root all members are listed by default                                                                  |
| `version`                                          | `--workspace` unified, `--filter`, `--preid`, `version.mode` from the yaml, `publish: false` skipped                                                                  |
| `unpublish`, `info`                                | operate by package name — work from anywhere                                                                                                                          |
| `init`, `config`, `login`, `ping`, `root`, `cache` | global / cwd-scoped — not workspace-scoped                                                                                                                            |

### Versioning

`ohpm-rs version` supports two workspace modes, selectable by the `--workspace` flag or
by `version.mode` in `ohpm-workspace.yaml`:

```sh
# Unified: every workspace member is set to the same new version
# (base = the workspace root's own version, else the highest member version).
ohpm-rs version --workspace minor

# Same effect when the workspace config declares version.mode: unified.
ohpm-rs version patch                       # run from the workspace root

# Filter to specific packages (comma-separated, repeatable):
ohpm-rs version --workspace patch --filter @demo/a,@demo/b

# Independent: only the package containing the current directory is bumped,
# works inside a workspace member too.
cd packages/a && ohpm-rs version patch
```

**Pre-release versions** (beta / rc / alpha) follow npm semver:

```sh
ohpm-rs version prerelease --preid beta     # 1.0.0        -> 1.0.1-beta.0
ohpm-rs version prerelease --preid beta     # 1.0.1-beta.0 -> 1.0.1-beta.1
ohpm-rs version prerelease --preid rc       # 1.0.1-beta.1 -> 1.0.1-rc.0   (reset to rc)
ohpm-rs version prepatch  --preid rc        # 1.0.0        -> 1.0.1-rc.0
ohpm-rs version premajor  --preid beta      # 1.0.0        -> 2.0.0-beta.0
ohpm-rs version patch                       # 1.0.1-rc.0   -> 1.0.2        (drops pre-release)
ohpm-rs version 2.0.0-rc.1                  # set an exact pre-release version
```

`--preid` must match `[0-9A-Za-z-]+` (no leading zeros when numeric). The
unified-mode base version compares full semver (a release outranks a
pre-release of the same number).

### `publish: false`

A package whose `oh-package.json5` sets `"publish": false` is **not
publishable**:

- skipped by `ohpm-rs version --workspace` (and the unified mode),
- excluded from the unified base version,
- an explicit `ohpm-rs publish <file>` of such a package fails with
  `PublishForbidden`.

`exclude:` entries in `ohpm-workspace.yaml` remove a package from the workspace
entirely; `publish: false` keeps it a member (so others can still depend on it
via `file:`) but marks it non-publishable.

## The local store

ohpm-rs keeps every downloaded package in a **content-addressed store** shared
by all projects on the machine — the `cache` config directory (default
`~/.ohpm/cache`), pnpm-store aligned:

```
<cache>/
  content-v1/<alg>/<h[0:2]>/<h[2:4]>/<h[4:]>   # downloaded archives, keyed by digest
  extracted-v1/<alg>/<h[0:2]>/<h[2:4]>/<h[4:]> # shared extracted trees (hard-link mode only)
  harball/                                     # publish staging
```

The layout matches real ohpm byte-for-byte, so ohpm-rs and the reference tool
share one cache: each side reuses the other's downloads (`ohpm DEBUG: found
package ... from cache file`).

Configure the store (pnpm-style resolution: env > CLI > project `.ohpmrc` >
user `~/.ohpm/.ohpmrc` > default):

```sh
ohpm-rs config set cache /path/to/store     # persist (user rc)
OHPM_CACHE=/path/to/store ohpm-rs install   # per-invocation
ohpm-rs install --cache /path/to/store      # per-command
ohpm-rs cache path                          # print the effective store dir
```

Manage it (≈ `pnpm store`):

```sh
ohpm-rs cache path                          # effective store dir
ohpm-rs cache status                        # verify every cached archive's integrity
ohpm-rs cache add @ohos/foo@1.2.3           # pre-fetch into the store (no install)
ohpm-rs cache clean                         # drop content-v1 + harball (+ extracted-v1)
```

`cache add` accepts registry packages only (`name[@version | @tag:<tag>]`;
`tag:latest` is invalid, like the reference); repeat runs are served from the
store.

### Hard-link mode (`cache_hardlink`, default off)

By default each project extracts its own copy of a package into `oh_modules`
(reference behavior). With `cache_hardlink=true` (via `config set`,
`OHPM_CACHE_HARDLINK`, or `~/.ohpm/.ohpmrc`), the archive is extracted once
into the shared `extracted-v1` layer and every project's store dir is
hard-linked from it — pnpm's zero-copy reuse: two projects installing the same
version share the same files on disk (copy fallback on cross-device links).

> ⚠️ Hard links share inodes: a build tool that modifies files inside
> `oh_modules` would corrupt the shared layer for every project using it.
> Keep the default off unless you understand this trade-off.

## Notes on fidelity

- Upload paths: packages above `use_stream_threshold_size` (default 5 MB) go
  through `POST {registry}stream/{name}` (multipart `metadata` + `pkg_stream`);
  smaller packages go through `PUT {registry}{name}` with a JSON body carrying
  base64 `_attachments`. Both match the reference uploaders, including the
  598-locked retry (3 attempts, 30 s) and the stream→attachment fallback.
- `oh-package.json5` is parsed as JSON5; unknown fields round-trip into the
  published metadata.
- The `.tgz` (HSP) bundle path (`InterfaceHar` + `.hsp`) is implemented.
- Out of scope: script hooks, conflict resolution (strict/overrides), unified
  lockfiles, HSP packages, the `config encrypt` crypto component, and
  rayon-accelerated extraction (the pipeline currently uses `spawn_blocking` +
  a semaphore).
