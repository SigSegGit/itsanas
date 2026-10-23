# The fleet, tested by CI

The acceptance kit (`scripts/acceptance.sh`, [MVP.md](MVP.md) §3) run on the
real machines every night, from GitHub Actions, without anybody at a
keyboard. Asked for by Nicolas on 2026-10-23: the hand test of the day before
took him a day and was an ordeal for the partner testing with him, and with
tests like that nobody new can be brought in.

**Status: 🟨 built and tested in CI without the fleet; not yet run on the
fleet.** It needs the one-time setup below, which only Nicolas can do (a key
on his machines, secrets in his repository).

## What runs

`.github/workflows/fleet.yml`, nightly at 03:17 UTC and on demand (Actions →
Fleet → Run workflow). For each machine that answers:

| Row | What it checks | Kit phase |
| --- | --- | --- |
| **V** | the machines' running daemons are one build (the snapshot's `version`, from the release after 0.1.12 on): an `update` that restarted nothing shows here. The runner, which runs this checkout, is left out | -- |
| **S** | the `test` node's daemon took a round in the last three intervals (its `status.snapshot`) | -- |
| **B** | a file written on the first machine is byte for byte on every other | `B write`, `B check` |
| **F** | deleted there, it is gone everywhere, and still gone a round later | `F delete`, `F check` |
| **C** | a canary written on the first machine, once it has crossed, is nowhere in what another account's node holds for `test` (its vault's directory for that owner) nor in that node's databases; SKIP, not PASS, when it holds nothing for `test` | `C plant`, `C scan` |

The **first machine is the runner itself**, a member of the `test` account
for the length of the run (`scripts/fleet/member.sh`): a VM in a datacentre
behind NAT, on a new address each night, that can reach the account only
through the coordinator, the public forwards and the relays. So B and F are
test **O** (written away from home, arrived home). It is **the same device
every night**: its home and folder are sealed under `FLEET_HOME_KEY` and kept
in the Actions cache, so each run is also a machine coming back after a day
away. Not a new device per run: every host keeps a withdrawn device's claim
for good, so a device a night would grow every host's table by one a night
for nothing (until 2026-10-23 it also pushed live devices out of the sixteen
claims a host hands out; the sixteen presented last are handed out now). When
the cache is gone (first run, a week unused, a damaged archive) the member logs
in from the 24 words as a new device and withdraws the previous one.
Without the phrase secret the runner does not take part, and B starts on the
first SSH host instead -- which is the LAN path, and says so little that the
run reports it.

A run is **red** when a row says FAIL, or when fewer than two machines answered
(nothing crossed a network). A machine that does not answer is a **SKIP** row,
with the reason. A red run opens an issue titled `fleet: red`, or comments on
the one already open; the session that resumes the project reads it first.

## Why it is safe to hold a key to Nicolas's machines in CI

- **The key runs one script.** Its `authorized_keys` line is
  `restrict,command="~/.itsanas-fleet/agent.sh"`: sshd runs the agent whatever
  the caller asked, with no shell, no terminal, no port forwarding (checked
  through a real sshd: `sh -c id` refused, a tunnel "administratively
  prohibited").
- **The agent is an allowlist** (`scripts/fleet/agent.sh`). A request is plain
  words or refused whole (exit 64): no shell character, no `..`, no option, at
  most 256 characters. It names only files the kit itself wrote, writes only into
  the `test` account's folder, at most 50 files, and scans other accounts'
  homes read-only, limited to what they hold for `test`. One request at a time
  (`flock`). It calls the kit, so "B passes" means the same thing at night as
  by hand. No sudo, no docker: the containers run as that user.
- **The secrets reach only main.** They sit in a GitHub environment, `fleet`,
  whose deployment rule is the `main` branch. The workflow never runs on a pull
  request; a branch a session pushes, or a fork's PR, never reads them.
- **The phrase secret is the `test` account.** That account holds test data
  and nothing else; that is a rule, not a property the code can check.
- **What a leaked key can do:** write files of about a mebibyte into the test
  account (50 at most), delete files the kit wrote, read the test node's
  snapshot, and learn whether a given canary is in a home. Rotating it is one
  line per machine (`setup.sh` again with a new key).

`scripts/check-fleet.sh` runs on every push: 18 requests a leaked key would
try are refused and the folder is left as it was; and each scenario is run once
where it must pass and once where it must not (a file that never arrives, a
readable canary, a daemon ten intervals late, one machine alone). Sabotaged:
the kit-name check removed, the wait that always passes, the stale snapshot
ignored -- red each time.

## Setup, once (Nicolas)

1. **A key for CI**, on any machine, never committed:
   `ssh-keygen -t ed25519 -N '' -C itsanas-fleet-ci -f fleet-ci`
2. **On the Pi and on the VM**, as the user whose ITSaNAS containers run, in
   the checkout: `git pull && bash scripts/fleet/setup.sh "$(cat fleet-ci.pub)"`.
   It prints `hello`, the test node's snapshot, a refused `sh`, and the host key.
   A vault moved off the home (`itsanas vault --move`) needs one line added to
   `~/.itsanas-fleet/agent.conf`: `VAULT_<account>="<host path of that vault>"`.
3. **How the runner gets in.** Either
   - **Tailscale** (preferred: no port opened at home, and the laptop later):
     the Pi and the VM on the tailnet with tag `tag:fleet`, an ACL letting
     `tag:ci` reach port 22 of `tag:fleet` only, and an OAuth client for
     `tag:ci`; or
   - **one forwarded port** on the Freebox to the VM's sshd (not 22, keys
     only), and the VM as `ProxyJump` for the Pi.
