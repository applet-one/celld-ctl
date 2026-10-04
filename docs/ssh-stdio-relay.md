# Owner access through SSH stdio (no Tailscale or TCP forwarding)

Some managed SSH gateways allow normal VM remote commands but reject TCP
forwarding (`ssh -L`/`-W` with `administratively prohibited`). You can still reach
the VM's loopback deployment listener through an SSH **remote command** running
`/usr/bin/nc -N 127.0.0.1 2222`. The included local relay turns that byte stream
into a loopback socket for the existing `cella` client.

This is for **VM owners/administrators who already have SSH access**. It uses
their administrator credential for the outer connection; the restricted deploy
key is used separately inside it. It does not make administrator keys safe to
share with CI or other developers. Those users need their own approved private
network path and restricted deployment keys.

Prerequisites: Python 3 and OpenSSH on your dev machine, normal SSH command access
to the VM, and OpenBSD netcat (`/usr/bin/nc` with `-N`) on the VM. No changes to
`cella` or the deployment service are required. exe.dev documents VM destinations
and piping via ordinary SSH in its [SSH destination](https://exe.dev/docs/faq/ssh-destination.md)
and [copy-files](https://exe.dev/docs/faq/copy-files.md) guides. `ssh exe.dev` is
the account interface; target your VM's actual SSH destination for this relay.

## 1. Start the local relay

Stop any previous `ssh -N -L ...` tunnel occupying the chosen local port.
On your **dev machine**, from this repository:

```sh
python3 scripts/ssh-relay.py --via YOUR_VM_SSH_DESTINATION
```

Use the same administrator SSH destination/authentication that already works
for logging into that VM. Your outer SSH config and local agent may be used;
the agent is never forwarded to the VM. If necessary, specify a separate
administrator private key with `--identity /path/to/admin-key`. Unlock that key
in your local agent beforehand if it requires a passphrase; the relay is batch
SSH and does not prompt.

Keep this terminal open. The relay binds **only 127.0.0.1:2222**, limits concurrent
connections, and supports the multiple connections used by keyscan/deploy/status.
It never requests SSH TCP forwarding: its remote command opens the TCP socket
inside the VM. It forwards encrypted inner SSH bytes, not raw deployment data.
Ctrl-C stops the relay and its active SSH subprocesses.

If local port 2222 is occupied, use `--listen-port 2224`; set `CELLA_SSH_PORT=2224`
in every following command. The remote listener remains 2222 unless the operator
has deliberately configured another port (`--remote-port`).

## 2. Verify the dedicated service's host key

In another dev-machine terminal:

```sh
export CELLA_HOST=cella-deploy@127.0.0.1
export CELLA_SSH_PORT=2222
export CELLA_SSH_KEY="$HOME/.ssh/cella-deploy"
ssh-keyscan -p "$CELLA_SSH_PORT" -t ed25519 127.0.0.1 > /tmp/cella-host-key
ssh-keygen -lf /tmp/cella-host-key
```

Compare the SHA256 fingerprint with the **dedicated deployment service's**
fingerprint supplied by the operator (A7). It is not the outer gateway's key.
`ssh-keyscan` alone does not authenticate the service. Only after a match:

```sh
cat /tmp/cella-host-key >> "$HOME/.ssh/known_hosts"
chmod 600 "$HOME/.ssh/known_hosts"
```

Known-host entries use your local endpoint, e.g. `[127.0.0.1]:2222`. Do not reuse
that entry for another VM without verifying its different server key. A changed
key is not a reason to disable strict verification.

## 3. Deploy normally

From the directory containing your Wrangler configuration:

```sh
cella deploy
cella status
cella logs --lines 50
```

Local builds and host-owned storage publication are unchanged. There are no
local R2 credentials or bucket settings. The host must have enrolled your public
deploy key as described in [A6](setup-host.md#a6-enroll-each-developers-public-deploy-key).

If the outer SSH connection fails, read the relay terminal's error. If the inner
connection fails, check the dedicated key, local port and known-host fingerprint.
A shell, `scp`, or SFTP through `cella-deploy` is still intentionally forbidden.
