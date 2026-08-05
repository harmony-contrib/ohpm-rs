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
  format), the registry client, RSA-signature login, and the publish
  orchestrator. Fully unit- and integration-tested against a mock registry.
- **`crates/ohpm-cli`** — the `ohpm-rs` binary: clap command definitions and thin
  command handlers.

Reference layout: the ohpm source in `lib/` of the DevEco Studio 6.1.1.280
installation (`config/`, `core/registry/`, `core/publish/`, `core/package/`).

## Build & test

```sh
cargo build --workspace        # binary: target/debug/ohpm-rs
cargo test  --workspace        # 62 tests: unit + mock-registry integration
```

## Environment-variable authentication (CI)

All auth inputs are read from `OHPM_*` environment variables, which override
`.ohpmrc` files. **Precedence: CLI flags > env vars > `.ohpmrc` > defaults.**

| Variable | Purpose |
|---|---|
| `OHPM_ACCESS_TOKEN` | Read-write access token — used directly, skips login. Highest priority. |
| `OHPM_PUBLISH_ID` | Publish id for the SSH-key login flow. |
| `OHPM_KEY_PATH` | Path to the encrypted private key for the login flow. |
| `OHPM_KEY_PASSPHRASE` | Private-key passphrase. **No interactive prompt is ever shown.** |
| `OHPM_READ_ACCESS_TOKEN` | Read-only token (used by `info` / `ping`). |
| `OHPM_REGISTRY` | Default registry override. |
| `OHPM_PUBLISH_REGISTRY` | Publish registry override. |
| `OHPM_STRICT_SSL`, `OHPM_CA_FILES`, `OHPM_HTTP(S)_PROXY`, `OHPM_NO_PROXY`, `OHPM_LOG_LEVEL`, `OHPM_FETCH_TIMEOUT` | Network / logging overrides. |

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
# Fast path: you already have a token.
export OHPM_ACCESS_TOKEN="<token>"
ohpm-rs publish my-lib.har --publish_registry https://repo.example.com/ohpm/

# Tokenless path: private-key login, fully from env.
export OHPM_PUBLISH_ID="<publish-id>"
export OHPM_KEY_PATH="/path/to/key.pem"
export OHPM_KEY_PASSPHRASE="<passphrase>"
ohpm-rs publish my-lib.har

# Obtain and persist a token once (stored in ~/.ohpm/.ohpmrc).
ohpm-rs login --publish_id <id> --key_path /path/to/key.pem
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
exclude:                 # optional: drop members by path glob or package name
  - "packages/internal"
  - "@scope/private"
version:                 # optional versioning policy
  mode: unified          # unified | independent (default independent)
```

When publishing a package that lives inside a workspace, `file:` protocol
dependencies are **resolved and rewritten to the target package's version**
before the metadata is uploaded, so published manifests never reference local
paths:

```json5
// source oh-package.json5
{ "name": "@app/app", "version": "0.9.0",
  "dependencies": { "@demo/lib": "file:../lib" } }
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

### Versioning

`ohpm-rs version` supports two workspace modes, selectable by the `--all` flag or
by `version.mode` in `ohpm-workspace.yaml`:

```sh
# Unified: every workspace member is set to the same new version
# (base = the workspace root's own version, else the highest member version).
ohpm-rs version --all minor

# Same effect when the workspace config declares version.mode: unified.
ohpm-rs version patch                       # run from the workspace root

# Filter to specific packages (comma-separated, repeatable):
ohpm-rs version --all patch --filter @demo/a,@demo/b

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

- skipped by `ohpm-rs version --all` (and the unified mode),
- excluded from the unified base version,
- an explicit `ohpm-rs publish <file>` of such a package fails with
  `PublishForbidden`.

`exclude:` entries in `ohpm-workspace.yaml` remove a package from the workspace
entirely; `publish: false` keeps it a member (so others can still depend on it
via `file:`) but marks it non-publishable.

## Publishing to crates.io

Both crates are publish-ready (`cargo publish --dry-run -p ohpm-core` passes
clean; `ohpm-cli` needs `ohpm-core` on crates.io first):

```sh
cargo login                 # once, with your crates.io API token
cargo publish -p ohpm-core  # library first
cargo publish -p ohpm-cli    # then the CLI (path dep is rewritten to the version)
```

Notes:

- The crates are **unlicensed** (all rights reserved). crates.io rejects
  `license = "UNLICENSED"` (not a valid SPDX expression), so each crate ships
  its own `LICENSE` file via `license-file`.
- Publishing order matters: `ohpm-cli` depends on `ohpm-core` by version, so
  the library must be published first.
- `crates/ohpm-core/examples/wsprobe.rs` is included in the package — a
  read-only workspace probe (`cargo run -p ohpm-core --example wsprobe -- <dir>`).

## Notes on fidelity

- Upload paths: packages above `use_stream_threshold_size` (default 5 MB) go
  through `POST {registry}stream/{name}` (multipart `metadata` + `pkg_stream`);
  smaller packages go through `PUT {registry}{name}` with a JSON body carrying
  base64 `_attachments`. Both match the reference uploaders, including the
  598-locked retry (3 attempts, 30 s) and the stream→attachment fallback.
- `oh-package.json5` is parsed as JSON5; unknown fields round-trip into the
  published metadata.
- The `.tgz` (HSP) bundle path (`InterfaceHar` + `.hsp`) is implemented.
- Out of scope for v1: `install`/`update`/dependency resolution, script hooks,
  the `config encrypt` crypto component. The module layout leaves room for them.