4. **The `fleet` environment** (Settings → Environments → New, deployment
   branches: `main` only), with:

   | Name | Kind | Value |
   | --- | --- | --- |
   | `FLEET_SSH_KEY` | secret | the private half, `fleet-ci` |
   | `FLEET_SSH_CONFIG` | secret | `Host pi` / `HostName ...` / `User ...` (and `ProxyJump vm` without Tailscale), the same for `vm` |
   | `FLEET_KNOWN_HOSTS` | secret | the lines `setup.sh` printed, `localhost` replaced by each `HostName` |
   | `FLEET_TEST_PHRASE` | secret | the 24 words of the `test` account, on one line |
   | `FLEET_HOME_KEY` | secret | a long random string (`openssl rand -hex 32`): seals the member's home in the cache |
   | `TS_OAUTH_CLIENT_ID`, `TS_OAUTH_SECRET` | secret | only with Tailscale |
   | `FLEET_HOSTS` | variable | `pi vm` |
   | `FLEET_SCAN` | variable | `pi:nicolas vm:nicolas` -- another account's home on each host |
   | `FLEET_COORD`, `FLEET_COORD_ID` | variable | `itsanas.ngas.fr:9898` and its device id (scripts/fleet-docker.sh) |

5. **Run it once by hand** (Actions → Fleet → Run workflow) and read the table
   in the run's summary.

## What it does not do yet

- ⬜ **The laptop.** Every defect of 2026-10-22 was born on it, and it is not
  driven: it is away or asleep most of the day, it would need OpenSSH Server on
  Nicolas's working machine (Tailscale only, never a port open to the
  internet). The kit's Windows half now has B, E, F and C's plant, with the
  shell kit's names, digests and codes (`check-fleet.sh` compares the two),
  so an agent in PowerShell has phases to call, and `scripts/fleet/agent.ps1`
  is that agent: the same allowlist, the same 18 refusals in `check-fleet.sh`
  (run with pwsh on Linux), no C scan. **Not tried behind Win32-OpenSSH**, and
  not wired: the setup on the laptop (OpenSSH Server, Tailscale, the key in
  `administrators_authorized_keys`, `agent.json`) is Nicolas's decision.
- ⬜ **E and G**, which need a machine of the account offline: stopping a
  container is docker, which is root. Not in v1, on purpose.
- ⬜ **Logs**: the self-dial of 2026-10-22 was in a log, and the agent reads
  none (container logs are docker's).
- **C reads only what the host holds for `test`**, never the rest of its vault
  (on the Pi, a terabyte to read every night for nothing). A host that holds
  nothing for `test` is a SKIP row: a scan of nothing proves nothing.

## What stays with people

What CI cannot do: install from the website (A), cut the power (J), take a
machine somewhere that is not home with a person using it (O by hand, beyond
the runner), a day on battery (H), a real person's file names (N), and whether
the tray and the Settings page read well. Each is one command with a PASS or
FAIL line, or one screenshot; a partner should never have to read a log.
