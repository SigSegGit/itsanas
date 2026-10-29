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
| **L** | what each machine's `test` node counts -- `copies H of W` on its snapshot's first line, which its icon draws (MVP.md L) -- said in the row: safe, NOT safe past the grace (it reads `exposed`, by `Copies::exposed`'s rule to the second), or NOT safe within the two-round grace. It says what the copies rule says, not the icon's colour: a paused node shows paused whatever its copies. A PASS either way: the row tests that the machine tells, not that the fleet is full. FAIL when a daemon that names its build stores files and counts no copies; SKIP for a daemon from 0.1.12 or before, a node storing nothing, or an agent copy older than this row (run `setup.sh` again) | -- |
| **B** | a file written on the first machine is byte for byte on every other | `B write`, `B check` |
| **F** | deleted there, it is gone everywhere, and still gone a round later | `F delete`, `F check` |
| **C** | a canary written on the first machine, once it has crossed, is nowhere in what another account's node holds for `test` (its vault's directory for that owner) nor in that node's databases; SKIP, not PASS, when it holds nothing for `test`, with what that account's own daemon says of its pledge and how many peers it hosts (`status.snapshot`), or that it has none | `C plant`, `C scan` |

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
with the reason. A SKIP is counted from night to night (in `.fleet-member`,
the runner's cache): the seventh night in a row the same row is SKIP, it is
FAIL, saying since when and why -- C was SKIP six nights over a setup defect
nobody had named. A row listed in `scripts/fleet/planned-skips` waits on a
numbered item of "Waiting on Nicolas" (HANDOVER §0) and stays SKIP however
long, its line ending "(planned: Waiting on Nicolas item N)";
`check-fleet.sh` fails when that item is gone from the list. A red run opens an issue titled `fleet: red`, or comments on
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
   **Again after every agent change** (the agent is a copy): `git pull && bash
   scripts/fleet/setup.sh`, with no key -- it keeps the one it installed.
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
   | `FLEET_KNOWN_HOSTS` | secret | one line per machine: `[HostName]:Port` (or `HostName` on port 22), then the two words `setup.sh` printed (see below) |
   | `FLEET_TEST_PHRASE` | secret | the 24 words of the `test` account, on one line |
   | `FLEET_HOME_KEY` | secret | a long random string (`openssl rand -hex 32`): seals the member's home in the cache |
   | `TS_OAUTH_CLIENT_ID`, `TS_OAUTH_SECRET` | secret | only with Tailscale |
   | `FLEET_HOSTS` | variable | `pi vm` |
   | `FLEET_SCAN` | variable | `pi:nicolas vm:nicolas` -- another account's home on each host |
   | `FLEET_COORD`, `FLEET_COORD_ID` | variable | `itsanas.ngas.fr:9898` and its device id (scripts/fleet-docker.sh) |

5. **Run it once by hand** (Actions → Fleet → Run workflow) and read the table
   in the run's summary.

**FLEET_KNOWN_HOSTS from Windows PowerShell**, the way that worked on
2026-10-23: read each machine's key through ssh and pipe the lines to `gh`.
Two ways that did not: a file written with `>` (Windows PowerShell 5.1 writes
UTF-16, which ssh on the runner cannot read, so every machine is "No
ED25519 host key is known"), and `ssh-keyscan` (it printed nothing there).

    $pi = (ssh -p 22010 USER@HOST cat /etc/ssh/ssh_host_ed25519_key.pub) -split ' '
    $vm = (ssh -p 22011 USER@HOST cat /etc/ssh/ssh_host_ed25519_key.pub) -split ' '
    $k = "[HOST]:22010 $($pi[0]) $($pi[1])", "[HOST]:22011 $($vm[0]) $($vm[1])"
    $k   # two lines, each "[HOST]:port ssh-ed25519 AAAA..."
    ($k -join "`n") | gh secret set FLEET_KNOWN_HOSTS --env fleet -R OWNER/REPO

A machine whose key is missing or wrong is a SKIP row naming the host key,
not a red run until its seventh night in a row (unless it is planned, as the
Pi's is while its key waits) -- read the SKIP lines of the first runs.

### The laptop, once (when Nicolas decides to)

1. **The `test` account on it**, beside the real ones:
   `$env:ITSANAS_PASSPHRASE = '...'` then
   `powershell -ExecutionPolicy Bypass -File install\provision.ps1 -Instance test -PhraseFile <the 24 words> -Folder $env:USERPROFILE\ITSaNAS-test`
   (provision.ps1's other options as for any machine of the account).
2. **OpenSSH Server and Tailscale**, from an elevated PowerShell:
   `Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0`,
   `Set-Service sshd -StartupType Automatic`, `Start-Service sshd`; then
   `PasswordAuthentication no` in `C:\ProgramData\ssh\sshd_config` and
   `Set-NetFirewallRule -Name OpenSSH-Server-In-TCP -RemoteAddress 100.64.0.0/10,fd7a:115c:a1e0::/48`,
   so port 22 answers the tailnet only (its IPv6 range too, or a name that
   resolves to IPv6 first waits for a timeout); the laptop on the tailnet with tag
   `tag:fleet`.
3. **The agent**, in the checkout, from an elevated PowerShell (an
   administrator's keys are `C:\ProgramData\ssh\administrators_authorized_keys`):
   `git pull; powershell -ExecutionPolicy Bypass -File scripts\fleet\setup.ps1 "<the contents of fleet-ci.pub>"`.
   It refuses without a `test` node, prints `hello`, the test node's snapshot,
   a refused `sh`, the host key, and a warning for each of step 2's settings
   it finds missing. It installs nothing and changes no system setting.
   **The one difference from the Pi:** an administrator's SSH logon on
   Windows is not filtered by UAC, so on an administrator account the agent
   runs with the full administrator token, where on the Pi it runs as a user
   without sudo. The allowlist is the same; what a bug in it would reach is
   not. The alternative is a local account without administrator rights
   that owns the test node and its folder (`provision.ps1` run as that
   user), at the cost of a second Windows session on the laptop. Nicolas's
   choice; `setup.ps1` warns when it is the first.
4. **The first run behind Win32-OpenSSH**, from the laptop itself, before
   CI depends on it: `ssh -i fleet-ci <user>@localhost hello` must print a
   `hello` line, and `ssh -i fleet-ci <user>@localhost sh` a `REFUSED` one.
   Nothing else has ever checked that the Windows sshd hands the request to
   the agent; if `hello` comes back as a refusal for an empty request, it
   does not.
5. **In the `fleet` environment**: `laptop` added to `FLEET_HOSTS` (not to
   `FLEET_SCAN`: the Windows agent offers no C scan), a `Host laptop` block in
   `FLEET_SSH_CONFIG` (its tailnet name, the Windows user), and the host key
   line in `FLEET_KNOWN_HOSTS`. A night the laptop sleeps is a SKIP row; seven
   in a row are a red run, on purpose: nothing has checked it for a week.

## What it does not do yet

- 🟨 **The laptop.** Every defect of 2026-10-22 was born on it, and it is not
  driven yet: it is away or asleep most of the day, and it needs OpenSSH
  Server on Nicolas's working machine (Tailscale only, never a port open to
  the internet) -- his decision, not a session's. Everything on this side of
  that decision is built: the kit's Windows half has B, E, F and C's plant,
  with the shell kit's names, digests and codes (`check-fleet.sh` compares the
  two); `scripts/fleet/agent.ps1` is the agent, with the same allowlist, the
  same 18 refusals and the same one-request-at-a-time lock, no C scan; and
  `scripts/fleet/setup.ps1` puts it on the laptop (see "The laptop, once"
  below). `check-fleet.sh` runs that setup with pwsh on a profile laid out as
  the laptop's (a space in its path, an emptied home, another key in the
  keys file) and runs the keys line it wrote the way sshd does. **Not tried
  behind Win32-OpenSSH**, only behind Linux sshd with pwsh: that the Windows
  sshd hands the request over in `SSH_ORIGINAL_COMMAND` and honours
  `restrict` is from its documentation, not from a run.
- ⬜ **E and G**, which need a machine of the account offline: stopping a
  container is docker, which is root. Not in v1, on purpose.
- 🟨 **Logs**: the self-dial of 2026-10-22 was in a log, and the agent reads
  none (container logs are docker's). The runner's own member daemon is read:
  at the end of a run `member.sh stop` prints its lines that repeated, most
  first, with addresses and ids blanked so one kind of line is one row, then
  its last forty lines. The first green run's log was eight copies of two
  lines, which forty lines of tail did not say.
- **C reads only what the host holds for `test`**, never the rest of its vault
  (on the Pi, a terabyte to read every night for nothing). A host that holds
  nothing for `test` is a SKIP row: a scan of nothing proves nothing.

## What stays with people

What CI cannot do: install from the website (A), cut the power (J), take a
machine somewhere that is not home with a person using it (O by hand, beyond
the runner), a day on battery (H), a real person's file names (N), and whether
the tray and the Settings page read well. Each is one command with a PASS or
FAIL line, or one screenshot; a partner should never have to read a log.
