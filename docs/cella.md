# Deploy with cella

`cella` builds apps on your machine and deploys them to a Linux host over SSH.
First [set up the host](setup-host.md).

## Install

From this repository:

```sh
cargo install --locked --path crates/cella --force
```

Supported platforms: Linux x86_64/arm64 and macOS arm64. You'll need OpenSSH,
`curl`, and your project's build tools and dependencies.

## Owner SSH setup

Use an SSH key authorized for your VM owner account. The account needs
passwordless sudo for `/usr/local/bin/celld-ctl transport`.
**Do not share this owner key with CI or untrusted users.**

Verify the VM's SSH host-key fingerprint through your provider or operator
before accepting it. Then test access and set your connection details:

```sh
ssh -F /dev/null -i "$HOME/.ssh/YOUR_VM_KEY" -o IdentitiesOnly=yes OWNER@YOUR_VM_HOST
export CELLA_HOST=OWNER@YOUR_VM_HOST
export CELLA_SSH_KEY="$HOME/.ssh/YOUR_VM_KEY"
```

Use a literal hostname or `user@host`; `cella` does not read `~/.ssh/config`.
For a passphrase-protected key, load it with `ssh-add` before deploying.
Set `CELLA_SSH_PORT` if SSH uses a port other than 22.

On exe.dev, use an [account-registered key](https://exe.dev/docs/cli-ssh-key.md)
and the [VM SSH destination](https://exe.dev/docs/faq/ssh-destination.md),
such as `YOUR_VM.exe.xyz`.

## Create an app

```sh
cella init my-app
cd my-app
pnpm install
cella deploy
```

The starter includes a Durable Object counter and `wrangler.jsonc`.
Use `cella init .` to scaffold the current directory. npm and Yarn work too.

For an existing Wrangler project, skip initialization, install its dependencies,
and run `cella deploy` from the project directory. Your Wrangler file is unchanged;
deployment storage credentials stay on the host.

## Deploy and inspect

```sh
cella deploy
cella status
cella logs --lines 50
cella deployments list
```

First deploy creates and activates the app. The Wrangler Worker name is its
default slug. Each app runs at `/` on a dedicated port; port 8000 lists app
links. exe.dev supplies authenticated HTTPS forwarding. Other hosts need
[their own trusted HTTPS proxy and access control](setup-host.md#proxy-and-readiness).

Useful options:

```sh
cella --project ./my-app deploy
cella --slug another-app deploy
cella deploy --verbose
cella deploy --json
cella status --json
```

Use status to check what's running; the newest deployment in history may not
have been adopted yet.

## Important limits

- JavaScript/TypeScript, static assets and standard WASM are supported;
  container/Python builds are not.
- Keep secrets out of Wrangler vars and asset directories, including dotfiles.
  There is no runtime app-secret manager.
- A failed deploy may already have published a version. Check status and logs
  before retrying.
- To roll back code, check out an earlier revision and redeploy. This does not
  undo data migrations.

See [architecture](architecture.md) for the trust boundary and
[operations](operations.md) for backup and recovery.
