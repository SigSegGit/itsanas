# Test Catalogue

**Last updated: 2026-10-07 — 1061 test functions across 33 binaries, 9 of them
`#[ignore]`d, plus 2 doctests. 245 are red-team tests.**

**945 of the 1061 tests have an entry of their own on this page** — an *entry*,
meaning a row in one of the tables below whose last cell says something, not a
name dropped into a sentence. Forty-seven of
the rest are the `itsanas-coord` section that says outright it catalogues by
property rather than test by test; the remaining sixty-odd are ordinary
omission, concentrated in `itsanas-store`'s unit tests and `itsanas-folder`.
CONTRIBUTING.md says every test gets an entry saying what it proves, so that is
a debt rather than a policy, and it is written here because a page that lists
some tests reads exactly like a page that lists all of them. Nothing had ever
counted: the section headings added up to 627 while the file named 522.

Every number on this page is checked against the source by
`scripts/check-counts.py` on each push, including each row of the table below
and each section heading — the headings naming one crate have to add up to that
crate's real test count, which five of them did not.
That check exists because these numbers were wrong: this header said 637 and
twenty-nine, the README said 637 and 29, `ROADMAP.md` said 589 and sixteen, and
the table below said `itsanas-folder` had 31 unit tests when it had 32 — which
is where the missing one in the totals had gone. The sentence that used to
stand here claimed three of the counts were already checked mechanically, and
named three checks that verify names, messages and wiring, and no count at all.

Those three exist too, and each because the thing it checks had already gone
wrong: `check-catalogue.sh` (every test named here exists), `check-messages.py`
(no message or command has had a line continuation collapsed into it), and
`check-wired.py` (every public method has a call site somewhere in the
workspace).

## What is verified by hand, and why

`scripts/acceptance.ps1`, test H on Windows, is verified by running it, not by
a test: it reads a live daemon's CPU time, memory and I/O, the system power
log and `powercfg`, none of which a CI runner has in a meaningful state.
`check-installers.sh` parses it. It was run against the laptop's daemon before
it was committed: a real sample, `H report` refusing a window shorter than a
day with the numbers, and `H sleep` refusing a verdict without elevation.
That run found two faults: one sample read back as no count at all (Windows
PowerShell returns a single object, not a list, for a one-row file), and the
numbers written with decimal commas on a French system.

One command sends requests to a coordinator with no automated test of the
sending:

- `itsanas peer find <username>` — `Lookup` then `Peers`

`itsanas device list`, `itsanas device forget`, `itsanas login --from` keeping
its coordinator and `itsanas passphrase` were in the same position until
2026-09-15. They are now driven through the real binaries by
`scripts/acceptance-local.sh` (the `acceptance-local` CI job), which enrols a
restored machine, finds it in the list, withdraws it by its short id, checks it
leaves the list and cannot enrol again, and changes a passphrase and opens the
node with the new one and not the old.

The **protocol** behaviour both rely on is covered.
`a_member_registers_enrols_a_device_and_is_then_findable_by_name` in
`itsanas-coord/tests/coordinator.rs` runs the exact Lookup-then-Peers sequence
from a machine that knows only the name, and
`a_revoked_device_leaves_the_live_set` covers the directory honouring a
revocation. What is untested is the glue: turning a `Response::Missing` into a
sentence, refusing your own account, and not adding an address twice.

The dangerous halves *are* tested. `resolve_device` refuses a prefix that names
no device, because a revocation filed against an identifier nobody holds would
be silent, permanent and impossible to notice.

Testing the rest would mean a coordinator harness inside the CLI crate for
about ten lines of glue. That is written here rather than papered over with a
test that cannot fail: both commands were run against the real coordinator on
the Raspberry Pi, and both were checked by taking the thing away first — the
configured address for `mandarine` was removed and recovered from the username
alone, and the dead device was counted in the daemon log eleven times before and
zero times after.

## Every test has one minute
## Every test has one minute

Tests are run with **`cargo nextest`**, not `cargo test`, and the reason is a
timeout. `cargo test` has none: a test that blocks — on a socket that never
answers, a lock nobody releases, a loop whose exit condition is wrong — blocks
the run, and the only thing that ends it is a person noticing. One job in this
project ran for five hours before it was stopped by hand.

nextest gives each test its own process, so it can time it and kill it. The
budget is in [`.config/nextest.toml`](../.config/nextest.toml): warnings at
twenty and forty seconds, **termination at sixty**, which turns a hang into a
failed test with a name.

```sh
cargo nextest run --workspace --all-features    # the suite, with the budget
cargo test --doc --workspace --all-features     # the doctests nextest does not run
cargo nextest run --profile measure ...         # report everything over a second
```

Measured on 2026-09-01, Windows x86-64:

| | slowest single test | total |
| --- | --- | --- |
| the suite, debug | 11.1 s (`a_long_run_of_alternating_partitions_still_converges`) | 26.6 s |
| the three `#[ignore]`d, release | 16.4 s (`a_store_killed_mid_write_never_lists_a_file_it_cannot_read`) | 16.4 s |

So the limit sits at about five times the slowest thing in the suite. That is
deliberate. A timeout that fires on a loaded runner teaches people to press
re-run rather than to look, and a limit nobody trusts protects nothing.

**There are no exceptions, and there was nearly one.** The crash test takes
**66 seconds in a debug build** — it kills a store mid-write a dozen times, and
each of those dozen processes pays a full 64 MiB Argon2id derivation, which is
slow on purpose. Writing it a timeout override was the obvious move and the
wrong one: the same test takes 5.9 seconds in release, so the cost was an
artefact of the build profile rather than of anything it asserts.

That was half right, and CI said so. In release the test's *own* guard fired:
it times one complete write and refuses to continue if that write is too fast
to interrupt, because otherwise every kill lands after the write has finished
and the test proves nothing. On a release build on a CI runner a whole write
finished in under 200 ms. The payload size was a constant picked against a
debug build, so the test now **calibrates it**: it doubles the payload until a
whole write crosses 400 ms, and asserts that it got there. That makes the test
independent of the optimisation level and of how fast the machine is, which is
what it should have been. It costs 16.4 s in release.

The `slow-tests` job therefore runs the `#[ignore]`d tests **in release**, and the
exception disappears instead of being documented. Before adding an override,
the question is whether the test is slow for a reason connected to what it
checks.

**One profile is exempt, and it is not about this code.** The ARM job
cross-builds for aarch64 and runs the suite under `qemu-user-static` on an x86
runner. At sixty seconds that job reported **eight tests timed out and 724
seconds for the suite** — while the same suite on a real Raspberry Pi 4, slower
silicon on an SSD, has nothing anywhere near a minute. The limit there was
measuring an instruction-set emulator, so the `ci-emulated` profile allows three
minutes -- and three minutes is also the ceiling no profile may pass, exempt or
not: `HARD_CEILING` in the gate below. It allowed five until 2026-09-24; by then
the whole emulated suite took 123 seconds, so no single test was near either
limit. The exemption and its reason live in `scripts/check-test-budget.py`
itself, which prints them on every run, so it cannot become the norm by sitting
somewhere nobody reads.

`scripts/check-test-budget.py` keeps this true. It checks that the profiles
still *terminate* rather than merely warn, that retries stay off, that any
override carries a comment justifying itself, and that every test invocation in
`.github/workflows` still goes through nextest — with `cargo test --doc` named
as the one exception, because nextest does not run doctests and moving to it
without that line would have stopped running two tests while the summary still
said everything passed. A timeout configured in a file nothing uses reads like a
guarantee and is not one.

| Binary | Tests |
| --- | --- |
| `itsanas-crypto` unit | 66 (1 `#[ignore]`d) |
| `itsanas-crypto` property (`tests/properties.rs`) | 15 |
| `itsanas-wire` unit | 17 |
| `itsanas-tls` unit | 19 |
| `itsanas-tls` handshake (`tests/handshake.rs`) | 5 |
| `itsanas-store` unit | 167 |
| `itsanas-store` integration (`tests/store.rs`) | 47 (1 `#[ignore]`d) |
| `itsanas-sync` unit | 12 |
| `itsanas-sync` convergence (`tests/convergence.rs`) | 24 |
| `itsanas-net` unit | 45 |
| `itsanas-net` two-node (`tests/two_nodes.rs`) | 58 |
| `itsanas-placement` unit | 34 |
| `itsanas-coord` unit | 119 (1 `#[ignore]`d) |
| `itsanas-coord` integration (`tests/coordinator.rs`) | 16 |
| `itsanas-discover` unit | 43 |
| `itsanas-policy` unit | 23 |
| `itsanas-folder` unit | 32 |
| `itsanas-folder` integration (`tests/folder.rs`) | 23 |
| `itsanas-folder` storage-vanished (`tests/storage_vanished.rs`) | 6 |
| `itsanas-folder` reports (`tests/reports.rs`) | 6 |
| `itsanas-cli` unit | 104 |
| `itsanas-android` unit | 10 |
| `itsanas-drive` unit | 9 |
| `itsanas-node` unit | 117 |
| `itsanas-node` away-from-home (`tests/away_from_home.rs`) | 5 |
| `itsanas-node` says-what-is-wrong (`tests/says_what_is_wrong.rs`) | 4 |
| `itsanas-node` five devices (`tests/five_devices.rs`) | 4 |
| `itsanas-node` withdrawals (`tests/withdrawals.rs`) | 4 |
| `itsanas-cli` crash (`tests/crash.rs`) | 1 (1 `#[ignore]`d) |
| `itsanas-cli` steering (`tests/steering.rs`) | 4 (4 `#[ignore]`d) |
| `itsanas-cli` setup (`tests/setup.rs`) | 1 (1 `#[ignore]`d) |
| `itsanas-release` release (`tests/release.rs`) | 14 |
| `itsanas-testkit` unit | 7 |

These counts are mechanical — regenerate them with
`cargo test --workspace -- --list`. If this table disagrees with that command,
the table is the bug.

Every automated test in ITSaNAS is listed here with the property it establishes.
The rule this project holds itself to: **if you cannot state in one sentence
what a test would catch, it should not exist.** A test that passes whether or
not the system works is worse than no test, because it buys false confidence.

Where a security claim is made anywhere in the documentation, there is a test
here that would fail if the claim were false.

## Red-team tests

A test whose name begins `red_team_` describes an **attack**. It passes when the
attack fails. Each one names the attacker, what it costs them, and what they get
if the test ever goes green for the wrong reason — because a security test whose
failure message is `assertion failed: !x` teaches nobody anything at three in the
morning.

They exist because an ordinary test found nothing here. The eviction protection
in `itsanas-discover` had a test that passed while the daemon above it was
handing protection to every stranger that dialled: the test **confirmed the
honest peer and nobody else**, encoding the assumption instead of checking it.
The attack was found by reading the code, not by running the suite. These tests
are the answer to that.

| Test | The attack it defeats |
| --- | --- |
| **`red_team_a_flood_of_authenticating_strangers_cannot_take_over_the_table`** | A device id is a free keypair. Mint 600, have every one claim the victim's owner tag so they sort to the front of the dial order, answer every dial correctly and store nothing. If merely authenticating earned protection, they would all become unevictable, fill the table, and the real Raspberry Pi would be refused entry forever while every node reported discovery as working. One laptop on the same wifi could silently stop a household syncing. |
| **`red_team_dialling_strangers_is_rationed_so_a_flood_cannot_eat_the_interval`** | Three hundred minted identities announce themselves. Without a cap the daemon opens three hundred connections per round and spends the whole sync interval shaking hands with machines that store nothing. |
| **`red_team_a_peer_that_only_answered_the_phone_has_earned_nothing`** | The rule underneath both of the above: completing a mutually authenticated handshake proves possession of a keypair generated a second earlier. It identifies a peer; it vouches for nothing. |
| **`red_team_a_failed_round_earns_nothing`** | Offering data a peer never took is not the peer storing it. |
| **`red_team_a_host_that_refuses_everything_is_not_reported_as_nothing_to_send`** | A host takes the connection, answers every question and refuses every byte — pledge 0, a full disk, or a leech. The owner's round printed `sent 0 B, 0 segments`, the line an idle round prints, so the owner believed nothing was pending. The report now counts refusals and keeps the first reason. |
| **`red_team_a_host_that_keeps_discarding_stops_getting_free_uploads`** | The follow-up attack: keep doing it, and let the owner's own repair drain their uplink forever. |
| **`red_team_a_host_that_keeps_discarding_stops_costing_bandwidth`** | The rule underneath it. |
| **`red_team_a_host_that_threw_the_data_away_stops_counting_as_a_holder`** | Accept everything, delete it, keep claiming the space. Free, undetectable without audits, and fatal to the replication guarantee. |
| **`red_team_the_user_id_never_appears_on_the_wire`** | Sit on a café or hotel network and listen. A user id is a public key; broadcasting it every thirty seconds would tell the room whose machine this is. Neither it, nor the household key, nor the version 1 tag of it is in a version 2 packet. |
| **`red_team_two_beacons_of_one_account_carry_unlinkable_tags`** | Listen and group. Version 1 sent one tag per account for ever, so anyone could tell which machines belong together. Two beacons of one machine, and of two machines of one account, now carry unrelated tag fields (neither half repeats) and the household still recognises each. Sabotaged with a fixed nonce: red. |
| **`red_team_a_stranger_holding_the_user_id_cannot_recognise_the_tag`** | A user id is public. Version 1's tag was a hash of it, so knowing who you are was enough to pick out your machines. Recognising a version 2 tag needs the account's master secret: another account's key does not, an unkeyed hash does not match, a hash keyed on the user id does not match. Sabotaged with an unkeyed hash and with the user id as the key: red. |
| **`red_team_a_tag_lifted_onto_another_device_is_not_recognised`** | Copy a household member's tag into a beacon signed by your own minted device and sort to the front of their dial order. The device is inside the keyed hash, so the copy fails. Sabotaged by dropping the device from the hash: red. A whole beacon replayed from another address still reads as "mine"; that costs one dial, which TLS device pinning refuses. |
| **`red_team_a_version_1_beacon_is_still_heard_and_never_counted_as_mine`** | Two failures at once. Refuse version 1 and an upgrade makes the household blind to its not-yet-upgraded machines on the LAN; count its unkeyed tag as ours and the old format is a downgrade path back to the forgeable tag. Sabotaged with a version check accepting only 2, and with a legacy tag read as ours: red. |
| **`red_team_an_upgraded_listener_still_learns_a_not_yet_upgraded_sender`** | The mixed fleet over a real socket: a version 1 Pi's beacon reaches an upgraded laptop, which records its device and port and dials it among the strangers. Sabotaged with a version check accepting only 2: red. |
| **`red_team_a_copied_tag_does_not_sort_a_stranger_among_my_machines`** | The copied-tag attack at the layer the daemon dials from: the table checks each tag against the device it stored. Sabotaged by checking against any stored device: red, while every beacon-level test stayed green. Found by `itsanas-redteam`. |
| **`red_team_a_replayed_version_1_beacon_does_not_demote_an_upgraded_machine`** | Nothing in a beacon is fresh, so an old version 1 beacon of an upgraded laptop, replayed, replaced its version 2 tag and dropped it among the strangers, its silence no longer reported. A device heard on version 2 keeps its tag. Sabotaged by removing that rule: red. Found by `itsanas-redteam`. |
| **`red_team_grinding_one_account_is_cut_off_after_a_few_attempts`** | The escrow blob is fetchable by anyone with a username, because a machine recovering from nothing has nothing to prove with. Grinding it is the attack, and the rate limit is the only defence — the single job a central component does better than a distributed one. |
| **`red_team_flooding_invented_names_cannot_reset_a_real_account_counter`** | The limiter is a table a stranger writes into. Evicting to make room would let an attacker clear their own counter. |
| **`red_team_reconnecting_does_not_reset_the_escrow_attempt_budget`** | A per-connection budget is no budget: reconnecting costs a handshake and buys a fresh one. |
| **`red_team_an_unenrolled_device_cannot_overwrite_someone_elses_escrow`** | Substituting a container whose passphrase you chose. |
| **`red_team_a_machine_holding_the_master_key_cannot_bring_a_withdrawn_device_back`** | Steal a laptop whose daemon reads its passphrase from a file. The owner withdraws it; the thief runs `itsanas register`, which signs a newer claim with the master secret every keystore holds. By timestamp that claim won and the device was enrolled again. |
| **`red_team_a_withdrawal_signed_on_a_slow_clock_still_withdraws`** | The same ordering the other way: an enrolment dated by a laptop forty minutes fast outranked a withdrawal issued later from a Pi with the right time, and `device forget` printed "withdrew" over a device still enrolled. |
| **`red_team_a_stranger_cannot_list_another_member_s_devices`** | A user id is public. The device list tells how much each machine pledges and how long each has been silent — which household's NAS has been off for a month — so only a live device of that account may ask. |
| **`red_team_coordinator_messages_keep_their_wire_numbers`** | Insert a message mid-enum and every deployed client or coordinator reads the ones after it as other messages. The peer protocol lost a week to exactly that; this protocol had no pin until it grew its first new message. |
| **`red_team_a_device_cannot_publish_an_address_for_a_device_it_does_not_own`** | Black-holing a member's machines through the address book. |
| **`red_team_a_name_cannot_be_taken_over_by_a_different_key`** | Sending everyone who looks a member up to an impostor. |
| **`red_team_an_oversized_username_is_refused_before_the_directory_sees_it`** | A megabyte where a name is expected. |

## How to run

```bash
cargo test --workspace                    # the fast suite (~9s)
cargo test --workspace -- --ignored       # expensive tests, real cost parameters
cargo clippy --workspace --all-targets -- -D warnings
cargo deny --all-features check           # advisories and licences
```

## What CI runs, and why each job exists

Defined in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml).

| Job | What it runs | Why it is there |
| --- | --- | --- |
| **changes** | `scripts/ci_scope.py plan`: the files a pull request changed, mapped to crates, plus every crate depending on them | Since 2026-10-06 a pull request tests what it touches (ROADMAP "Selective CI"). Every job below except lint and supply-chain takes its crates, or its skip, from this one. Push to `main`, the nightly run, a `milestone` label and a change to Cargo.lock, a manifest, the toolchain, `.config/` or a workflow run everything. |
| **lint** | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo doc` with `RUSTDOCFLAGS=-D warnings`, and the five checking scripts below | Style drift and lint debt compound, and clippy catches real bugs in crypto code — sign confusion, lossy casts, misused ranges. Broken intra-doc links are the quietest kind of rot: the documentation keeps claiming a relationship the code no longer has and nothing fails until a reader clicks it. |
| **test** | `cargo nextest run` and the doctests, on the affected crates (the workspace on a full run), on Ubuntu, Windows, macOS; gated per step, never skipped as a job, because its legs are required checks | ITSaNAS must run on a Windows laptop and a Linux Pi at once. Path handling, endianness assumptions and filesystem semantics differ; a Linux-only suite would not notice. macOS also catches things the other two share — a watcher test that assumed a folder is quiet the instant it is created passes on both and is a race there. |
| **slow-tests** | `cargo test -- --ignored --test-threads 1` | Three tests are marked `#[ignore]`: the real 64 MiB Argon2id cost, a 64 MiB streaming round trip that takes ~45s in a debug build, and the crash-consistency test that spawns a dozen processes each paying a full derivation. Too slow for every push, far too important to never run. |
| **cross-build** | `cargo build --release` for `aarch64-unknown-linux-gnu`, then `cargo test --workspace` for that target under `qemu-user-static`, then `scripts/smoke.sh` | The Raspberry Pi 4B+ and the Freebox VM are first-class targets, and a cross-*build* only says the types line up. aarch64 is where blake3 switches to its NEON backend, which links cleanly and hashes wrong. Emulation runs the instruction set on the host's kernel, so it does not reproduce aarch64's weaker memory ordering and asks an emulated CPU which features it has. The smoke script is the same one an installer runs at the end of a real install, so what CI checks is what a person sees on their own machine. |
| **android-core** | `cargo check --target aarch64-linux-android` for the data-path crates | The app is built from these crates, so this job is what stops a change breaking the phone build a week before anybody opens Gradle. `ring` is excluded because it assembles its own primitives, which is a build-tool question rather than a code one. The job needed an NDK it did not have for two weeks, because its own comment claimed the crates were pure Rust and blake3 compiles C. |
| **minimum-rust-version** | `cargo check` on the pinned MSRV | Prevents accidentally requiring a newer toolchain than the documented minimum, which would break users on distro Rust. |
| **supply-chain** | `cargo deny check` | Fails on any unpatched advisory, any yanked crate, and any licence not compatible with AGPL-3.0. For a system whose entire value is "your host cannot read your data", a vulnerable crypto dependency is a release blocker. |
| **coverage** | `cargo llvm-cov`, on full runs only | Not a target to game — used to spot whole modules or error paths with no test at all. |
| **acceptance-local** | `scripts/acceptance-local.sh`: B, D, E, F and G of `docs/MVP.md` (C, H, I and J by their negative controls only) between three nodes of one account and a throwaway coordinator, then every phase of `scripts/acceptance.sh` pointed at a situation it must refuse | The MVP verdict is taken with that kit on four real machines, in a morning that is expensive to waste, so a check that cannot pass — or cannot fail — has to show up on a push instead. Its first runs found two checks that printed FAIL and exited 0, and F "passing" on a file that had never arrived. It is still one machine with no power cycle: laboratory evidence, not a fleet result. |
| **ai-code-review** (`ai-code-reviewer.yml`) | `scripts/test_ci_code_reviewer.py` against a fake endpoint, then `scripts/ci_code_reviewer.py` on `git diff <base>...HEAD` through the models in `AI_MODEL_NAME` | A second reading of every PR's diff for missing tests, unbounded memory and P2P holes, posted as a comment — advice, not a verdict: the job is red when the reviewer is broken or no model answers, never for what it says. Its own tests exist because every failure mode it has was found on its first evening: Gemini's free tier kept three models at 503 for minutes, one model name had been retired, another timed out. Each test was checked by breaking the code it guards. See `AGENTS.md`. |
| **no-warnings** | `ci_scope.py verify` -- every job passed, or was skipped by the plan with a reason written down -- then reads the annotations of every check run in the run and fails on any at warning or failure level | On 2026-09-14 every run carried fourteen Node.js 20 deprecation warnings and passed, so nobody read them. A warning allowed to stay teaches people to skip warnings, and the next one is the one that matters. One allow-listed line: the retry `install-action` prints when a Windows runner's bash fails to start. |

The five checking scripts in **lint**, each of which exists because the thing it
checks had already gone wrong:

| Script | What it refuses |
| --- | --- |
| `check-catalogue.sh` | A test named in this file or in HANDOVER.md that does not exist in `crates/`. Three deleted `transport` tests survived here as evidence, one of them in bold as the security property that mattered. |
| `check-ci-scope.py` | A scope decision that tests less than it must: a change to `itsanas-crypto` that does not reach every dependent (checked against a fixpoint over the real workspace), a docs- or scripts-only PR that runs a Rust test job, a Cargo.lock, manifest, toolchain, `.config/` or workflow change that is not a full run, an unclassified path that is not a full run, a verdict that accepts an unplanned skip. And `ci.yml` disobeying the plan: a gated job without its gate, the test matrix skipped by job (its required checks would never report), a job testing `--workspace` instead of the plan's crates, android-core checking other crates than the plan assumes. Sabotaged eight ways through `scripts/sabotage.py`, red each time; the Cargo.lock sabotage first went green through the unknown-path fallback, and the check now demands the named reason (HANDOVER §0, 2026-10-06). |
| `check-counts.py` | Any number about tests in README.md, ROADMAP.md or this file that the source does not support. All three disagreed with the tree and with each other. |
| `check-messages.py` | A line continuation collapsed into a message or a command. `cargo fmt` eats them in string literals and the scripts that write large edits eat them everywhere else; the Android job ran with thirteen literal spaces in it from the day it was written. |
| `check-wired.py` | A `pub fn` with no call site. Four mechanisms were designed, implemented, tested, documented and wired to nothing. |
| `check-installers.sh` | An installer that does not parse, uses bash syntax while claiming POSIX, is missing from `install/README.md`, demands a Rust version Cargo.toml does not, writes a systemd `ReadWritePaths` without the leading dash that lets a unit start before the path exists, cannot be re-run over the coordinator binary it installed (the copy step is cut out of `coordinator.sh` and run against a file that is already its destination, and against an older one it must still replace), or cannot undo itself — every entry point is *run* with `--clean` and has to reach the uninstaller's dry run. A grep for the flag would have passed on all three ways that delegation breaks: a missing sibling, a case arm that shifts and falls through, and a script that never had the arm at all. Two instances are provisioned with `provision.sh --instance` in a throwaway HOME (a stub `itsanas`, a `systemctl` that only logs), `b` is cleaned with `--purge-account`, and `a`'s home, passphrase file, enablement and the shared unit template must survive while nothing stops or disables `itsanas@a` (HANDOVER §8 0i; sabotaged by globbing `*.environment` and by disabling `itsanas@*`, red both times). The tray's logon shortcut (§8 0f): the shortcut's name and arguments are read out of `provision.ps1` as written and evaluated for two instances and the default node -- each must run the installed script through `conhost --headless` with its own `-Instance`, under a name of its own -- then `clean.ps1 -Instance zz-check-b -Yes` runs against a throwaway Startup folder holding all three and must remove b's alone (sabotaged by dropping `-Instance`, by globbing `ITSaNAS tray*.lnk` in the clean, and by one name for every node; red all three times). The PATH line (§8 3e): the `path-line` block must be identical in `linux.sh` and `macos.sh`, and is run in a throwaway HOME under zsh -- two installs leave one marked line in `~/.zprofile` without gluing it onto a last line that has no newline, `clean.sh --yes` removes that line and leaves the person's untouched, and a Debian `~/.profile` that already puts `$HOME/.local/bin` on the PATH gets no second entry (sabotaged by dropping the duplicate check, by a clean that keeps the line, by dropping the Debian check, and by editing one copy of the block; red all four times). Red-team pass on the release batch: `tray-autostart.sh` refuses an `ITSANAS_BIN` holding a newline or `%` and writes no `.desktop` entry (sabotaged by dropping the pattern: red); `get.ps1` puts the old `itsanas.exe` back when the new one cannot be renamed in (a mocked `Rename-Item` throws; sabotaged by dropping the restore: red), and on Windows adds to the user PATH in a throwaway HKCU key keeping `%USERPROFILE%` and REG_EXPAND_SZ (sabotaged by writing it expanded as REG_SZ: red); `sign-release.sh`, against a fake `gh` and the checkout's `itsanas-release`, reaches the signing question for an untouched draft and stops before it, with nothing uploaded or published, for a draft with a binary byte changed or a manifest version that is not the tag's (sabotaged by skipping `check`: red). |

The workflow also runs **daily on a schedule, in full**, so a newly published
advisory against a dependency -- or a new rustc, or a new runner image --
surfaces even when nobody has pushed for a month.

---

# `itsanas-crypto` — unit tests (66)

## `secret` — secret hygiene (4)

| Test | What it proves |
| --- | --- |
| `debug_never_reveals_bytes` | Formatting a secret prints `SecretBytes<32>(redacted)` and no key bytes. Daemon logs on an ITSaNAS node are readable by the machine's owner, who is explicitly not the person whose keys those are. |
| `equality_is_value_based` | Secret comparison is by value via constant-time `ct_eq`, so tests comparing keys are meaningful and comparison does not leak timing. |
| `random_secrets_differ` | Two freshly drawn secrets differ and neither is all-zero. Catches a miswired CSPRNG returning a constant — the failure mode that silently makes every key identical. |
| `from_slice_rejects_wrong_length` | 31- and 33-byte inputs are refused. Prevents a truncated key being silently zero-padded into a weak key. |

## `kdf` — key derivation (7)

| Test | What it proves |
| --- | --- |
| `context_strings_are_unique` | No two derivation contexts collide. A collision would mean two different purposes share a key — for example the signing key equalling the data key. |
| `context_strings_carry_a_version` | Every context contains a version marker, so a future v2 key schedule derives entirely different keys instead of silently reinterpreting v1 material. |
| `different_contexts_yield_different_keys` | All pairs of contexts produce distinct keys from identical input. The empirical counterpart to the uniqueness check above. |
| `derivation_is_deterministic` | Same input, same key, every time. This is what makes recovery from a phrase work at all. |
| `one_bit_of_master_change_changes_every_subkey` | Flipping one bit of the master secret changes every derived subkey. Catches a derivation that accidentally ignores part of its input. |
| `expansion_separates_by_label` / `expansion_separates_by_root_key` | Per-object key expansion is separated by both label and root key, so two objects never share a (key, nonce) pair. |

## `ids` — identifier encoding (4)

| Test | What it proves |
| --- | --- |
| `hex_round_trips` | An identifier survives rendering and re-parsing exactly. These strings appear in CLI output, config and the coordinator API. |
| `short_form_is_twelve_chars` | The abbreviated form used in logs is stable, so log output stays greppable across versions. |
| `parsing_rejects_bad_input` | Empty, non-hex, too-short and too-long inputs are all refused rather than being silently padded or truncated into a valid-looking identifier. |
| `hex_rendering_is_lowercase_and_zero_padded` | Encoding is canonical. Without this, `0x05` could render as `5` and produce a 63-character identifier that fails to round trip. |

## `identity` — identity and signatures (18)

| Test | What it proves |
| --- | --- |
| `recovery_phrase_round_trips` | A generated master secret produces a 24-word phrase that reconstructs it exactly. The single most important recovery path in the system. |
| `recovery_phrase_tolerates_untidy_input` | A phrase retyped in capitals, with line breaks and stray whitespace, still works. **This test found a real bug**: raw BIP-39 parsing rejected it, which would have locked out a user typing 24 words off paper correctly. |
| `a_corrupted_recovery_phrase_is_rejected_not_silently_accepted` | Transposed words, non-words and an empty string are refused rather than producing a valid-but-wrong identity. |
| `short_phrases_are_rejected` | A valid *12-word* BIP-39 phrase is refused. 12 words carries 128 bits; ITSaNAS requires 256. Without this check a user could unknowingly halve their key strength. |
| `identity_is_a_pure_function_of_the_master_secret` | The same master secret always yields the same user id and public keys, on any machine. If this failed, recovery on a new device would produce a stranger. |
| `different_masters_give_different_identities` | Distinct masters give distinct identities — no accidental collapse to a shared identity. |
| `signatures_verify_and_reject_tampering` | Valid signatures verify; altered messages and flipped signature bits do not. |
| `a_signature_cannot_be_replayed_under_another_domain` | A signature over an oplog head is **not** accepted as a device certificate. Without domain separation, any signature could be replayed in any other context. |
| `domain_prefix_is_unambiguous` | `("ab", "c")` and `("a", "bc")` hash differently. Catches the classic concatenation ambiguity that makes domain separation decorative. |
| `one_users_signature_does_not_verify_under_another` | Alice's signature fails under Bob's key. Basic, and the thing that stops a host forging log entries. |
| `chunk_ids_deduplicate_within_a_user_but_not_across_users` | One user gets a stable address for identical content (deduplication works) while two users get unrelated addresses for the *same* content (a host cannot tell they hold the same file). |
| **`red_team_two_accounts_sealing_one_file_share_no_sealed_bytes`** | Two hosts of two accounts compare what they hold: the same 4 KiB file sealed by two accounts shares no 16-byte run of sealed bytes and no address, while one account's seal is byte-stable (the vacuity guard). Equal sealed bytes would let two hosts prove two people hold one file, and a host with a candidate copy confirm it. Sabotaged by a convergent seal (unblinded address, one root and one owner for all accounts): red. The chunk-size sequence still links them; that is ROADMAP's fingerprint, not this test. |
| `chunk_id_does_not_expose_the_plaintext_hash` | The chunk id is not the raw content hash. Otherwise a host holding a guess of the plaintext could confirm it by hashing — a confirmation-of-file attack. |
| `diffie_hellman_agrees_in_both_directions` | X25519 agreement is symmetric, the prerequisite for wrapping keys to another user. |
| `device_keys_are_independent_of_the_user_master` | Device keys are independently random and restorable from their seed, so revoking a stolen laptop never requires rotating the user's identity. |
| `secrets_are_redacted_in_debug_output` | `MasterSecret` and `UserKeys` never print key material even when logged wholesale. |

## `seal` — authenticated encryption (16)

The security core. Most of these assert that an **attack fails**.

| Test | What it proves |
| --- | --- |
| `deterministic_seal_round_trips` | Content-addressed sealing recovers the plaintext exactly, with the documented overhead. |
| `deterministic_seal_is_byte_stable` | Sealing identical content twice gives identical bytes. Deduplication depends on it, and so does remote audit — an owner re-derives what a host should hold rather than storing a second copy. |
| `random_seal_round_trips_and_is_never_byte_stable` | Randomised sealing round trips and never repeats a nonce. Nonce reuse under XChaCha20-Poly1305 is catastrophic. |
| **`another_user_cannot_open_your_chunk`** | Bob, holding Alice's sealed chunk, cannot decrypt it. **This is the load-bearing test of the entire project.** If it fails, ITSaNAS has no reason to exist. |
| `a_host_cannot_substitute_one_chunk_for_another` | A chunk sealed for address A does not open at address B, so a host cannot serve stale or swapped content undetected. |
| `purpose_confusion_is_rejected` | A chunk is not accepted where an oplog segment is expected. Type confusion across object kinds is a real source of protocol attacks. |
| `owner_confusion_is_rejected` | Ciphertext is bound to its owner, so objects cannot be attributed to the wrong user. |
| `every_single_bit_flip_is_detected` | **Exhaustive**: every bit of every byte of a sealed object is flipped in turn, and every one is rejected. Not sampled — all of them. A malicious host cannot corrupt a single bit silently. |
| `truncation_is_detected` | Cutting the ciphertext at any length fails to open, so a host cannot serve a partial chunk. |
| `an_unknown_format_version_is_refused_not_guessed_at` | An unknown version byte produces an explicit version error rather than a misparse. Forward compatibility that fails loudly. |
| `empty_and_undersized_inputs_do_not_panic` | Empty and short inputs at every length below the minimum return errors instead of panicking. A panic here is a remote denial of service, since these bytes come from an untrusted peer. |
| `empty_plaintext_is_a_valid_object` | A zero-byte file is a legitimate object, not an edge case that errors. |
| `associated_data_encoding_is_unambiguous` | Length-prefixing makes `("ab","c")` and `("a","bc")` distinct contexts, so binding cannot be bypassed by shifting a field boundary. |

## `keystore` — passphrase-protected storage (14)

| Test | What it proves |
| --- | --- |
| `round_trips_a_master_secret` | The primary use: a master secret sealed under a passphrase comes back intact. |
| `survives_serialisation` | The on-disk encoding round trips and still opens. Catches a header layout bug that would brick every existing keystore. |
| **`red_team_a_keystore_sealed_by_an_older_build_still_opens`** | A keystore sealed by `argon2` 0.5.3, stored as bytes in the test, must open in every later build. Every local keystore and escrow container is one Argon2id derivation from its keys, and a test that seals and opens in the same build cannot see a dependency that derives differently -- both halves change together. Written before moving to `argon2` 0.6.0, which it then passed; sabotaged by switching Argon2 to version 0x10. |
| `a_wrong_passphrase_fails` | Wrong, empty, and trailing-whitespace passphrases all fail. |
| **`downgrading_the_kdf_cost_is_detected`** | An attacker rewriting `memory_kib` from 64 MiB down to 8 KiB in a stolen container cannot then open it. Without binding the KDF parameters into the associated data, a stolen escrow blob could be made trivially brute-forceable. |
| `an_escrow_blob_cannot_be_passed_off_as_a_local_keystore` | Container labels are bound, so a stolen escrow blob cannot be dropped in as a device keystore. |
| `one_users_escrow_blob_cannot_be_served_for_another` | A malicious coordinator cannot hand Bob's client Alice's blob and have it open, even if they share a passphrase. |
| `tampering_with_the_salt_is_detected` | Salt modification is caught. |
| `tampering_with_the_ciphertext_is_detected` | Ciphertext modification is caught. |
| `each_lock_uses_a_fresh_salt` | Two locks of the same payload under the same passphrase differ. Salt reuse would let an attacker attack many containers with one dictionary pass. |
| `malformed_input_is_rejected_without_panicking` | Every truncation length, and unknown version and KDF bytes, produce errors not panics. This parser reads data supplied by the coordinator. |
| `recommended_parameters_actually_work` *(`#[ignore]`)* | The real 64 MiB / 3-pass Argon2id parameters function end to end. Fast tests use deliberately weak parameters; this is the one that exercises what ships. Runs in CI's **slow-tests** job. |

## `wellknown` — published-identity ban list (3)

| Test | What it proves |
| --- | --- |
| **`the_ban_list_matches_the_actual_fixture_identities`** | The three banned user ids are exactly the fixture identities, derived independently in this test. If the test kit and the ban list drift apart, the ban list would silently protect nothing while appearing to work. |
| `ordinary_identities_are_not_banned` | Normal users are unaffected — the ban list is not accidentally matching everything. |
| `the_ban_list_has_no_duplicate_or_empty_entries` | No all-zero or duplicated entries, so the list covers as many identities as it appears to. |

---

# `itsanas-crypto` — property tests (15)

Unit tests pin down specific known-dangerous cases; property tests check the
same guarantees hold across randomly generated inputs, which is where encoding
and length bugs hide. 256 cases each, in
[`tests/properties.rs`](../crates/itsanas-crypto/tests/properties.rs).

| Test | What it proves |
| --- | --- |
| `sealing_round_trips_for_any_plaintext` | Both sealing modes round trip for arbitrary content up to 4 KiB and arbitrary address lengths. |
| **`a_host_can_never_open_what_it_stores`** | For arbitrary key pairs and arbitrary content, a host cannot decrypt what it holds — with either of its own root keys. The generalised form of the project's central claim. |
| `any_single_byte_corruption_is_detected` | Corruption at a random position with a random non-zero delta is always caught. |
| `truncation_at_any_point_is_detected` | Truncation at any random offset is always caught. |
| `chunk_ids_are_stable_and_content_separating` | Addresses are stable per content and never collide across distinct content for one user. |
| `chunk_ids_never_align_across_users` | Two arbitrary users never derive the same address for the same content. |
| `signatures_bind_signer_domain_and_message` | Across arbitrary domains and messages, a signature verifies only for its signer and only under its own domain. |
| `recovery_phrases_round_trip` | Every possible 32-byte master secret produces a 24-word phrase that reconstructs it. |
| `a_mistyped_recovery_phrase_never_reconstructs_the_original_identity` | A transposed phrase never decodes back to the original master secret. **Documents a real limitation found by this test**: BIP-39 gives a 24-word phrase only an 8-bit checksum, so roughly 1 transposition in 256 decodes cleanly to a *different* valid identity. That cannot be fixed in this crate; the mitigation is at the CLI layer — see [DESIGN.md](DESIGN.md#recovery-must-be-verified-not-assumed). |
| `keystores_round_trip_and_reject_wrong_passphrases` | Arbitrary passphrases including empty and Unicode round trip, and any different passphrase fails. |
| `keystores_are_bound_to_their_label` | Arbitrary distinct labels never open each other's containers. |
| `keystore_encoding_round_trips` | The on-disk encoding is exact for arbitrary payloads. |
| `arbitrary_bytes_never_panic_the_keystore_parser` | Fuzz-style: random bytes never panic the parser. This input arrives from the coordinator. |
| `arbitrary_text_never_panics_the_identifier_parser` | Random text never panics identifier parsing. This input arrives from peers and config files. |
| `identifier_hex_round_trips` | Hex encoding round trips for every possible 32-byte value. |

---

# `itsanas-testkit` — fixture integrity (7)

These protect the test data itself. See [TEST-USERS.md](TEST-USERS.md).

| Test | What it proves |
| --- | --- |
| **`corpus_matches_its_published_digests`** | Every fixture file hashes to its pinned digest, and the corpus digest matches. The tamper check: since the corpus is generated from seeds in source, altering the test data requires editing reviewed code *and* moves a digest published in the documentation. |
| **`every_fixture_identity_is_banned_in_production`** | All three published users are refused by production. Their keys are printed in [TEST-USERS.md](TEST-USERS.md); this is what stops that being an attack. Enforcement is tested at the store layer by `the_published_test_identities_are_refused_by_the_normal_constructor`. |
| `recovery_phrases_rebuild_the_documented_identities` | Each phrase in the docs reconstructs exactly the user id claimed beside it. Ties documentation to code — a stale doc fails CI. |
| `canaries_are_unique_and_actually_present_in_plaintext` | Each canary is unique, genuinely present in its owner's plaintext, and absent from everyone else's. Without this, the on-disk plaintext-leak tests would pass **vacuously** — searching for a string that was never there. |
| `the_shared_document_gets_a_different_address_for_every_user` | On real corpus data: three users holding byte-identical content derive three unrelated chunk ids. |
| `filler_is_deterministic_and_seed_separated` | Generated content is reproducible and seed-separated, without which every pinned digest is meaningless. |
| `the_corpus_covers_edge_case_sizes` | The corpus contains an empty file, a file over 512 KiB, and a file whose size is not a multiple of 1 KiB. Guards the *test data* against becoming too tidy to catch boundary bugs. |

---

# `itsanas-store` — unit tests (142, plus the 25 vault tests below)

| **`red_team_a_holder_silent_past_the_window_stops_counting_as_a_copy`** | The ledger was optimistic in the one direction that loses data. Repair drained `under_replicated`, which counted **every holder record whatever its age** — so a machine that died six months ago still counted as one of your three copies and repair never fired. The only thing that withdraws those records is a *failed audit*, which needs that machine to answer; a dead one never does. The account believed it had three copies, had one, and nothing said otherwise. |
| `a_holder_that_keeps_answering_keeps_counting` | The other half, and what stops the window being a data-loss machine of its own: a window that expired live records would re-replicate a healthy fleet's entire content on a schedule. |

## `reliability` — remembering that a peer failed (9)

| Test | What it proves |
| --- | --- |
| **`red_team_a_host_that_keeps_discarding_stops_costing_bandwidth`** | The decision rule under the test above: three consecutive failures pause new content. |
| `one_failure_is_not_enough_to_stop_sending` | A host mid-restart, a swapped disk, a chunk collected on one side of a race. Reacting to a single failure would make a household stop syncing every time a machine rebooted at the wrong moment. |
| `the_lifetime_totals_survive_a_reset` | Consecutive failures decide the sanction; the totals are for somebody deciding whether to keep a peer at all, and clearing them on every pass would hide a host that fails half the time. |
| `counters_saturate_rather_than_wrapping` | A wrap would turn a peer that failed four billion challenges into a trusted one. |
| `a_paused_peer_explains_itself_and_a_healthy_one_says_nothing` | The message names the way back. |

## `holders` — the placement ledger's key layout (8)

| Test | What it proves |
| --- | --- |
| **`everything_one_device_holds_sorts_together_in_the_other_ordering`** | The reason a second ordering exists at all: without it, "what does this peer hold?" walks every row for every peer on every audit round. |
| `the_two_orderings_describe_the_same_pair` | Both encodings of one fact decode back to it. |
| **`every_holder_of_one_chunk_sorts_together`** | Why the chunk comes first in the composite key. If the device sorted first, "who holds this chunk?" would walk the whole table, which on a Pi with a million chunks is the difference between a repair pass that finishes and one that does not. |
| `a_chunk_held_only_here_is_flagged_as_the_only_copy` | The alert condition is distinguishable from an ordinary shortfall: everything else is background work, this one is a disk failure away from loss. |
| `a_chunk_held_more_widely_than_its_target_has_no_shortfall` | Saturating rather than wrapping. An underflow here would ask the repair loop for four billion pushes. |
| `a_key_round_trips_through_its_two_halves` / `a_key_of_the_wrong_length_is_refused_rather_than_guessed_at` | The encoding, and that a key written by something else is refused rather than reinterpreted. |
| `a_disk_check_pass_fits_in_the_refresh_window_at_any_interval` | §8 4c: `rows_per_round` sizes the rolling disk check so a whole pass fits in `REFRESH_AFTER` at any round interval -- 15 874 rows a round for sixteen million chunks at five minutes, 190 477 at an hour, never more rows than exist. The first plan's fixed 16 384 would take forty-two days a pass on an hourly policy. |

## `index` — the placement ledger (18)

Where this node's data actually went. This is what replaced the coordinator's
signed node-set epoch: an owner who already keeps a log of their own chunks can
record where they put them, and then no global membership list has to be agreed
by anybody. See [DESIGN.md](DESIGN.md) §8.

| Test | What it proves |
| --- | --- |
| **`the_loss_queue_is_read_from_a_moving_start_and_wraps`** | Bounded, because a node that lost a disk has millions of losses and reading them all would allocate a gigabyte before healing anything. From a cursor, because reading from the top of the key order every round lets a run of losses no reachable peer holds sit at the front for ever and starve everything behind it — the chunk that could have been repaired is never reached and nothing says why. |
| `a_loss_is_recorded_once_and_cleared_when_the_chunk_returns` | The queue is a set, not a log, and it shrinks. |
| **`collecting_a_chunk_stops_it_being_a_loss`** | A chunk nothing references cannot be missing. Leaving the entry would have repair asking peers for ever about data this node deliberately threw away — and telling them what it no longer has. |
| **`the_two_orderings_never_disagree_whatever_is_done_to_the_ledger`** | The ledger is kept in two key orders, because "who holds this chunk?" and "what does this peer hold?" are range scans under opposite prefixes and answering one with the wrong ordering is a full table walk — fourteen million rows per audit round at a terabyte. That is denormalised state, which this project refuses everywhere else, and the refusal is only earned if every path writing one writes the other in the same transaction. Exercised across recording, batching, forgetting a holder, forgetting a device, and collecting a chunk. |
| **`red_team_a_host_cannot_work_out_which_of_its_chunks_will_be_asked_about`** | The attack the *first* random version did not stop. Seeking to "the first record at or after a random cursor" is not uniform: it weights each record by the gap before it, and gaps between random ids are exponentially distributed. Harmless only while the host cannot tell which of its chunks sit behind the widest gaps — and ordered by chunk id it could, because it received the chunks. Simulated at sixteen questions a round, a host keeping the best 90% of the data passes **92%** of rounds where keeping 90% at random passes 18%: silently losing a tenth of somebody's files would have been invisible. So the ledger is ordered under a keyed hash, and this measures whether the key does the work. |
| **`the_audit_order_changes_completely_when_the_key_does`** | Two owners must not share an audit order, or a host could learn it from the challenges one owner sends and apply it to another's data. |
| **`a_ledger_ordered_by_chunk_id_is_rebuilt_under_the_audit_key_on_open`** | The dangerous half of the migration, because it looks healthy: right number of rows, wrong ordering. Left alone, every audit on that store stays predictable to the host — the exact weakness the key removes — and nothing anywhere says so. |
| **`red_team_the_same_question_is_not_asked_twice_every_round`** | The attack that broke the audit for six commits: keep the chunks that will be asked about, delete the rest. Selection used to sort by when each record was last confirmed — but a push round re-stamps a whole batch from one clock reading, so every timestamp in a batch was equal and the sort fell through to its tie-break, the chunk id. The same sixteen lowest ids, every round, for ever. Sixteen chunks out of fourteen million bought a spotless record. |
| `the_challenges_for_one_device_never_name_another_device_s_chunks` | Exhaustive over every cursor in the space: no draw may wander out of one peer's range into a neighbour's. Auditing a peer on another peer's records fails an innocent host. |
| **`every_holding_is_reachable_by_some_cursor`** | A cursor past the device's highest id wraps rather than being discarded. Without the wrap the lowest-numbered chunks would be the only ones never asked about — a hole an attacker can park its deletions in. |
| **`a_probe_is_remembered_until_the_peer_answers_for_it`** | The probe survives a failed round (the peer still owes an answer) and is cleared by a passing one (the sanction is over). A marker left standing after the pause lifts would misdirect the next round's questions. |
| **`a_ledger_written_before_the_second_ordering_is_rebuilt_on_open`** | Every write path writes both orderings, so they cannot drift while running — but they can *start* apart, on a store written before the device-first table existed. Left alone that is silent and total: challenge selection reads the second table, so no audit would ever ask anything, and a node that has stopped checking its hosts looks exactly like one whose hosts are honest. |
| **`a_target_counts_this_device_so_three_asks_for_two_elsewhere`** | The counting convention, pinned. Off by one here means the repair loop targets two copies while reporting three, and nothing ever says so — it surfaces as data loss after two machines die instead of after three. |
| **`one_chunk_nobody_holds_makes_the_whole_account_unrecoverable`** | Why the headline number is a minimum and not an average, in one test. Eight of nine holder records exist and not one complete copy does: a file comes back only if every chunk does, so almost-everywhere is nowhere. An average would report 2.0 here and read as comfortable. |
| **`a_peer_that_holds_every_chunk_is_reported_as_holding_everything`** | The other direction from the copy count, and not the same question. A peer holding a complete set is one broken cipher away from reading the account, and it is also what decides whether this scales: if the unit of hosting were "a whole account", somebody offering four terabytes would need peers who could each take four terabytes. The second half of the test spreads the same three chunks over three machines — same one complete copy, and nobody holding a whole set. |
| **`a_holder_nobody_has_heard_from_stops_counting_as_a_copy`** | A holder record is a memory: it says a device once acknowledged a chunk, not that the device still exists. This fleet had a destroyed machine listed as a holder until somebody read a log. Two claims and one observation must report one copy, not two — and a machine that has gone quiet is not one holding a share of you, it is one nobody can say anything about. |
| **`a_big_account_does_not_report_itself_lost_because_the_audit_is_slow`** | The arithmetic that decided the freshness rule is asked of the *machine* and not of the chunk. The audit re-checks sixteen chunks per peer per round at 300 s a round, so a chunk waits `chunks / 16` rounds for its turn — past ~64,500 chunks, four or five gigabytes, longer than the window. A per-chunk rule would report zero complete copies for a fleet whose every audit passes, at exactly the size this project is for. |
| **`this_machine_is_not_one_of_the_copies`** | The question is what survives losing this machine, so this machine does not count. Pins the difference from `under_replicated`, which counts it on purpose — the two answer different questions, and confusing them is how a backup report says two when the answer is one. |
| `an_account_with_nothing_stored_is_not_reported_as_unsafe` | Zero data is no question, not a failure. Reporting zero copies for an empty account trains somebody to ignore the number that matters. |
| **`the_chunks_closest_to_being_lost_are_reported_first`** | A repair pass on a laptop is interrupted by the lid closing. Ordered by chunk id, the work done before the interruption would be random with respect to risk, and the chunk with one copy left could wait behind a thousand that had two. |
| **`recording_the_same_holder_twice_refreshes_rather_than_duplicates`** | A peer syncing hourly acknowledges the same chunks every hour. One row per acknowledgement would grow the ledger without bound and inflate the replica count — wrong in the direction that hides a real shortage. |
| **`forgetting_a_device_clears_it_from_every_chunk_and_nothing_else`** | A peer that left stops being evidence for every chunk at once, and every other device's records survive. Otherwise losing one peer looks like losing all of them and the node re-uploads its entire store. |
| **`collecting_a_chunk_takes_its_holder_records_with_it`** | Garbage collection does not leave the repair loop working to restore the replication of a chunk that no longer exists. |
| **`the_ledger_survives_reopening`** | It is the only record of where this node put its data. Losing it on restart would make every node re-upload everything after a reboot. |
| `forgetting_a_holder_leaves_the_others_alone` | One failed storage challenge removes one host, not all of them. Removing more would make a single bad answer start a repair storm. |
| `an_unreferenced_chunk_is_not_reported_as_under_replicated` | Deleted and overwritten data is on its way out; restoring its replication is work done to keep something nobody wants. |
| `holders_come_back_sorted_so_two_devices_agree_on_order` | Two nodes comparing ledgers see the same order. |
| `holders_are_kept_apart_by_chunk` / `a_recorded_holder_comes_back` | The basic paths. |
| `recording_a_batch_matches_recording_one_at_a_time` | A sync round commits once rather than once per chunk, which on an SD card is most of the time spent. |
| `recording_an_empty_batch_does_nothing_rather_than_opening_a_transaction` | A quiet round costs no write. |
| **`red_team_the_holder_count_follows_the_ledger_through_every_path`** | §8 1c reads how much an account proved it hosts from `holder_counts`, kept in the transaction that writes each holder row. Record, refresh, every forget path and `forget_device` move it exactly; a refresh is not counted twice; another device's count does not move. Sabotaged by dropping the decrement in `forget_holders`. |

## `summary` — do we hold the same chunks? (6)

`src/summary.rs`. One hash instead of one identifier per chunk. The tests are
about the two ways a reconciliation can be worse than none: agreeing about
something nobody looked at, and disagreeing for a reason that is not data.

| Test | What it proves |
| --- | --- |
| **`two_machines_holding_the_same_set_agree_in_one_hash`** | The case that happens on almost every round of almost every day, and the reason this exists: saying "nothing changed" costs thirty-two bytes rather than a two-thousandth of the account. |
| **`one_chunk_missing_is_located_rather_than_merely_noticed`** | A differing hash has to say *where*, or the only possible response is to list everything — which is the cost it exists to avoid. |
| **`order_within_a_bucket_is_part_of_the_contract`** | Both sides scan a table keyed by chunk id, so both are sorted. If one ever were not, two honest machines would disagree for ever and it would look exactly like data loss. |
| `an_empty_set_has_a_defined_answer_on_both_sides` | Two nodes holding nothing agree without a special case, and one holding nothing does not accidentally agree with one holding something. |
| **`a_summary_of_a_different_length_is_all_disagreement`** | Comparing the overlap would report agreement about a part nobody looked at, which is the one answer a reconciliation must never give. |
| `one_bucket_hashed_alone_equals_its_place_in_the_summary` | `bucket_digest` over one bucket's chunks equals that bucket's digest in `buckets`, and chunks of other buckets in the slice change nothing: what lets a due walk re-check each bucket from the rows it re-stamps. |

## `chunker` — content-defined chunking (18)

| Test | What it proves |
| --- | --- |
| **`the_gear_table_is_pinned_forever`** | The 256-entry gear table hashes to a fixed digest. If it ever changes, every chunk boundary in the network moves: existing stores stop deduplicating against new writes and every client re-uploads every file. This is the test that makes deriving the table safer than pasting one in. |
| **`inserting_a_byte_at_the_front_shifts_only_local_boundaries`** | Over 90% of chunks survive a one-byte prefix insertion into 4 MiB. This is the entire reason content-defined chunking exists; with fixed-size chunking it finds zero. |
| `editing_the_middle_leaves_both_ends_intact` | The same property for a mid-file insertion. |
| **`the_average_chunk_size_is_close_to_the_target`** | Empirical mean chunk size is within 2× of the configured average. A mistranscribed cut mask still produces valid, reassembling chunks — just at the wrong size, quietly wrecking the dedup/overhead trade-off. Nothing else would catch that. |
| `chunks_reassemble_into_the_original_bytes` | Chunking loses and reorders nothing, across six sizes from 0 to 1 MiB. |
| `chunking_is_deterministic` | Two runs agree on boundaries, so two devices will too. |
| `size_bounds_are_respected` | Every non-final chunk sits within min and max. |
| **`highly_repetitive_data_still_terminates_and_respects_the_maximum`** | Long runs of one byte are the pathological case for a rolling hash — the hash can settle into a state where the mask never matches. Proves the max-size ceiling stops that producing one enormous chunk. |
| `a_buffer_shorter_than_the_minimum_is_one_chunk` | The minimum is enforced. |
| `offsets_are_contiguous_and_start_at_zero` | Chunk offsets tile the input exactly. |
| `an_empty_buffer_produces_no_chunks` | Zero-length input is not a special case that panics. |
| `invalid_configurations_are_rejected` | Out-of-order or zero bounds fail at construction. |
| **`small_configurations_do_not_panic_on_mask_lookup`** | Extreme averages do not index off the end of the mask table. This test found a real bug: the loose mask index overran for any average above 2^27. |

## `blob` — content-addressed storage (11)

| Test | What it proves |
| --- | --- |
| `round_trips_what_it_was_given` | Sealed bytes come back byte-identical. |
| **`storing_the_same_address_twice_writes_once`** | Deduplication actually saves a write rather than silently rewriting. |
| **`a_blob_lands_at_a_sharded_path_not_a_flat_one`** | Two-level fan-out is real. A flat directory degrades badly at a million chunks on both ext4 and NTFS. |
| **`files_that_are_not_blobs_are_ignored_by_the_scan`** | Garbage collection deletes what the scan reports, so a scan that reported a foreign file would delete a stranger's data. |
| **`no_staging_file_survives_a_successful_write`** | The write-then-rename path cleans up, so writes do not leak a temp file each time. |
| **`sweeping_removes_crash_leftovers_but_not_blobs`** | Crash recovery removes abandoned staging files and touches nothing else. |
| `removal_is_idempotent` | Deleting twice is not an error. |
| `addresses_lists_everything_across_the_fan_out` | The scan finds blobs in every shard directory. |
| `a_missing_address_is_none_not_an_error` | Absence is a value, not a failure. |
| `an_empty_blob_is_storable_and_distinguishable_from_a_missing_one` | Zero-length content is not confused with absence. |
| `total_bytes_counts_stored_bytes` | Size accounting is correct. |

## `index` — transactional metadata (12)

| Test | What it proves |
| --- | --- |
| **`red_team_an_open_racing_a_lock_probe_still_opens`** | `Index::is_locked` (what `status` and the tray ask) probes by opening, which takes the exclusive lock for an instant; a daemon starting in that instant failed with `Locked` and stayed down. `Index::open` now waits out a lock for up to `LOCK_PATIENCE` (2 s). A thread probing every 5 ms while 100 opens run: none fails; a lock held for good is still `Locked`. A probe spinning with no pause starved every open on Linux CI (a load no caller makes; the tray polls every few seconds). Sabotaged (patience 0): red, 78 of 100 opens gave up. |
| **`two_files_sharing_a_chunk_both_hold_it`** | Deleting one of two files that share a chunk does not take the other's data with it. |
| **`a_file_that_repeats_a_chunk_counts_each_occurrence`** | A file of ten identical blocks references one chunk ten times. Getting this wrong frees live data on the first delete. |
| **`a_chunk_can_be_resurrected_before_it_is_collected`** | Restoring identical content before GC runs takes the chunk out of the collection queue, so GC does not delete a blob that is live again. |
| `overwriting_a_file_releases_only_the_chunks_it_stopped_using` | An overwrite computes the right delta rather than releasing everything. |
| `adding_a_file_references_its_chunks` | Reference counts go up on write. |
| `a_fresh_index_reads_as_empty_rather_than_erroring` | A brand-new store reads empty instead of failing on a table that was never written. |
| `state_survives_reopening` | Data is durable across a process restart. |
| `files_come_back_sorted_so_two_devices_agree_on_order` | Iteration order is deterministic, which matters once two devices compare listings. |
| `a_file_round_trips` | Entries store and load unchanged. |
| **`removing_an_absent_file_still_records_the_tombstone`** | A device deleting a file it never downloaded must still record the deletion, or a machine that was away when a file arrived cannot take part in removing it — and the file comes back on the next sync. |
| `forgetting_a_chunk_clears_both_tables` | Post-GC cleanup leaves no half-state. |

## `oplog` — the operation log (15)

| Test | What it proves |
| --- | --- |
| **`a_host_can_verify_a_segment_without_being_able_to_read_it`** | The entire bargain: hosts police authenticity, owners read. A stranger's key cannot open the body. |
| **`the_sealed_body_does_not_leak_the_path_in_plaintext`** | A filename does not appear in the encoded segment. Without this, hosts learn what their peers store. |
| **`a_host_dropping_a_segment_from_the_middle_is_detected`** | The concrete attack: a host holds segments 1–3 and serves only 1 and 3, hiding whatever change 2 carried. The chain link catches it. |
| **`a_sequence_gap_is_detected_even_when_the_chain_links_up`** | A compromised device cannot skip sequence numbers while chaining correctly and leave a peer believing its history is complete. |
| **`an_envelope_that_lies_about_its_body_is_caught_on_open`** | A validly re-signed envelope claiming the wrong sequence range is still rejected, because the body is cross-checked against the envelope's claims. |
| **`tampering_with_any_envelope_field_invalidates_the_signature`** | Five separate fields, each mutated independently. |
| **`a_segment_signed_by_another_device_is_rejected`** | One device cannot forge a segment attributed to another. |
| **`malformed_bytes_do_not_panic_the_decoder`** | Every truncation and 200 single-byte corruptions of a valid segment return errors rather than panicking. A host controls these bytes entirely. |
| `two_segments_with_identical_entries_are_still_distinct_objects` | Randomised sealing and random object ids stop two identical batches colliding in the blob store. |
| `a_valid_chain_validates` | The honest case passes. |
| `a_chain_whose_first_segment_claims_a_predecessor_is_still_walked` | Starting mid-chain is legitimate for a peer catching up. |
| `an_empty_chain_is_vacuously_valid` | Nothing to check is not an error. |
| `an_empty_segment_is_refused` | No wasting a sequence number and an object id on nothing. |
| `a_segment_round_trips_for_its_owner` | The happy path works. |
| `encoding_round_trips_through_the_wire_format` | Serialisation preserves everything, including verifiability. |

## `path` — logical path validation (10)

Paths arrive from a peer's operation log, so they are attacker-controlled the
moment the sync engine starts materialising files.

| Test | What it proves |
| --- | --- |
| `the_two_unicode_spellings_of_one_name_are_two_paths_today` | Pins a contract rather than a defence. macOS returns decomposed filenames (NFD) where Linux and Windows use composed (NFC), and nothing here normalises, so one `Café.txt` is two logical paths. Reproduced on Windows without a Mac on 2026-09-16: two `put`s, two stored files, `scan` printing `out  Unicode/Café.txt` **twice** with `0 conflicts`. Nothing is lost; the person gets a duplicate they cannot tell apart. This test exists so that adding normalisation is a decision somebody takes on purpose, not a silent change that makes every existing accented path unreachable. |
| **`traversal_is_rejected_in_every_position`** | `..` is refused leading, trailing and interior. Without this a peer writes `../../../.ssh/authorized_keys`. |
| **`absolute_paths_are_rejected`** | Unix absolute paths and Windows drive-letter prefixes both refused. |
| **`backslashes_are_rejected_rather_than_translated`** | Translating would make `a\b` and `a/b` name one file on Windows and two on Linux, so the devices would diverge. |
| **`windows_device_names_are_rejected`** | The Pi will happily create `com1.txt`; the laptop must never try to open a serial port. Includes negative cases (`console.txt`, `com10`) so the rule is not over-broad. |
| **`trailing_spaces_and_dots_are_rejected`** | Windows silently strips these, so `evil.txt ` and `evil.txt` would collide on one device and not another. |
| `control_characters_are_rejected` | NUL, newline, carriage return and tab refused. |
| `malformed_separators_are_rejected` | Empty, doubled and trailing separators refused. |
| `oversized_paths_and_components_are_rejected` | Bounds enforced, so one log entry cannot make the index enormous. |
| `ordinary_paths_are_accepted` | The rules are not so strict that normal filenames — including Unicode and dotfiles — break. |

---

# `itsanas-store` — integration tests (47)

Full path from plaintext to disk and back. `tests/store.rs`.

| Test | What it proves |
| --- | --- |
| **`alices_entire_corpus_round_trips_byte_identical`** | M2 exit criterion. Every fixture file survives chunking, sealing, storage and reassembly unchanged. |
| **`no_users_plaintext_ever_touches_the_disk`** | M2 exit criterion, and the single most important property in the project. Both canaries are scanned against both stores — a user's own store must not leak their plaintext either, because that laptop can be stolen. Includes a vacuity check proving the canary really is in the plaintext, so the scan cannot pass by scanning nothing. |
| **`an_insertion_at_the_start_of_a_large_file_reuses_almost_every_chunk`** | M2 exit criterion, end to end through the real store. |
| **`two_users_storing_the_same_document_produce_unrelated_chunk_ids`** | Two users storing byte-identical content get disjoint addresses. If addresses were plain content hashes a host could correlate users and confirm guessed files. |
| **`one_users_store_cannot_be_opened_with_another_users_keys`** | Sealing is bound to the owner, not merely to the directory. |
| **`red_team_a_write_past_the_budget_leaves_no_chunk_no_entry_and_no_log`** | A 1 MiB file offered to an account with 344 KiB left, through `write_file`, so nothing asks its size first and the refusal can only come part-way through the stream -- after chunks were sealed and stored. It must be refused with the account, the limit and the total it would have reached, and leave no blob, no index entry and no log entry behind. A folder pass retries a refused file every round, so debris here is a disk filling with the first part of the same file again and again. Sabotaged twice: without the in-stream check the file is accepted; without the clean-up the blobs stay. (HANDOVER §8 0n.) |
| **`red_team_a_write_past_the_disk_room_is_refused_and_leaves_nothing`** | The account may have room while the disk has not: what this machine's pledge still owes is space promised to others. A 1 MiB write against a 300 KiB local ceiling (100 KiB already held) is refused as `DiskFull` with the numbers and leaves no chunk and no index entry; a 100 KiB write inside it succeeds, so a store that refuses everything cannot pass. Sabotaged (the disk check never fires): red. (HANDOVER §8 1b.) |
| **`red_team_a_pull_charges_the_folder_copy_too`** | A pull writes the file twice when a folder is synced: the store's chunks and the folder's plaintext. The ceiling charged one, so a pull of X under a room of X put 2X on the disk. With the folder on the home's volume a 100 KiB pull under 100 KiB of room is refused and 50 KiB fits; bytes pulled since the budget was set count twice too (25 here + 25 fits, + 26 does not); with the folder on another volume, its free space bounds the pull. Sabotaged twice (charged once; other volume ignored): red. |
| **`red_team_many_small_writes_cannot_pass_the_disk_ceiling_together`** | The disk bound is a ceiling on the account's local bytes, not a room each write is checked against alone: a folder pass imports many files after one `bound_writes`, and the first version let a hundred 1 GB files into 10 GB of room (found by the `itsanas-redteam` agent before merge). Five 100 KiB files against 300 KiB: three fit, the fourth is refused; an edit in place is charged its growth. Sabotaged (per-write room again): red. |
| **`a_write_inside_the_budget_succeeds_and_an_edit_is_charged_only_its_growth`** | Keeps the test above from passing on a store that refuses everything, and pins what is charged: growing a 600 KiB file to 700 KiB in a 1 MiB account succeeds (the old version is not counted twice), 400 KiB more is refused naming 700 KiB held, and deleting the file makes the room back. Sabotaged on the replaced-size subtraction and on the cached-total adjustments of `put_file` and `remove_file`. `release_file`'s adjustment is the same line as `remove_file`'s and was broken with it; no test isolates it. |
| **`red_team_what_the_account_holds_elsewhere_counts_against_it`** | A phone keeping 2 GB of a 40 GB account holds 2 GB locally. Counting only that would let it write 38 GB past what the account may hold: bytes known only from other devices' logs count too. |
| **`the_published_test_identities_are_refused_by_the_normal_constructor`** | The claim README.md and SECURITY.md both make. Before this test the ban-list function was defined, exported, and called by nothing. |
| **`red_team_a_held_store_says_so_before_anybody_is_asked_for_a_key`** | `itsanas status` prints the daemon's snapshot when the store is locked -- the normal state of a working machine. It used to reach that arm through an open that resolves the passphrase *first*, so on the laptop the command answered "no terminal to prompt on": it demanded the keystore secret in order to print a plaintext file lying beside it. If the probe regresses, the owner of a running node cannot ask whether their data is safe without unsealing their keys, and `MVP.md` test L cannot pass. |
| **`a_chunk_served_under_the_wrong_address_does_not_decrypt`** | The substitution attack, with two genuine chunks from the same user. |
| **`a_corrupted_blob_is_detected_and_never_returned_as_content`** | A flipped bit in stored ciphertext surfaces as an error, not as data. |
| **`a_deleted_blob_is_reported_rather_than_silently_returning_short_data`** | A missing chunk fails the read instead of returning a truncated file. |
| **`garbage_collection_honours_the_grace_period`** | Nothing is deleted inside the grace window — a peer may still be fetching it — and everything is once the window passes. |
| **`deleting_one_of_two_identical_files_keeps_the_other_readable`** | GC with shared chunks does not destroy live data. |
| **`unsealed_writes_survive_a_restart_and_are_announced_afterwards`** | Simulates a power cut between a write and the next flush. The entry is not lost, so the peer still learns the file exists. |
| **`every_write_is_announced_in_the_log_exactly_once`** | Sequence numbers are dense from 1, and a second flush re-emits nothing — so a peer never replays an entry twice. |
| **`the_segment_chain_links_up_across_many_flushes`** | Five flushes produce a chain with no gaps and non-overlapping sequence ranges. |
| `identical_files_stored_twice_occupy_one_copy_on_disk` | Deduplication measured in bytes on disk, not merely in chunk ids. |
| `overwriting_a_file_eventually_reclaims_the_bytes_it_stopped_using` | 1 MiB overwritten by 1 KiB drops below a tenth of the original size after GC. |
| `a_store_reopens_with_everything_intact` | Everything survives a restart, and the reopened store reports healthy. |
| `a_healthy_store_reports_healthy` | The integrity check does not cry wolf on a good store, and writing files leaves no orphan blobs. |
| `the_store_rejects_paths_that_would_escape_the_sync_root` | Path validation is wired into the store, not merely available. |
| `a_file_larger_than_one_chunk_uses_several_and_still_verifies` | Multi-chunk files reassemble and hash correctly. |
| `an_empty_file_is_stored_and_distinguishable_from_a_missing_one` | Zero chunks is a valid file, distinct from absence. |
| `a_non_default_chunker_still_round_trips` | The tuning knob does not produce unreadable data. |

---
| **`red_team_a_node_does_not_tell_its_own_account_it_holds_what_its_disk_lost`** | Found by Rodin on the 4c plan: a machine answers its own account's summary from its index, so a Pi whose disk lost a blob kept agreeing with the laptop, which re-stamped it as a copy. `Store::check_disk` records the loss and `held_summary` leaves it out, while `chunk_summary` -- the set this node wants held -- keeps it. Sabotaged by counting losses in `held_summary`. |

# `itsanas-sync` — unit tests (12)

## `conflict` — deciding who keeps the original path (10)

| Test | What it proves |
| --- | --- |
| **`the_winner_is_the_same_whichever_side_asks`** | The rule is antisymmetric. If it were not, both devices would each believe they won, both would write to the original path, and they would overwrite each other forever. |
| **`the_higher_device_id_wins_regardless_of_write_count`** | A device that has written a thousand times does not thereby beat one that wrote twice — the outcome must not depend on unrelated activity elsewhere. |
| **`the_order_is_total_so_no_pair_is_ever_undecided`** | Strict and antisymmetric across every pair, including a version against itself. |
| **`the_marker_goes_before_the_extension`** | `report.pdf` → `report.conflict-….pdf`, not `report.pdf.conflict`, which Windows would associate with nothing. |
| **`a_dotfile_keeps_its_leading_dot`** | `.bashrc` does not become `.conflict-….bashrc`, which would be a different, no-longer-hidden file. |
| **`only_the_final_component_is_examined_for_an_extension`** | A directory called `my.files` does not swallow the marker. |
| **`two_different_devices_produce_two_different_siblings`** | Three-way conflicts are rare but real; two losers colliding on one sibling path would destroy one of them. |
| **`the_sibling_path_is_a_valid_logical_path`** | The generated name survives the store's own path validation — it is about to be written to a real store. |
| `a_file_with_no_extension_gets_the_marker_appended` | Extension-less names are handled. |
| `a_multi_dot_name_splits_on_the_last_dot` | `archive.tar.gz` splits sensibly. |

## `engine` — sync reporting (2)

| Test | What it proves |
| --- | --- |
| `a_report_counts_every_outcome_kind` | Each of the seven outcomes is tallied, a file with no room counts as deferred as well as `no_room`, and a deferred operation asks for another round. |
| **`a_quiet_round_reports_no_work_and_no_retry`** | A round that only recognised things it already knew reports no progress. If it reported progress, the settle loop would never terminate. |

---

# `itsanas-sync` — convergence tests (24)

The M3 exit criteria. Real stores, real chunking, real sealing, real signatures;
only the network is simulated. Nothing uses randomness or wall-clock time, so a
failure reproduces exactly. `tests/convergence.rs`.

| Test | What it proves |
| --- | --- |
| **`a_host_that_loses_everything_is_survivable_because_the_devices_still_have_it`** | The premise the architecture rests on, as a scenario rather than a sentence in a design document: hosts are blind *and* untrustworthy, so one losing its whole disk must cost nothing but bandwidth. Written because `Cloud::forget_all_chunks` existed for exactly this and nothing called it — an anticipated scenario that was never written down. |
| **`a_device_that_is_offline_publishes_nothing_and_blocks_nobody`** | The ordinary state of this network: most machines are off most of the time. An offline device must not stop the others converging, and must not appear to have published work it never sent. |
| **`a_device_that_never_comes_back_still_gets_its_work_to_everyone_else`** | The scenario the whole architecture exists for. The Pi writes at 3am, publishes, and is switched off permanently; the laptop and VM still converge on its work, having never spoken to it. |
| **`work_propagates_through_a_third_device_that_only_relays`** | The Pi and the VM are never online simultaneously. The work still reaches the VM via the laptop. |
| **`concurrent_edits_produce_both_files_and_lose_neither`** | Two edits during a partition yield two files on every device, with both bodies intact. |
| **`a_three_way_conflict_produces_three_distinct_files`** | All three versions survive a full partition; none is silently dropped. |
| **`a_sequential_edit_is_not_treated_as_a_conflict`** | The common case stays clean. If ordinary edits produced siblings the folder would fill with junk and the feature would be worse than useless. |
| **`a_delete_racing_an_edit_never_destroys_the_edit`** | The asymmetry rule. A concurrent delete loses, because a lost edit is unrecoverable and an unexpected resurrection takes a second to undo. |
| **`a_delete_that_saw_the_edit_is_honoured`** | The counterpart: a normal delete actually deletes, on every device. Without this the product does not work. |
| **`an_offline_device_does_not_resurrect_a_file_deleted_while_it_slept`** | Tombstones do their job. Without them the returning device re-announces what it still holds and the file comes back from the dead everywhere. |
| **`re_creating_a_deleted_file_works_and_converges`** | A path can go live → deleted → live again without ending up both present and deleted. |
| **`the_final_state_does_not_depend_on_the_order_devices_sync_in`** | The same divergence healed in two opposite orders reaches an identical state. Order dependence here would be a permanent, silent disagreement in production. |
| **`syncing_repeatedly_changes_nothing`** | Hosts re-serve segments freely and there is no acknowledgement telling them to stop, so applying an operation twice must be a no-op. |
| **`re_resolving_a_conflict_is_idempotent`** | Guards the specific bug this suite caught: a conflict re-resolved every round means a settle loop that stops when nothing changes never stops. |
| **`a_long_run_of_alternating_partitions_still_converges`** | Ten rounds of rotating partitions, twenty files, full agreement at the end. More history than a hand-built scenario covers. |
| **`red_team_a_chunk_that_does_not_match_its_address_leaves_the_file_deferred`** | The only host answers with noise of the right length. `accept_chunk` refused it and the engine counted it as fetched: the file was adopted with a hole, the round read as finished and a session moved its markers past the segment. Now deferred, and the honest bytes complete it. |
| **`an_operation_whose_chunks_are_unavailable_is_deferred_not_half_applied`** | A segment can arrive before its chunks. Materialising anyway would create a file that exists but cannot be read. |
| **`a_deferred_operation_completes_once_its_chunks_show_up`** | And the retry actually completes. |
| **`red_team_a_pull_past_the_disk_ceiling_fetches_nothing_and_waits`** | 8.1b's pull half. A laptop with 100 KB of disk ceiling for the account is offered the Pi's 300 KB file: it must come back `NoRoom`, counted as deferred so the round is retried, with **no chunk of it fetched** (a refusal half-way would leave chunks no index entry counts) and no index entry; raised to 400 KB, the next round brings it in whole. Sabotaged on the check in `apply_upsert`: red. |
| **`red_team_a_conflict_is_charged_both_versions_against_the_disk`** | A conflict keeps both files, so the incoming version frees nothing: a 300 KB version against our 1 KB one must be refused by a ceiling with 500 bytes to spare. Then, with room for exactly both, the conflict resolves; replayed on a disk with no room left, it must read as known rather than refused, or the round never finishes. Sabotaged three times (the check in `apply_conflict` removed; the incoming side charged as a replacement; the check moved above the idempotence check): red each time. |
| **`the_hosts_hold_everything_and_can_read_none_of_it`** | Every byte the simulated hosts hold is scanned for Alice's canary *and* for each of her filenames. Includes a vacuity check proving the canary is really in the data. |
| **`every_segment_a_host_holds_is_verifiable_by_that_host`** | Hosts cannot read segments but must be able to authenticate them, or anyone could flood a host with garbage attributed to a peer. |
| **`a_full_corpus_converges_across_three_devices_with_partitions`** | The realistic end-to-end case: a real data set written across three devices that are never all online together, converging byte-identically. |
| **`version_vectors_order_sequential_writes_and_flag_concurrent_ones`** | The underlying primitive, checked at the level of real stores rather than in isolation. |
| **`content_is_not_released_until_two_other_machines_have_it`** | The one store operation that destroys data if it is wrong. A device with a limit has to be able to let go of files, and the difference between that and eating the second copy to make room is this check — one remaining copy is not a floor, it is the last one. Asserts the refusal at zero holders *and* at one. Confirmed by sabotage: removing the guard turns this and the next test red. |
| **`a_holder_nobody_has_heard_from_does_not_authorise_letting_go`** | An acknowledgement is evidence about the past. `coverage` already refuses to count a silent machine as a copy; letting go of local content on the strength of one is worse, because it acts on the belief rather than reporting it. Reachable only by moving the clock, because recording a holder *is* contact. |
| **`red_team_a_stranger_that_only_claims_to_hold_a_chunk_cannot_make_it_releasable`** | The worst finding of the sweep, and it needed no bug in the crypto, the transport or the accounting — two throwaway keypairs. `Request::Hosted` records a holder for any device that completes a handshake; the answer to "what stops a liar?" was the owner's storage challenges, and `session::audit` ran only from `sync_once`, which only ever runs against peers this node **dials**. A device that only ever dials in appeared in no peer list and was never challenged once. So: mint two keys, ask `WantHosted` for the chunks with fewest copies, answer `Hosted` holding nothing, and the owner believed two live fresh holders existed. A device over its keep budget then deleted its only copy — free, remote, permanent, and invisible, because `coverage` and `status` both read the same ledger. Releasing now counts **proved** holders: ones that have answered a challenge, which costs holding the bytes. The test asserts all three steps of the ladder — nothing on two liars, still nothing on one proved, gone on two. |
| **`releasing_one_file_leaves_a_chunk_another_file_still_uses`** | Deduplication means two paths can share a chunk. Freeing by path rather than by reference would empty half of a file the device was told to keep, and the damage would surface only the next time somebody opened it. |
| **`a_file_this_device_made_and_released_is_still_listed_and_still_fetchable`** | Found on a Raspberry Pi, not in a test: a file put on a device with a 300 KiB limit, pushed to two hosts, released exactly as designed — and then gone from `itsanas ls` on the machine that made it, with `itsanas get` answering "no such file" for a file two other machines were holding. The catalogue walked only *other* devices' chains, on a rule that stopped being true the day content could be released. Fails when the walk over this device's own log is removed. |
| **`a_deleted_file_is_not_resurrected_by_reading_this_devices_own_log`** | The other half. Reading one's own chain must not bring back everything one has ever deleted. |
| **`a_file_can_be_released_fetched_back_and_released_again`** | A limit has to work more than once. Releasing erased the holder ledger along with the local copy, reusing the rule garbage collection needs — where a chunk goes because its *file* went. A release is the opposite case: the file is still in the account and the copies elsewhere are what made letting go safe. Observed on a Raspberry Pi before it was fixed: a device stuck at 380 KiB against a 300 KiB limit, refusing round after round because it had forgotten the second holder while fetching the file back. Fails when `release_chunk` is put back to `forget_chunk`. |
| **`a_reachable_machine_with_stale_records_does_not_authorise_a_release`** | The case a per-machine liveness rule cannot see, and the one that costs data: a peer that stays online and empties its disk. the count of live holders asked only whether the *device* had been heard from, so such a peer counted as a copy for ever — and after a release there is no audit left to contradict it. Two bars now, and the per-`(chunk, device)` timestamp that tells them apart was already being written and read by nothing. Fails when the second bar is removed. |
| **`the_chunks_a_device_holds_can_be_paged_without_walking_the_directories`** | The push sweep built its list with `BlobStore::addresses`, which walks the fan-out directories and says in its own documentation that it is never on a hot path. It was on the hottest one: every round, per peer — at a terabyte, a recursive `readdir` over sixteen million files every five minutes and 537 MB allocated in one go, against a measured peak of 17 MiB. The replacement pages from the index in chunk order; the test checks the paging is exact, and that chunks awaiting collection stay in it, because a replay needs the chunks of the operation it is applying and not only of the current state. |

---

# `itsanas-store` — the vault (25 of the store's unit tests)

Storage for *other people's* data. The vault holds no keys and no constructor
takes one, so these tests are about accepting, serving and accounting — never
about reading.

| Test | What it proves |
| --- | --- |
| **`red_team_held_bytes_for_one_owner_needs_no_walk`** | `Node::held_for_others` runs on every reconcile pass and called `stats_for`, two walks of our own blobs each time. `held_bytes_for` reads a per-owner running total (the vault_owner_chunk_bytes table, changed in the chunk's own transaction) plus the owner's chain rows. It equals the walk after puts and a removal, keeps its answer when a blob file is deleted behind the vault's back (so it did not walk), and an unclean open rebuilds it from the disk. Sabotaged twice (`stats_for` again; the rebuild not writing the per-owner rows): red. |
| **`a_segment_with_a_bad_signature_is_refused_before_it_is_stored`** | A host that stored unverified envelopes would be a convenient way to attribute garbage to someone else's device. |
| **`a_segment_that_does_not_continue_the_chain_is_refused`** | Otherwise a host can be induced to store a chain with a hole and then serve that hole to a peer as though it were complete. |
| **`re_offering_the_current_tip_is_accepted_as_a_no_op`** | Peers re-offer freely — there is no acknowledgement telling them to stop — so this must neither error nor duplicate. |
| **`an_owner_whose_chunks_are_held_but_whose_log_is_not_still_counts`** | Guards a real bug this suite caught: the owner list was derived from the segment table alone, so a host storing chunks but no segments reported zero bytes and its quota was blind to the bulk of what it held. |
| **`red_team_segments_count_against_the_pledge_like_any_other_foreign_byte`** | `would_exceed_pledge` read `stats().bytes` (now `held_bytes`, the same number kept as a running total), and `bytes` summed the chunk blobs alone. Log segments live in their own table and counted for nothing, so `held` stayed at zero however many arrived: **every `StoreSegment` passed the quota, for ever**, on any host whose pledge exceeded one segment. No account and no invitation were needed — a throwaway device key completes the handshake, and a self-signed envelope of random bytes is indistinguishable from a real one because nobody can decrypt either. About 1,280 frames put 10 GiB on the disk; there is no segment-removal API and redb does not shrink, so it was not reclaimable, and `itsanas status` reads the same field so the operator watched a disk fill with no cause. A running byte total per chain now feeds the quota, backfilled once on open for vaults that predate it. |
| **`two_owners_chunks_do_not_collide_even_at_the_same_address`** | Chunk ids are blinded per user so a collision should not happen, but correctness must not depend on that. |
| **`one_owners_segments_are_never_served_under_another_owners_name`** | Owner scoping is real, not incidental. |
| **`resuming_after_an_unknown_segment_returns_nothing_rather_than_everything`** | An unrecognised resume point must not cause the whole chain to be re-sent. |
| **`resuming_after_a_segment_skips_what_the_caller_already_has`** | The resume path works, so a catching-up peer does not re-download its own history. |
| `a_chain_is_stored_and_served_in_order` | What comes back validates as a chain. |
| `the_limit_caps_the_response` | One request cannot ask for unbounded work. |
| `heads_are_reported_per_device_and_scoped_to_one_owner` | Head reporting is per device and does not leak across owners. |
| `stats_account_for_every_owner` | Quota accounting sums correctly. |
| `a_chunk_round_trips_without_the_vault_ever_holding_a_key` | The basic path. |
| `everything_survives_reopening` | Durable across a restart. |
| **`red_team_the_held_total_is_the_walk_after_every_kind_of_write`** | The pledge reads `held_bytes`, a running total, instead of walking every blob under the storing lock (3.4 s per refused offer at 50,000 chunks on the laptop, about 1 µs now). The total must equal the walk after puts for two owners, re-puts of a held address at a larger and a *smaller* size (the file on disk is kept, so indexing the offered length would let a peer store 8 MiB and have 1 byte counted), deletes repeated and of nothing, a segment, and a clean reopen. Fails when a delete skips the total, a put indexes `sealed.len()`, or a re-put does not subtract the old row. |
| **`red_team_a_crash_between_a_blob_and_its_row_is_rebuilt_at_open`** | A blob is written, then indexed; a crash between the two leaves a blob nobody counts, or a row for a blob gone. A vault not closed cleanly is rebuilt from its directories at the next open, rows removed as well as added, and the crash's staging file swept (counted by nothing, otherwise never reclaimed). Fails when the unclean mark is ignored, stale rows are kept or staging is not swept. |
| **`red_team_a_write_that_fails_after_its_blob_is_rebuilt_after_a_clean_close`** | Found by `itsanas-redteam`: a blob written whose commit then fails (ENOSPC on redb) leaves an uncounted blob, and a later *clean* shutdown cleared the mark and blessed the drift for good. A write that does not reach its commit, by error or panic, now marks the vault suspect and the close stays unclean. Fails when `Drop` ignores the mark or the guard is defused early. |
| **`red_team_an_open_that_fails_mid_rebuild_leaves_the_vault_unclean`** | Also `itsanas-redteam`: an open whose rebuild failed (an antivirus lock, a permission) dropped the half-built vault through the clean close, so the retry trusted the total it never fixed. The vault is suspect until `open` returns. Fails when it starts unsuspect. |
| `a_vault_from_before_the_total_is_totalled_at_open` | An upgraded vault has index rows and no total; reading it as 0 would hand the pledge back in full. Fails when a missing total is not rebuilt. |

---
| **`red_team_a_blob_lost_behind_the_index_is_found_within_one_pass`** | §8 4c. The summary an owner compares is read from the vault's index, and since 4c an agreeing summary re-stamps records without asking: a blob gone from the disk behind the index left the summary agreeing for ever. One pass of `Vault::check_disk` removes the row, the summary changes, and the pledge stops counting the bytes. Sabotaged by keeping the row. |
| **`red_team_the_disk_check_resumes_where_it_stopped_after_a_restart`** | The check's cursor is on disk: a daemon restarted more often than a pass takes must not check the first rows for ever. A blob lost at the end is found after a reopen. Sabotaged by not keeping the cursor. |
| **`red_team_one_disk_check_call_holds_at_most_one_batch`** | Found by the CI reviewer on #218: the slice a round owes is 190 477 rows at a terabyte on an hourly policy, 12 MB of keys in one `Vec` on a 17 MiB machine. One call examines at most `MAX_CHECK_BATCH` and the daemon loops. Sabotaged by dropping the cap. |
| `the_disk_check_keeps_every_row_whose_blob_is_there` | A healthy disk loses nothing to the check: every row examined, none removed, the summary unchanged. |

# `itsanas-net` — unit tests (45)

## `protocol` — messages and challenges (12)

| Test | What it proves |
| --- | --- |
| **`a_proof_for_one_nonce_does_not_answer_another`** | Otherwise a host computes one proof, throws the chunk away, and answers every future challenge from cache. |
| **`a_proof_requires_the_actual_bytes`** | A host that discarded the chunk fails. |
| **`red_team_a_claim_to_hold_things_is_bounded_like_the_claim_to_have_dropped_them`** | `Dropped` withdraws holder records and `Hosted` writes them. The first was bounded at `MAX_HAVE_BATCH` and the second fell through to `_ => true`, so one frame could add rows to a victim's index without limit — permanent rows, in the table `Store::release` reads before deleting the last local copy. The asymmetry was the tell: somebody bounded the message that *removes* records and not the one that *adds* them. Both arms are now one arm, so the next verb added here is compared against both. |
| **`a_single_bit_of_difference_fails_the_challenge`** | Corruption is caught, not just deletion. |
| **`an_unbounded_segment_request_is_not_acceptable`** | One request cannot ask a peer to assemble everything it holds. |
| **`a_maximum_size_chunk_fits_in_one_frame`** | The largest legitimate message fits the frame limit, so normal operation does not hit it. |
| `every_request_variant_round_trips_through_the_wire` | A variant that fails to encode is a runtime failure on a live connection. |
| `every_response_variant_round_trips_through_the_wire` | The same, for responses. |
| `a_hello_is_accepted_from_the_floor_upwards_and_refused_below_it` | Version negotiation is a window, not a point: anything at or above the floor is answered with what both sides know. |
| **`red_team_every_variant_keeps_its_number_on_the_wire`** | postcard writes a variant as its position. Version 4 inserted `Response::ChunkSummary` before `WantHosted` and `Refused`, so a version-2 peer's `WantHosted` decoded as a `ChunkSummary` — "Hit the end of buffer" on every round of the fleet for a week — and its `Refused` as a `WantHosted`. Round trips cannot see this, since both ends compile the same enum; this pins the deployed numbers. Append, never insert. |
| **`red_team_a_peer_from_before_the_current_wire_order_is_refused_at_hello`** | Versions 2 and 3 share the old order, so the floor is 4: such a peer is refused at the hello with a line naming both versions, instead of every later answer decoding as the wrong message. |
| `a_refusal_carries_no_secret_material` | Documents that `Refused` is operator-facing only. |

## `service` — what a peer may obtain (24)

| Test | What it proves |
| --- | --- |
| **`what_a_peer_fetches_is_useless_without_the_key`** | The reason there is no access-control list. The served bytes contain no plaintext, and a stranger's keys cannot open them. |
| **`a_node_stores_and_serves_a_strangers_chunk_without_reading_it`** | The mutual-storage bargain in one test: the host serves back exactly what it took, cannot open it, and the guest can. |
| **`a_host_that_discarded_a_chunk_cannot_fake_the_proof`** | Deleting to save space is detected. |
| **`storing_beyond_the_pledge_is_refused`** | Otherwise "pledge 10 GB" is meaningless and the disk fills. |
| **`red_team_concurrent_stores_cannot_take_a_host_past_its_pledge`** | The listener serves connections concurrently since 2026-09-17, and the pledge check is a read before a write. Sixteen offers released at the same instant all saw the same free space and all were stored: a host took on four times its pledge. The check and the write now happen under one lock. Fails when the lock is removed. |
| **`a_bad_request_never_becomes_a_local_error`** | A peer must not be able to decide when this node reports a fault. |
| **`a_forged_segment_is_refused_rather_than_stored`** | Signature checking is wired into the service, not merely available. |
| **`an_unknown_chunk_is_none_rather_than_an_error`** | "I do not have it" is ordinary. |
| `a_storage_challenge_passes_when_held_and_fails_when_not` | Both directions. |
| `a_node_that_pledged_nothing_still_serves_its_own_data` | Hosting nothing must not break syncing your own devices. |
| `heads_for_an_unknown_owner_are_empty_rather_than_an_error` | No invented chains. |
| `hello_reports_this_nodes_device_and_agrees_on_a_version` | The opening exchange. |
| **`red_team_a_peer_can_only_withdraw_records_about_itself`** | `Dropped` corrects a ledger, which is the shape of request that becomes an attack if the subject is taken from the message. A host able to say "device B no longer holds these" could make an owner believe its data is unreplicated, or erase the record of the only holder that still has it. The subject is the connection's proven device and cannot be named in the request at all. |
| **`a_segment_already_held_is_answered_not_stored`** | `accepted` has to be what happened, not what the call returned. Mapping `Ok(_)` to accepted made every idle round look like a round that had moved something, so the daemon printed a line every five minutes on a quiet fleet — which is how an operator learns to stop reading the log. |
| **`a_hello_from_a_newer_peer_is_answered_with_the_version_both_sides_know`** | The version window. Requiring an exact match — which is what this did — meant no node could speak to a node one commit ahead, so every protocol addition partitioned the network until every machine upgraded at the same instant. Survivable in one household; impossible for people who join and leave. |
| `a_hello_from_below_the_floor_is_refused_rather_than_guessed_at` | A window has a bottom. Below it there is no shared vocabulary, and pretending otherwise fails on some later message instead of this one. |
| **`red_team_a_leaving_notice_withdraws_only_the_caller`** | `Leaving` withdraws every record about a device at once, the most destructive notice a peer can send: pointed at the wrong device it would make an owner forget the only holder that still has its data. It carries no device, so the subject is the connection's proven one. Also checks the chunk is then under-replicated, which is what makes the next round repair it instead of waiting the seven days of `LIVE_FOR`. |
| `a_drop_notice_for_another_account_is_refused` | A node hosting somebody else's sealed data keeps no ledger about it. Accepting silently would look like the record had been withdrawn somewhere. |
| `a_peer_can_fetch_this_nodes_own_segments_and_chunks` | The basic serving path. |
| `the_segment_limit_is_clamped_to_the_protocol_maximum` | Limits are applied. |
| `a_node_that_keeps_no_book_says_so` | §8 0o 2b.3 (c): a `PeerService` built without `with_relay` answers `Request::Presences` with `NO_BOOK`, which `PeerClient::presences` reads as "cannot tell", not as an error. |
| **`red_team_presences_are_answered_to_the_device_tls_proved_and_no_other`** | `Request::Presences` carries nothing: who may ask is the device the connection proved, handed to the book as `caller`. The member gets its rows; any other device gets `NOT_YOURS`. Sabotaged by asking the book about a fixed device instead of the caller. |
| **`red_team_a_host_that_bounds_accounts_asks_before_every_store`** | §8 1c: with `with_owners`, both `StoreChunk` and `StoreSegment` ask the bound, a claim is credited to the device TLS proved, never another, and a host answers it with its own claim (`claiming`) so a machine that is never dialled can bound what it pulls. Sabotaged four ways: skip the gate on chunks, on segments, credit a fixed device, answer without the host's claim. |
| `an_answer_never_exceeds_what_the_receiver_accepts` | The client refuses a padded answer whole, so the service trims its own to `MAX_RELAYED_ROWS` rows of at most `MAX_RELAYED_ROW_BYTES`: a book past the bounds costs rows, never the answer. |

## `transport` — binding and serving (2)

| Test | What it proves |
| --- | --- |
| `binding_a_public_address_no_longer_needs_an_override` | Documents a deliberate *removal*. Binding a public address used to be refused, because the transport leaked chunk identifiers and sizes to anyone on the path. TLS closed that, so the refusal became cargo cult and was deleted. The test exists so that nobody restores the refusal believing it was ever a security control. |
| `loopback_still_binds` | The ordinary case did not regress while the above changed. |

Authentication is not tested here. It lives one layer down, in `itsanas-tls`,
and is catalogued with that crate.

---

## `session` — what a round establishes, and what one forged chain may not stop (7)

| Test | What it proves |
| --- | --- |
| **`red_team_a_peer_that_only_answered_the_phone_has_earned_nothing`** | See **Red-team tests** above. |
| **`red_team_a_failed_round_earns_nothing`** | See **Red-team tests** above. |
| **`red_team_a_host_serving_a_forged_segment_loses_that_chain_not_the_call`** | The refresh stage of §8 2c, which the honest test server cannot play: a tampered segment in the middle of one device's chain is refused by `put_segment`, the genuine prefix before it is kept, the rest of that chain dropped and the refusal reported -- not an error that ends the pull for every other device's chain. The strict `refresh` other callers use still errors. Sabotaged (the tolerated refusal turned off): red. |
| `a_peer_that_already_held_our_data_has_earned_it` | The steady state of a host that has been storing for weeks: nothing to send, nothing to fetch, and still the most valuable peer this node knows. Requiring fresh transfer would demote every long-standing host to stranger the moment it caught up. |
| `a_peer_that_accepted_our_data_has_earned_it` / `a_peer_that_served_us_our_own_work_has_earned_it` | The two ways a peer proves it is real: it stored something, or it gave us something of ours. |

---
| **`red_team_a_chunk_written_after_the_summary_is_not_restamped`** | Found by Rodin on the 4c plan: the verdict "this bucket agrees" comes from a summary taken before the re-stamp reads its rows, so a chunk written between would be recorded as held by a peer that never saw it. `restamp_agreeing` recomputes each bucket's digest from the rows it re-stamps and hands a mismatch back to be listed. Sabotaged by skipping the recomputation. |

# `itsanas-net` — two-node tests (58)

Real stores, real chunking, real sealing, real signatures, real TCP.
`tests/two_nodes.rs`.

| Test | What it proves |
| --- | --- |
| **`a_sync_round_records_which_peer_now_holds_this_nodes_data`** | The replacement for a coordinator-published node set, end to end over a socket. Without it the repair loop has no idea whether a chunk exists anywhere but on this disk, and the honest answer to "is my data safe?" is "no idea". |
| **`a_peer_that_already_had_the_data_is_still_recorded_as_holding_it`** | The property that makes the ledger converge rather than only grow. A device restored from its recovery phrase learns where its data lives by *asking*, instead of re-uploading its whole store to find out — and the answer costs nothing extra, since it is the same round trip that decides what to send. |
| **`a_host_that_refuses_to_store_is_not_recorded_as_holding_anything`** | A node that pledged nothing still answers, because refusing to host does not stop it being a peer. Recording it as a holder would let a node believe its data was replicated onto a machine that declined it — the worst possible error, indistinguishable from safety until the local disk dies. |
| **`red_team_a_host_that_keeps_discarding_stops_getting_free_uploads`** | The attack auditing alone does **not** stop. Accept, delete, wait: the audit catches it every round and the owner re-uploads every round, so the host pays nothing and the owner pays a full upload each time. The more data the owner has, the more it costs them. Detection without memory is not a defence. |
| **`red_team_a_stranger_is_not_told_which_chunks_this_node_has_lost`** | An attack that repair itself introduced. Asking a peer "do you have chunk X?" tells it this node does not. The ids are blinded so nothing about the content leaks — but *which chunks now exist only on hosts* is precisely the list to delete to destroy somebody's data, and the first version asked every peer it connected to, strangers the discovery loop had just dialled included. A peer is now asked only about chunks the ledger already records it as holding, which discloses nothing it did not tell this node itself. |
| **`what_doctor_finds_is_what_repair_fixes_first`** | Two detectors that ignored each other. `doctor` knows every local loss in one pass; the daemon's sampling scan needs fifty-five days to reach a given chunk on a terabyte store. Somebody running `doctor` because a file would not open therefore learned the answer and had no way to act on it. They now share a queue, and a loss `doctor` found is repaired in the next round rather than eventually. |
| **`a_disk_that_quietly_lost_a_block_gets_it_back_from_a_host`** | The half of repair that pushing cannot do. `push` restores *replication* by offering a peer what the peer lacks; it can put nothing back on **this** disk, and a chunk missing here is the one failure the placement ledger was built to survive. A dropped block, an inode lost to a power cut, a partial restore: the file is unreadable, the bytes are on three other machines, and until now nothing reached for them and the only cure was a human running `doctor` and knowing what to do next. |
| **`red_team_one_forged_chain_does_not_stall_the_pull_of_the_others`** | §8 2c, end to end. A host keeps, beside an account's genuine chain, one it made up: a free device key signing a segment under the victim's user id, its body sealed under another account's key. The signature verifies so the vault keeps it; the body does not open, and that used to fail the whole pull -- the genuine file never arrived, on this round or any later one, because the forged segment is replayed from the vault. Now the genuine file is adopted, the round reports `refused_chains: 1`, and does so again on the next round while new honest work still arrives. Sabotaged (the refused chain propagated again; the count reported as 0): red. |
| **`red_team_a_peer_that_trickles_cannot_hold_the_caller_past_its_budget`** | A peer opens a TLS record announcing 16 KiB and sends it one byte a second, so no single read ever reaches the 30-second timeout. Until `PEER_SESSION_BUDGET` (300 s, the daemon's whole conversation with one peer) such a peer held a daemon's round for as long as it kept going: 45 minutes on a laptop on 2026-10-06, with an older-build node behind a VMware adapter, while none of its files left and none of its peers' were written. With a 2 s budget the caller is refused within 10 s. Sabotaged (the budget ignored): still connected after 20 s, red. |
| **`red_team_a_relay_that_serves_noise_is_not_written_down_as_a_holder`** | Same liar, over the real transport. The pull recorded every chunk a peer *answered* as held by it before anything checked the bytes, so the ledger counted a copy that does not exist and repair would ask the liar first; and the file was adopted. Now only chunks on this disk afterwards are recorded, and the file stays absent. |
| **`red_team_a_relay_cannot_poison_a_chunk_on_the_ordinary_pull_path`** | The same attack as the repair one, through the door the repair defence did not cover. `accept_chunk` verifies; a second method wrote peer bytes unverified and argued that a chunk which fails to open is caught later by `read_file`. It is not caught later, and the reasoning against it had already been written fifteen lines away: noise under a real address makes `has_chunk` true, so nothing looks for the real bytes — not the repair scan, which checks presence, and not `doctor`, whose recorded loss the next scan clears because the blob is now there. That path is every chunk of every sync. |
| **`red_team_a_host_cannot_answer_a_repair_request_with_rubbish`** | A host cannot read what it stores, so its one route to destroying data is to wait for a repair request and answer with noise. Written unverified, those bytes would make `has_chunk` true, the scan would stop looking, no other peer would ever be asked, and a **recoverable** loss would become permanent — strictly worse than refusing to answer. The bytes are opened and re-addressed before anything is written, and a host that answers with something else loses that record. |
| **`a_failing_assertion_inside_a_server_scope_fails_rather_than_hangs`** | The harness under every test in this file. A panic used to skip the line that sets the shutdown flag, so `thread::scope` joined an accept loop that never stopped and the suite reported a **hang**. Every red-team test here runs inside `with_server`, so for as long as this was broken, a test that caught an attack reported a timeout — and a timeout is what everybody retries and nobody reads. Found by sabotaging a verification step on purpose and watching the suite hang instead of fail. `itsanas-coord`'s harness has had the guard, and the rationale written above it, since its server was written. |
| **`a_replacement_device_pulls_a_whole_corpus_back_from_a_stranger`** | MVP acceptance test D, the half nobody had checked. Recovery from a passphrase restores the *account* — the user id, the keys, the ability to speak — and says nothing whatever about whether the files come back, which is the only part the user cares about. A machine writes a corpus, edits one file, deletes another, uploads to a host belonging to somebody else, and is destroyed; a replacement built from the same master secret with a **new device id** pulls. Contents byte for byte, the edit rather than the original, and the deletion as a deletion — that last is the one that fails quietly, because a restore which resurrects everything you ever deleted looks exactly like one that worked. It also asserts the restored device knows *where* its data lives, which is what found that a pull recorded no holders at all. |
| **`red_team_a_host_that_keeps_only_what_it_expects_to_be_asked_is_caught`** | The same attack as the unit test above, end to end over a socket, against the exact set the old rule would have named: the host keeps the sixteen lowest chunk ids out of 58 and deletes the rest. Under the old rule it survived every round for ever. |
| **`a_paused_host_that_starts_answering_again_is_sent_data_again`** | The way back, on a store of a hundred chunks rather than one. A paused peer is offered one chunk a round; the first version left the audit to *find* it in the ledger, where it sat as one fresh record among the thousands the peer is paused for, so every question landed on something it had already lost and the sanction never lifted — a ban wearing the words of a suspension. The probe is now written down and is the only thing a paused peer is asked about: accept, answer, cleared, in two rounds. **The earlier version of this test used a 37-byte file** — one chunk, one record, the single case where finding the probe is guaranteed — so it passed while the mechanism it named did not work. |
| **`red_team_a_host_that_threw_the_data_away_stops_counting_as_a_holder`** | The attack that costs nothing: accept everything offered, delete it immediately, keep claiming the space. A node trusting its own ledger would believe its files were on three machines while two held nothing, and find out on the day the third disk died. The audit withdraws the record, the chunk shows as under-replicated, and the same round re-uploads it. |
| **`an_audit_never_asks_about_a_chunk_it_could_not_check`** | Verifying a proof means re-deriving the sealed bytes locally. Challenging on a chunk this device has collected would fail for a reason that is nothing to do with the peer, and would withdraw an honest record. |
| `an_audit_confirms_a_host_that_is_still_holding_the_data` | The ordinary path: evidence becomes proof, for the moment it is asked. |
| **`a_metadata_round_makes_the_file_listable_before_it_is_downloaded`** | The behaviour everyone expects from a phone: everything listed, tap one to download it. Before the catalogue, a metadata round left the file invisible — deferred means no index entry, and a client on a metered connection could show nothing at all. |
| **`a_metadata_round_learns_what_changed_without_downloading_it`** | The other half: nothing is fetched, nothing is half-written, and a later round on an unmetered connection completes it. Writing this test is what found that deferred work was never retried. |
| **`a_delete_racing_an_edit_still_leaves_the_file_listed`** | The listing applies the same asymmetry as the merge engine. A listing that hid a file the engine is about to keep would tell somebody their edit was lost. |
| **`a_file_deleted_elsewhere_is_never_offered_for_download`** | A client that listed a file deleted last week, and fetched it when tapped, would have resurrected it. |
| `a_metadata_round_offers_the_log_but_sends_no_chunks` | The upload direction: a photo taken on mobile data does not upload itself, and the peer still learns it happened. |
| **`two_nodes_sync_a_file_over_a_real_socket`** | The M4 exit criterion. |
| **`red_team_a_trickled_handshake_is_cut_off_by_the_node_listener`** | The deadline is unit-tested in `itsanas-tls`; this proves the node's listener applies it. Going back to plain `accept` left every other test green, which is how the Rodin audit of 2026-09-17 found the gap. Fails when the listener ignores its deadline. |
| **`red_team_a_peer_speaking_protocol_5_is_never_asked_for_presences`** | A machine not yet on protocol 6 does not know `Presences`: asking costs a refusal a round, or a dropped connection if it misparses. `PeerService::speaking_at_most(5)` stands in for it and a counting `Relay` records every question: a v6 service is asked once (the control), a v5 one never, and the client reads its silence as `None`. Sabotaged (the client's `spoken < 6` gate removed): red. |
| **`red_team_connections_that_say_nothing_do_not_stop_a_node_serving_others`** | The listener served one connection at a time, so one silent TCP connection held it for the thirty-second read timeout, and one every thirty seconds made the node undialable for everybody, for free. Harmless on a home network, an off switch on a forwarded port. With three silent connections open, an honest peer must authenticate and get an answer within five seconds. Fails, after thirty seconds, when the listener is made serial again. |
| **`a_device_takes_the_files_it_asked_for_and_none_of_the_others`** | The ordinary case for a phone, not an edge case: a few gigabytes free against an account of hundreds. The device names what it wants and the source declines everything else, so the merge engine treats the rest as it treats a sleeping peer — deferred, nothing half-written, still listed for a client to fetch on demand. This was a byte budget inside the pull, which stopped when the allowance ran out and therefore kept whatever the log replayed first; deciding *which* files is now `itsanas_policy::keeping`, and this is the network half. |
| **`a_second_push_offers_nothing_and_says_so`** | Found by reading three machines' daemon logs after an upgrade: "sent 400 B (0 chunks, 1 segments)" every five minutes on a fleet where nothing was happening. A push offered the whole chain every round whatever the peer held, the vault refused each already-held segment with a chain-break, and `store_segment` maps every refusal to `false` — so the waste was invisible from the pushing side and grows without bound as the chain does. Fails when the resume is removed. |
| **`a_round_that_has_nothing_to_say_says_it_in_one_hash`** | The cost that made a terabyte impossible: a round asked its peer about every chunk it held, every time — a two-thousandth of the account per round, a hundred and forty gigabytes a day at a terabyte, to learn what is almost always "nothing has changed". Asserts the idle round lists **zero** chunks, that a change lists a slice rather than the account, and that the periodic ledger walk still happens — because a round that never touches the ledger lets every record age out of countable in silence, and `release` destroys local data on the strength of them. Fails when the reconciliation is bypassed. |
| **`red_team_a_change_asks_only_about_what_the_peer_has_not_confirmed`** | Listing a named bucket whole cost 2.1 MB per bucket at a terabyte, so one saved photograph cost 2.1 MB a peer and the 100 MB/day budget bought three megabytes of change a day. After a full round, a new file must ask about exactly its own new chunks, though confirmed chunks share their buckets (the fixture asserts they do). Red when the freshness filter is removed, and when a bucket's range starts past its first chunk. |
| **`red_team_a_chunk_the_peer_silently_dropped_is_found_by_the_next_full_walk`** | What narrowing gives up, and its bound: a narrowed round does not ask about a chunk the peer confirmed recently and then threw away in silence; the full walk, due every `REFRESH_AFTER` (inside `LIVE_FOR`), must, and puts it back. Red when the freshness filter reaches the full walk -- the drop would then go unseen by the sweep for good. |
| **`red_team_a_full_peer_is_not_sent_what_it_refused_round_after_round`** | A refusal leaves no holder record, so a host with a smaller pledge was asked about every chunk it lacked and sent each one's bytes, refused, every round -- the unheld remainder of a terabyte every five minutes against a phone. Asserts the round stops at the first refusal, the next round sends nothing and asks only about what the host is recorded as holding, and after `FULL_RETRY` one round probes again. Red when any of the four is removed: the in-round stop, the remembered refusal, the record filter, the retry. |
| **`a_peer_that_never_agrees_still_gets_its_ledger_walked`** | The defect a review found in the first reconciliation, and the one that would have reached a person as "ITSaNAS says my data is nowhere and refuses to free any space". The freshness guard was consulted only in the branch where the two sides *agree* — and a peer whose storage budget is smaller than the account disagrees on every round, by design, so the walk was never due, never performed and never stamped. Fourteen days later `coverage` reports no copies and `release` refuses, on an account where nothing has gone wrong. Fails when the guard goes back inside the arm. |
| **`a_file_this_device_never_downloaded_can_be_fetched_when_it_is_asked_for`** | The capability the storage budget rests on, and which did not exist when the budget shipped: a device lists a file it does not hold, and opening it goes and gets it — that one file, not the account. Without this, `keep` produces files that are visible and unopenable, and a phone client is a browser for things you cannot read. |
| **`a_file_this_device_made_and_released_can_be_fetched_back_from_a_host`** | The worse half of the same defect, and the one that only showed itself once the listing was fixed: opening a released file still answered "no such file" while two hosts held it. `apply_segments` skips a device's own chain, on the reasoning that its own state already reflects it — untrue the moment content can be released. A listed file that cannot be opened looks like corruption; a file that is not listed looks like a device that has not synced. Fails when the replay mode is put back to `OthersOnly`. |
| **`a_release_rests_on_two_real_peers_and_notices_when_one_stops_holding`** | The test this repository did not have, and the reason three defects in the release path were found by hand on a Raspberry Pi and none by 683 tests. Every other release test writes the holder ledger directly — a device that never spoke to anything — and reads it back, which is sound for testing the *choice* and useless for testing the release: a release never fails on the choice, it fails on the provenance of the evidence. Here two real hosts take a copy over two real sockets, the release decides from what those exchanges left behind, one host then throws the chunk away, and the next round has to notice — which no audit could, because a challenge is checked against a local copy this device released. Fails when the ledger sweep is removed. |
| **`the_side_that_dialled_ends_up_hosting_too`** | The reciprocal half, and the test that decides whether somebody behind a router they do not control can take part at all. Only one of the two nodes runs a server, which is the same asymmetry NAT produces. Before `host_for` existed the dialling side could only give its data away; now its vault grows. Fails if the offer is emptied. |
| **`the_owner_learns_who_is_holding_after_a_reciprocal_round`** | Taking the chunks is half of it. An owner that does not record where its copies went cannot audit them and will keep asking somebody to hold what is already held. Measured through `under_replicated`, before and after. |
| **`red_team_a_vault_already_at_its_pledge_takes_nothing_more_on`** | `host_for` reads the vault's running total; if it forgot what is held, a full host would be given a whole pledge of room again every round. A vault holding 1 MiB for a stranger on a 1 MiB pledge takes nothing. Fails when `held_bytes` returns 0. |
| `a_pledge_of_nothing_takes_nothing_on` | Hosting stays opt-in over the new path: a node that offered no space must not have its disk filled by a peer that asked nicely. Fails when the pledge is ignored. |
| **`a_host_stores_a_strangers_data_and_cannot_read_a_byte_of_it`** | Alice's whole corpus pushed to Bob's node, then every byte Bob holds scanned for Alice's canary. |
| **`a_host_relays_one_device_to_another_that_it_never_met`** | The architecture's whole reason for existing, over a socket: the Pi pushes and powers off, the VM pulls the Pi's work from a host it has never met. |
| **`syncing_twice_transfers_nothing_the_second_time`** | Without the have/missing exchange this re-uploads everything every round, which at real sizes saturates the link forever. |
| **`concurrent_edits_on_two_machines_converge_over_a_socket`** | The convergence property, through the real protocol, so a transport bug that lost or reordered work shows up. |
| **`a_peer_cannot_push_a_forged_segment_into_a_host`** | End to end, not just at the service layer. |
| **`a_storage_challenge_works_over_the_wire`** | Including that the owner re-derives the expected bytes rather than keeping a second copy — which is what makes remote audit possible at all. |
| **`a_malformed_request_gets_a_refusal_rather_than_a_dropped_connection`** | A peer cannot kill a sync round by sending something silly. |
| **`a_host_that_has_pledged_nothing_refuses_to_store_but_still_answers`** | Refusing to store does not make a node stop being a peer. |
| **`red_team_a_host_that_refuses_everything_is_not_reported_as_nothing_to_send`** | Found by the acceptance bench, whose E, F and G failed on nodes that had pledged nothing while every round said there was nothing to send. The push report counts refusals apart from "already held" and names the reason — `PledgeFull` for the host's `PLEDGE_EXHAUSTED`, which both sides share as one constant. Fails when the count or the reason is dropped. |
| `a_larger_file_survives_the_wire_byte_for_byte` | Multi-chunk fetch and reassembly. |
| `a_peer_asking_about_an_unknown_user_gets_an_empty_answer` | No invented chains over the wire either. |

---

---
| **`red_team_an_idle_due_walk_lists_nothing_and_keeps_the_records_fresh`** | §8 4c, the number the step exists for: a due walk listed the whole account per peer, 537 MB every 3.5 days at a terabyte. Against an agreeing real host, with every record aged and the walk due, it now asks about zero chunks, re-stamps every one, leaves every record fresh and stamps the walk. Sabotaged by listing everything on a due walk, and by skipping the re-stamp. |
| **`red_team_a_host_that_lost_a_blob_behind_its_index_is_caught_by_the_next_due_walk`** | The downside §10 8 names, closed: a host's blob deleted behind its index, one pass of its disk check, and the owner's due walk lists the bucket that now differs, offers the chunk again and puts it back -- without listing the whole account. Sabotaged by the check keeping the row. |
| **`red_team_a_peer_refused_on_push_cannot_be_hosted_by_being_pulled`** | Found by the redteam agent on §8 1c: the bound sat on `StoreChunk` and `StoreSegment`, and the hosting pull -- the peer naming owner and chunks, this node fetching them -- was checked against this node's pledge only. `host_for_bounded` asks the same bound before each chunk; against a bound that refuses, nothing is taken and the refusal is reported. Sabotaged by skipping it. |

# `itsanas-cli` — crash consistency (1, `#[ignore]`d)

`tests/crash.rs`. Spawns the real binary, kills it mid-write, and checks what
survived. Ignored because every invocation pays a full production Argon2id
derivation and it spawns a dozen; the `slow-tests` CI job runs it.

| Test | What it proves |
| --- | --- |
| **`a_store_killed_mid_write_never_lists_a_file_it_cannot_read`** | MVP acceptance test J for the half a test can reach. A dozen hard kills at measured points inside a real write, each followed by `doctor --deep`, and after every one the store must be readable, undamaged, and need no repair. A file that never appeared is fine; orphaned chunks are fine and expected. A file the store *lists* and cannot read is not. |

Two things this test is careful about, both learned the hard way:

- **It calibrates rather than guesses.** The first version killed at fixed
  millisecond delays, passed, and exercised nothing: every kill landed inside
  the Argon2id derivation that precedes any store access, and zero chunks were
  written in any round. It now times one complete write and kills at fractions
  of it, and an assertion fails the test if no round managed to
  interrupt real work.
- **It does not claim to cover a power cut.** Killing a process discards the
  process, not the kernel's page cache. Verified rather than assumed: the suite
  was re-run with `blob.rs`'s per-chunk `sync_all` removed and passed
  identically, so this cannot distinguish a store that flushes from one that
  does not. `docs/MVP.md` records the ten-second experiment on real hardware
  that would.

---

# `itsanas-cli` — the daemon loop, driven (4, `#[ignore]`d)

`tests/steering.rs`. The only tests that run `sync_loop`: each starts the real
daemon on a throwaway node and reads the snapshot it writes every time round
its loop -- the stamp says a loop went by, `files` what the store holds. One
test per way a paused node could still move a file. Ignored because `init`, a
`login` and the daemon each pay a full Argon2id derivation; the `slow-tests`
CI job runs them in release (7 s for the three on the laptop). **That job is
Linux only**, so Windows and macOS rest on one run by hand on Windows
(2026-10-06); HANDOVER §10 item 13 has the matrix to add.

| Test | What it proves |
| --- | --- |
| **`red_team_a_paused_daemon_takes_in_no_file_until_resumed`** | A daemon started paused: a file written into the folder is not taken into the store while it loops (two snapshots three seconds apart both say `files 0`), and is after `itsanas resume`. The unit tests in `control` prove the decisions; this proves the loop obeys them -- the CI reviewer's finding on #243: deleting the loop's guard passed every unit test. Sabotaged (`take_in` called while paused): red, with the two below. |
| **`red_team_a_pause_landing_mid_round_takes_in_no_file`** | A pause asked for while a round is under way. A "peer" the test owns accepts the daemon's connection and says nothing, holding the round open; the pause and a new file arrive; the peer lets go. The folder scans inside and at the end of the round re-read the pause, so the store still holds nothing. Sabotaged (either re-read removed): red. Not covered: `halted` stopping the dials to further peers, which needs a second one. |
| **`red_team_a_paused_daemon_adopts_nothing_its_own_devices_push`** | The paused node's listener keeps serving, by design, so a second device of the account (restored from the 24 words `init` printed) pushes a file into its vault; the store must not adopt it until `resume`. It asserts the push really sent something -- its first run passed on nothing, because a node with no pledge refuses its own account's push. **The test the first version of this guard needed:** a `match` arm that ignored the vault drain's result while the drain ran anyway. Sabotaged (that version put back): red. |
| **`red_team_a_timed_pause_ends_by_itself_without_resume`** | A real daemon paused through a control file with until = now+4 takes in a file written during the pause by itself once the end passes, never before it and without `itsanas resume`, and logs "pause over, syncing resumed"; Sabotaged (until ignored; comparison inverted): red. |

---

# `itsanas-cli` — unit tests (104)

## `bench` — measuring this machine (4)

`itsanas bench` exists because "will this work on a Raspberry Pi" can only be
answered by the person holding one. Its own correctness matters more than most:
a benchmark that measures a broken path produces a confident wrong number.

| Test | What it proves |
| --- | --- |
| **`the_generator_produces_exactly_what_was_asked_for`** | Every throughput figure divides by this. A generator quietly delivering fewer bytes would inflate all of them. |
| **`the_generator_is_deterministic_so_the_check_at_the_end_is_meaningful`** | The round-trip check compares what was read back against a second run of the generator. Non-deterministic and every run fails; constant and the check proves nothing. |
| `a_stage_that_took_no_measurable_time_reports_zero_rather_than_infinity` | Dividing by a zero duration gives `inf`, which formats as a nonsense size and reads as a spectacular result. |
| `durations_are_reported_in_units_a_person_can_act_on` | "15.3 hours" is a decision; "55080 seconds" is arithmetic homework. |

## `discovery` — the daemon's use of local discovery (7)

| Test | What it proves |
| --- | --- |
| **`a_confirmed_device_survives_a_flood_of_strangers`** | The eviction attack at the layer the daemon actually uses. Without confirming a device after a successful authenticated round, anyone on the network can push the Raspberry Pi out of the laptop's table and the two stop finding each other while both believe discovery is working. |
| `a_discovered_device_becomes_something_to_dial` | Discovery produces an address *and* the device to pin, which is what stops an address answering as somebody else being trusted. |
| **`red_team_a_flood_of_authenticating_strangers_cannot_take_over_the_table`** | See **Red-team tests** above. This is the one that found a real bug. |
| **`red_team_dialling_strangers_is_rationed_so_a_flood_cannot_eat_the_interval`** | A flood cannot consume the interval real syncing needs. |
| `a_confirmed_peer_is_still_dialled_every_round_however_many_strangers_arrive` | The ration limits strangers, never work: three real machines still sync every round on a noisy network. |
| `the_neighbourhood_is_empty_until_something_is_heard` | No invented peers. |
| `the_poll_is_short_enough_that_shutdown_feels_immediate` | A Ctrl-C must not wait out an announce interval. |

## `control` — pause (open-ended or timed), resume, sync now, how often (12)

What a tray, a wizard or a terminal asks of the running daemon, through a file
in the home (`crates/itsanas-cli/src/control.rs`). The daemon holds the store's
lock, so nothing else can open it; the file needs no socket and behaves the same
on every platform. The loop's dispatch on these decisions has no test of its
own here; `tests/steering.rs` drives the loop for the pause (above). Run end
to end by hand on 2026-10-06 too: `sync-now` refused while paused, `interval 5s`
refused, `1m` taken.

| Test | What it proves |
|---|---|
| `the_file_round_trips_and_ignores_keys_it_does_not_know` | What is written reads back; a key from a newer tray does not stop an older daemon reading the rest; `paused yes` is refused rather than read as "not paused". |
| `writing_replaces_the_file_whole_and_a_missing_file_asks_nothing` | No file is "nothing asked"; a write replaces the whole file through a temporary name, and leaves no temporary behind. |
| **`red_team_a_control_file_cannot_make_the_daemon_dial_in_a_tight_loop`** | `interval 0`, `1` or `29` -- typed, or a tray's bug -- is held at the 30-second floor, so two seconds after a round nothing is due; a huge one is held at a day. Without the floor, a daemon dials every machine of the account every two seconds. Sabotaged (no clamp): red. |
| **`red_team_one_sync_now_is_one_round_never_a_loop`** | One "sync now" is one round: the same stamp read again is not a second, a stamp already in the file when the daemon starts is not replayed, and a stamp from a clock that went back still counts. Sabotaged (any stamp is a request): red. |
| **`red_team_a_paused_node_never_runs_a_full_round`** | Paused, a due round, a daemon started paused and a `sync-now` written anyway all give `Publish` -- say where this machine is, move no file -- never `Round`; resuming gives `Round`. Sabotaged (paused runs a round): red. |
| **`red_team_an_unreadable_control_file_never_resumes_a_paused_node`** | A file that cannot be read keeps the last state understood: the default would be "not paused", resuming a node paused on purpose (a metered link) because a backup tool held the file. Said once, not every two seconds. Sabotaged (fall back to the default): red. |
| **`red_team_a_control_file_that_grew_still_says_paused`** | The daemon reads the file every two seconds, so one that grew by accident (a log redirected into it) is read only up to 4096 bytes -- the whole lines that fit -- and still says the `paused 1` written first. The first version refused such a file, which every reader then had to guess about, and three guessed "not paused". A multi-byte character split by the limit changes nothing; exactly 4096 bytes is read whole; UTF-16 (PowerShell 5's `Out-File`) is refused rather than read as nothing asked. Sabotaged (refuse past the limit; skip the NUL check): red. |
| **`red_team_an_unreadable_control_file_at_start_never_syncs`** | At start there is no earlier state to keep: a file that exists and cannot be read starts the daemon paused -- it most likely holds a pause -- rather than syncing over it. Once the file can be read, it decides. Sabotaged (start from "not paused"): red. |
| `a_person_types_durations_and_learns_the_bounds` | `10m`, `1h`, `90`, `1d`, `auto` parse; `5s` and `2d` are refused with the bounds named rather than clamped to a number nobody typed. |
| **`red_team_a_timed_pause_that_has_expired_syncs_by_itself`** | A pause whose end has passed stops holding by itself: the log says "pause over, syncing resumed" and a round is due at once, with no resume and no wait for the interval; Sabotaged (until ignored; comparison inverted; no immediate round when the pause ends): red. |
| **`red_team_a_timed_pause_holds_until_its_end`** | A paused node whose until is in the future does not sync; until = u64::MAX, typed or from a corrupted file, survives the file round-trip and is a pause that lasts rather than an overflow or panic; Sabotaged (until ignored; comparison inverted; now+1 arithmetic): red. |
| `a_pause_is_asked_for_in_words_and_said_back_in_dates` | `pause --for` accepts 1m..30d in the parse_every style and refuses anything else with the bounds and the open-ended alternative; the end is said back as a UTC date plus "in N min", and u64::MAX gives no date and no overflow. |

## `setup` — the setup engine: run again, secrets, the service's home (7)

`crates/itsanas-cli/src/setup/` (`mod.rs`, `steps.rs`, `sign.rs`; tests in
`setup/tests.rs`). `itsanas setup` walks Machine, Account, Secret,
Registration, Pledge, Folder, Updates, Connectivity, Service and Verify; each step has a
check, an apply and a one-line remedy, so a second run skips what is done.
`signout` / `signin` live here too. These tests drive the engine with a scripted
person and a fake service manager; no real window or service is ever opened.

| Test | What it proves |
| --- | --- |
| **`red_team_setup_run_again_never_remakes_the_account_or_touches_the_keystore`** | A second setup on a configured node finds every step done, never calls init/login again, and leaves the keystore byte for byte; Sabotaged (Account check skipped): red. |
| **`red_team_no_event_and_no_log_line_carries_a_recovery_word`** | Across two accounts, no progress event and no setup.log line contains a recovery word of its own run; Sabotaged (a word put in an event): red. |
| **`red_team_words_typed_back_wrong_write_no_account`** | Wrong words typed back for the 3 asked positions leave no account written, so nobody leaves with a paper that restores nothing; Sabotaged (engine skips the check / words_match accepts any word): red. |
| **`red_team_joining_from_a_coordinator_refuses_a_passphrase_the_service_file_cannot_hold`** | Joining from a coordinator with the service on refuses, at the Account step and before any network or keystore, a passphrase the service file cannot hold, so no node is left restored with no service; Sabotaged (check removed): red. |
| **`red_team_sign_out_forgets_the_passphrase_and_sign_in_needs_the_right_one`** | Signout deletes the service's passphrase file and keeps the keystore; signin refuses a wrong passphrase and rewrites the file with the right one; Sabotaged (file not deleted / passphrase not checked): red. |
| **`red_team_the_service_is_never_installed_for_a_home_it_would_not_run`** | Setup --home ELSEWHERE refuses to install a service that would run a different node, before installing anything; Sabotaged (home guard removed): red. |
| `the_updates_choice_is_written_and_a_later_run_without_it_keeps_it` | `updates = auto` from setup reaches the node's configuration, a second setup that does not ask leaves it auto (re-running setup never undoes a choice), and the answers file takes `updates = "off"` and refuses a mistyped value instead of defaulting. |

## `update::tests` — the self-update: only a signed, newer, intact release replaces the program (10)

`crates/itsanas-cli/src/update.rs` (tests in `update/tests.rs`). `itsanas
update [--check]` and the daemon's daily look. The tests serve a fake release
from a temporary directory through the `Fetch` trait -- no network -- with a
key generated per test in place of the pinned one, and a temporary file as the
installed program. Every red-team test checks the program is byte for byte what
it was, nothing was set aside, and no download was left beside it. Sabotaged
with `scripts/sabotage.py`: every defence below turned its test red.

| Test | What it proves |
| --- | --- |
| `a_signed_newer_release_replaces_the_program` | The path that must work: release 0.2.0 signed by the key, its binary intact, replaces a 0.1.0 installed from a release. |
| **`red_team_an_update_signed_by_another_key_is_refused`** | A release signed by any other key is "not signed by the ITSaNAS release key" and nothing changes: whoever can upload to the release page cannot push a program. Sabotaged (manifest parsed without its signature): red. |
| **`red_team_a_modified_manifest_with_a_valid_signature_is_refused`** | The real signature file beside a manifest with one line changed (another binary's hashes) is refused: the signature covers the exact bytes read. Sabotaged (as above): red. |
| **`red_team_an_older_release_is_never_installed`** | A correctly signed 0.0.9 offered to 0.1.0 is "up to date", and `install` refuses it on its own as not newer: an old release with a known bug cannot be replayed. Sabotaged twice (check's version floor; install's guard): red both. |
| **`red_team_a_download_whose_hash_differs_is_refused`** | A binary swapped on the server after signing (same size, one bit) is "not the file that was signed". Sabotaged (download not checked): red. |
| **`red_team_a_truncated_update_is_never_put_in_place`** | Half the binary is "incomplete", never put in place: half a program is a machine that no longer starts. Sabotaged (as above): red. |
| **`red_team_a_build_not_installed_from_a_release_never_updates_itself`** | A program whose bytes are not its own version's released binary (built from source) refuses to update; one running under a cargo `target/` directory does not even look. Sabotaged (origin check removed; target/ test removed): red both. |
| `without_a_pinned_key_nothing_is_fetched` | With `RELEASE_KEY` still `None`, `check` answers "no key" without one request: a build that can trust nothing does not ask. |
| **`red_team_a_failed_swap_puts_the_old_program_back`** | The replace is two renames; when the second fails the first is undone and the old program is where the service starts it; when the first fails nothing moved. Sabotaged (no rollback): red. |
| `the_daily_look_follows_the_setting` | `off` makes no request and clears the notice; `notify` writes the version `status` shows and installs nothing; `auto` installs and clears the notice. |

## `answers` — the unattended answers file (2)

| Test | What it proves |
| --- | --- |
| `an_answers_file_says_everything_but_the_secrets` | Every non-secret key of an answers file is parsed into Answers, and none of them is a secret. |
| `a_mistyped_answer_is_refused_not_ignored` | An unknown or mistyped key in the answers file is refused, never silently ignored (a typo must not leave a machine offering nothing). |

## `secrets` — the 24 words and the passphrase, never in argv, env or a web page (5)

The native window per platform (WinForms through PowerShell on stdin, osascript,
zenity, kdialog) or the terminal. Only the built command and the parsing are
tested: no window was opened by a test.

| Test | What it proves |
| --- | --- |
| **`red_team_no_secret_window_carries_a_secret_in_argv_or_env`** | For every backend (PowerShell, osascript, zenity, kdialog) and every ask, the built Command's argv and env hold no recovery word and no passphrase; Sabotaged (phrase put in argv): red. |
| **`red_team_typed_back_words_are_checked_not_waved_through`** | Words_match accepts the right words at the asked positions (case and spaces tolerated) and refuses wrong, missing or swapped ones; Sabotaged (any non-empty word accepted): red. |
| `positions_are_three_distinct_words_of_the_twenty_four` | The confirm step asks 3 distinct positions within 1..=24, in order. |
| `the_windows_reply_survives_any_code_page` | The window's reply comes back base64-encoded, so a non-ASCII passphrase is not mangled by the console code page. |
| `the_secret_window_is_the_platforms_own_or_the_terminal` | The backend choice is the platform's own window when there is a desktop and the terminal otherwise (an SSH session included), and gives a clear error naming what to install when there is neither. |

## `service` — the background service under the installers' names (6)

| Test | What it proves |
| --- | --- |
| `every_name_matches_what_the_installers_and_clean_scripts_use` | Task, unit, plist, passphrase-file and tray-autostart names, with and without an instance, match provision.ps1/provision.sh/macos.sh and clean.ps1/clean.sh. |
| `names_and_paths_are_quoted_where_they_land` | Instance names and paths are quoted in the generated task script, unit and plist, and none of them contains the passphrase. |
| `the_passphrase_file_reads_back_in_both_forms` | A passphrase file written by setup or by the installers reads back as the same passphrase. |
| **`red_team_a_windows_passphrase_starting_with_a_hash_reads_back`** | A bare Windows passphrase such as `#Horse-Battery-9` is not taken for linux.sh's commented placeholder, and is written and read back, so setup does not stop at the Secret step for ever; Sabotaged (any leading # read as the placeholder): red. |
| `the_passphrase_file_is_written_whole_and_alone` | The service's passphrase file is written in full, with owner-only permissions, and holds nothing else. |
| `the_windows_scripts_parse` | (Windows) The generated wrapper, task, tray and ACL scripts, the secret window and the tray icon script all pass PowerShell's Parser::ParseFile, with and without an instance. |

## `verify` — the final check (2)

| Test | What it proves |
| --- | --- |
| `an_unreachable_laptop_is_not_a_failure_an_unreachable_forward_is` | The dial-back verdict passes a machine behind NAT that announced nothing, and fails one whose announced forward cannot be reached, giving a remedy. |
| `the_snapshot_count_is_read_as_the_daemon_writes_it` | The canary check parses the daemon's snapshot file count in the format the daemon writes; otherwise every setup would fail its last check. |

## `web` — the local setup and Settings page (14)

`crates/itsanas-cli/src/setup/web/` (tests in `web/tests.rs`). `itsanas setup`
on a desktop and `itsanas settings` serve a page on 127.0.0.1 with a random
port and a 128-bit token, std::net only. The tests run a real server
in-process with stand-in secret windows; no real browser is launched.

| Test | What it proves |
| --- | --- |
| **`red_team_no_response_ever_carries_a_recovery_word_or_the_passphrase`** | Two whole setups driven through the page's HTTP API with a scripted person in place of the window; every byte that came back (static files, plan, every state poll, run reply) holds neither passphrase nor the 24-word phrase, nor any word of it the other run did not also say; Sabotaged (engine emits the phrase in an event, so in the state JSON): red. |
| **`red_team_a_rebinding_host_with_the_right_token_is_refused`** | GET /api/state and POST /api/quit under Host evil.example:PORT with the valid token are 403, and the server keeps serving; Sabotaged (Host check dropped): red. |
| `a_foreign_host_is_refused_before_anything_is_served` | A rebinding name, another port, no port, a localhost-prefixed name, no Host and two Host headers all get 403 for the page itself; 127.0.0.1:PORT and localhost:PORT get 200; Sabotaged (Host check dropped): red. |
| `the_api_refuses_a_missing_or_wrong_token` | /api/state, /api/plan and /api/run with no token, a token one digit off, or the right token beside a wrong one are 403; the right token is 200; Sabotaged (token check forced true): red. |
| `a_cross_origin_request_is_refused_and_no_cors_header_is_sent` | A POST with a valid token from Origin evil.example, null, another local port, or Sec-Fetch-Site cross-site is 403; the page's own origin is answered with no Access-Control-* header; Sabotaged (Origin check skipped): red. |
| `oversized_requests_are_refused_without_being_read_whole` | A head one byte over 16 KiB that never ends gets 431, and a body announced at 64 KiB+1 or 10 MB that is never sent gets 413, both before the read timeout, so neither is waited for or read whole; Sabotaged (head limit raised; body limit removed): red. |
| **`red_team_connections_dripping_bytes_cannot_starve_the_page`** | Sixteen connections each sending one byte every 2 s (under the per-read timeout) are dropped at the 10 s connection deadline, and the person's next API call is answered; Sabotaged (per-read timeout only): red. |
| `every_response_says_no_store_and_forbids_framing` | The 200 page/js/css, 404, 403 (Host), 403 (token), 200 API and 431 answers all carry no-store, a CSP with script-src 'self' and frame-ancestors 'none', X-Frame-Options DENY, nosniff, no-referrer and Connection: close; Sabotaged (Cache-Control max-age=60): red. |
| `while_a_window_is_open_the_page_is_told_so_and_a_closed_window_says_what_to_do` | While a stand-in window blocks, /api/state says running and waiting on the 24 words, and a second /api/run is 409; once the window closes, the state says failed with a remedy, waiting is cleared and no keystore was written; Sabotaged (waiting never set): red. |
| `settings_steer_through_the_control_file_and_change_the_pledge_through_the_engine` | On a node set up through the page: the setup page refuses Settings actions (404); Settings pause writes the control file, a pause 'for 1h' writes a pause with an end (`until`), resume clears it, a pledge change through /api/run reaches the node's config, and sign out deletes the passphrase file; Sabotaged (sign out a no-op): red. |
| `the_page_asks_no_secret_loads_nothing_from_elsewhere_and_sends_its_key_in_a_header` | Index.html has no password field or textarea and keeps the never-type and window-opened sentences; no embedded file names an http(s) address or a CDN; app.js sends the X-Itsanas-Token header, clears the address with history.replaceState, and uses neither innerHTML nor eval. |
| `a_desktop_is_needed_for_the_page_and_ssh_never_counts_as_one` | Has_desktop: Windows and macOS yes, Linux only with a display, any SSH session no (X forwarding included), so setup falls back to the terminal where a browser would open unseen. |
| `the_browser_is_opened_by_the_systems_own_program_with_the_url_as_one_argument` | Browser_command builds rundll32 url.dll,FileProtocolHandler / open / xdg-open with a URL containing # and & as one whole argument, never through cmd. |
| `the_suggested_pledge_is_a_fifth_in_whole_gib_and_capped` | Suggest_pledge offers a fifth of the free disk, rounded down to whole GiB, 0 on a nearly full disk, at most 500 GiB, in integers. |

## `daemon` — pacing (3)

| Test | What it proves |
| --- | --- |
| `the_default_interval_is_neither_a_busy_loop_nor_an_hour` | Too short and three machines polling each other is a constant load on a Pi; too long and the thing feels broken. |
| `shutdown_is_noticed_quickly_enough_to_feel_immediate` | A Ctrl-C that took a whole interval to be noticed would be indistinguishable from a hang. |

The daemon's real behaviour — that two nodes converge with nobody running
`sync` — is verified by running it, not by a unit test. The loop itself is
twenty lines around `session::round`, which the two-node suite covers
thoroughly; a test with a fake clock around it would assert that the loop calls
the function, which is not a property worth having a test for.

## `main` — leaving quietly, saying how old an answer is and without a passphrase, naming a device, choosing a port, listing, migrating and requiring the instances, staying departed, what a sync brings, the phrase as printed, the tray's one word and a pause that never hides a hung daemon, a pledge that keeps the split and a refused chain said, a departed node not registering, a node under the other home variable, a timed pause shown only while it holds (32)

`itsanas status | head -20` printed twenty lines and then a Rust panic and a
note about `RUST_BACKTRACE`. Rust disables SIGPIPE at startup, so `println!`
panics where every other command-line program simply ends. It was found in the
output of `install/provision.sh`, which pipes `status` into `head` itself.

| Test | What it proves |
| --- | --- |
| **`a_panic_that_is_not_a_closed_pipe_is_never_swallowed`** | The dangerous half of the fix. A hook that exits 0 on the wrong panic turns a crash into a silent success, which is worse than the noise it removed. Five real panic messages must still reach the reader. |
| **`a_prefix_that_names_no_device_is_refused_rather_than_invented`** | `itsanas device forget` accepts the short device id the logs print. A revocation is a signed record the coordinator files and honours, so one written against an identifier nobody holds would be silent, permanent and impossible to notice. Five strings that name no device must all be refused. |
| `the_short_form_the_logs_print_is_enough_to_name_a_device` | The other half: a unique prefix resolves, and one shared by two devices refuses rather than picking one. |
| **`an_age_never_reads_as_fresher_than_it_is`** | With the daemon running, `itsanas status` prints a snapshot rather than refusing, and the header says how old it is. That number decides whether the reader trusts what follows, so every boundary rounds *down* -- towards admitting the snapshot is older. The case that matters is the last one: a daemon that died three days ago must not leave something that reads as current. |
| **`a_port_another_node_on_this_machine_is_configured_for_is_not_chosen`** | Two accounts on one machine are two daemons. Every node used to be created on 9797, so the second daemon could not bind. The case the kernel cannot see is the one tested: the first account's daemon is stopped, 9797 binds, and handing it out puts two daemons on one port at the next boot. |
| `a_port_something_already_holds_is_skipped_and_exhaustion_says_so` | A port nothing can bind is never offered, and running out of the hundred-port range returns nothing rather than a port that fails later. |
| **`the_ports_of_the_other_nodes_beside_this_one_are_found_and_its_own_is_not`** | A sibling node is a directory holding a keystore. A directory without one does not count, and a node's own configuration must not count against it. |
| **`red_team_a_stopped_node_is_never_reported_as_a_running_one`** | `status` prints the snapshot in two different situations — the daemon is holding the store, or nothing is running and this is what a stopped node last said, possibly last week. One sentence for both would make "this node is running" a claim the command cannot support, and that sentence is what a reader uses to decide whether to trust the numbers under it. |
| **`red_team_a_running_node_is_reported_with_its_age_and_no_passphrase`** | The other half of `an_age_never_reads_as_fresher_than_it_is`: that one checks the arithmetic, this one checks the arm is reachable at all. `snapshot_status` takes a path and nothing else, so it *cannot* prompt -- the guarantee is structural rather than a promise. A regression here is a node whose health is unreadable without the passphrase. |
| **`red_team_a_silent_daemon_is_stale_never_healthy`** | `itsanas status --brief` is what the Windows tray draws: `healthy` only while a daemon holds the store and its snapshot (now stamped with its interval) is within two intervals; three intervals old is `stale`, no daemon `stopped`, after `leave` `departed`. A green icon over a hung daemon is the failure a tray exists to prevent (HANDOVER §8 f). Sabotaged (age check off): red. |
| **`red_team_a_setting_never_erases_a_pause_it_cannot_read`** | Over a control file that cannot be read, `interval` and `sync-now` refuse and leave it as it is: starting from the default would write "not paused" over the pause it holds (a tray's "Sync every" resuming a node paused on a metered link). `status --brief` says `unknown`, not `healthy`. `resume` rewrites it. Sabotaged (any command rewrites; `healthy`): red. |
| `a_paused_node_says_paused_and_a_hung_one_still_says_stale` | `itsanas pause` turns the tray's word to `paused`; a paused daemon silent for three intervals is still `stale`, or a pause would hide a dead daemon behind a calm icon. `resume` asks for a round at once, and `sync-now` with no daemon running refuses rather than saying it was asked. |
| `a_snapshot_without_a_stamp_is_printed_but_not_dated` | A snapshot written by an older version has no time on its first line. Printing it is right; inventing an age for it is not, because the age is the only thing telling a reader whether to trust the numbers under it. |
| `a_node_that_has_never_synced_says_so_rather_than_printing_nothing` | A node whose daemon has not finished a round yet has no snapshot. Succeeding with empty output would read as a healthy node with nothing to report, which is the opposite of the truth. |
| `the_ports_a_node_had_to_skip_are_all_named_not_just_the_first` | The second account on a machine skipped 9797 *and* 9798 and was told only that "9797 is used by another node" — singular, naming one of two. Ports here are handed out without asking, so this line is the only place somebody learns what happened, and counting instances from it counted wrong. |
| **`a_taken_listen_port_is_answered_with_a_free_one_and_the_commands_to_move`** | Nodes created before `init` chose ports all sit on 9797, and the second one's daemon exited with "address in use" — under systemd, every thirty seconds. The error now names a free port and `itsanas listen` / `register`, and offers no port when none is free. |
| **`a_refusal_is_reported_once_and_then_only_after_a_quiet_period`** | A host with pledge 0 refuses every round. The line that ended the silent `sent 0 B` must not become one line every five minutes per peer, for ever: reported at once, then at most once per `OUTAGE_QUIET`, and again at once after a round with no refusal. The acceptance bench checks that `sync` against a pledge-0 host prints the reason. |
| `the_message_std_prints_when_a_pipe_closes_is_recognised` | The message copied from the Pi, and its Windows spelling, are both matched — only on the prefix, because the tail belongs to the platform. |
| **`red_team_instances_lists_only_homes_that_hold_a_node`** | `itsanas instances` scans `~/.itsanas` and `~/.itsanas-*`. `~/.itsanas-passphrase` is the default node's passphrase *file* and an emptied home is no node; listing either sends somebody to `--instance passphrase` or to a node that is gone. Only directories holding `keystore.bin` are listed. |
| **`red_team_migration_names_the_node_after_its_account_and_keeps_its_data`** | `itsanas migrate` renames `~/.itsanas` to `~/.itsanas-<account>`. The migrated home must open with the same passphrase and read back a file stored before, and nothing may be left at `~/.itsanas`: a migration that made the new directory and copied only the keystore and config opens fine and has lost every file (sabotaged that way, red). |
| **`red_team_migration_never_lands_on_an_existing_home`** | A `~/.itsanas-<account>` that already exists is somebody's node: the migration refuses and both homes stay as they were. |
| **`red_team_after_migration_a_command_without_a_name_refuses`** | With neither `--instance` nor `--home`, a machine holding only named instances refuses and names them, instead of falling back to `~/.itsanas` where `init` would make a second identity and a stale `itsanas.service` would start it. A machine with no node at all still gets `~/.itsanas`, or the first `init` could never run. Sabotaged (always fall back): red. |
| **`red_team_a_departed_node_stays_departed_until_it_rejoins`** | After `itsanas leave` the home holds a `departed` file; the daemon exits 0 at once (so `Restart=on-failure` and the logon task do not loop), `serve` and `sync` refuse, each naming `itsanas rejoin`, and `rejoin` undoes it. Otherwise systemd would bring back a machine its peers were told had gone. Sabotaged (the check reads another file): red. |
| **`red_team_a_departed_node_cannot_register_again`** | `register` publishes this device's address as well as its claim; after `itsanas leave` it ran unguarded, so a departed node could hand the peers that were told it had gone a fresh address. It now refuses first, naming `itsanas rejoin`. Sabotaged (guard removed): red. |
| **`red_team_a_node_under_the_other_home_variable_is_not_shadowed`** | On Windows the CLI prefers `USERPROFILE`; a node made when `HOME` won is invisible there, and `init`/`login` would mint a second identity. A derived home with no node, when the other variable's home holds one at the same place, is refused with the path and `--home` named; an explicit `--home` is never second-guessed. Sabotaged (`stranded_node` never finding): red. |
| `migration_advice_says_the_old_unit_restart_loops` | After `migrate` the old unit still points at `~/.itsanas`, fails at each start and is restarted by `Restart=` or the logon task until disabled; the advice says so. Sabotaged (sentence softened): red. |
| **`red_team_what_a_sync_pulled_lands_in_the_folder`** | `itsanas sync` by hand fetched into the store and left the synced folder empty until an `itsanas scan` no guide names (found by a persona run of FIRST-STEPS, HANDOVER §8 q). A file put in the store as a pull would must be written out by `sync_folder`, which `sync` runs after its rounds. Sabotaged (it returns at once): red. |
| `the_phrase_as_init_prints_it_reads_back_as_the_words` | `init` prints the words as a numbered grid; a person pastes that into `--phrase-file`. The numbers are dropped, words are never guessed at, and one word a line works too. |
| **`red_team_an_empty_mount_point_is_not_a_reachable_folder`** | An unmounted disk leaves an empty mount point; `instances` must say UNREACHABLE unless 0l's `.itsanas-folder` marker is there, never trust `is_dir` alone. |
| `a_refused_chain_is_said_on_the_sync_line` | `itsanas sync`'s summary names a device chain the peer served and this node refused (§8 2c), and adds nothing when there is none; without it the round reads as a finished sync with one device's changes left out. Sabotaged (the line dropped from `deferred_note`): red. |
| **`red_team_pledge_under_what_keep_needs_is_refused_and_saves_nothing`** | `itsanas pledge 1M` on a node keeping 20 GiB is refused with the `space --pledge .. --keep .. --apply` command, and neither the node file nor the node in memory changes; with no keep the same pledge goes through, so the setter is not one that refuses everything (HANDOVER §8 3a). Sabotaged (the `check_split` call in `set_pledge` dropped, or moved below the assignment; `check_split` always `Ok`): red. |
| `a_timed_pause_shows_paused_only_while_it_holds` | Status --brief, which every tray reads, says paused only while the pause holds, so an expired timed pause never shows a blue icon over a node that is syncing again. |

# `itsanas-policy` — when to sync, and how much (23)

`src/lib.rs`. A decision table with an argument attached to every row, and no
dependency on anything — so the phone, the Mac shell and `itsanas daemon` reach
the same schedule instead of each keeping its own number. The daemon is what
uses it today: `itsanas daemon` prints the interval, the scope and the reason.

## `plan` — when to sync (15)

| Test | What it proves |
| --- | --- |
| **`every_combination_produces_a_plan_with_a_reason`** | Totality. A sync tool that silently does nothing in some unconsidered corner is the failure this crate exists to prevent, and "silently" is the operative word: every state has to be explainable to the person looking at it. It walks `Network::ALL`, `Power::ALL` and `Attention::ALL` rather than lists written out here, because the list written out here is the one somebody forgets — it went on checking two `Attention` variants after a third was added, and passed. |
| **`a_service_on_ethernet_does_not_inherit_a_phone_s_interval`** | The Pi in the cupboard is not a backgrounded app. Two hours is not a considered choice about ethernet; it is the smallest number that survives Android Doze, and applying it to a permanently-powered machine would make an edit take up to two hours to cross a household through a node that was awake the whole time. |
| **`a_service_on_a_metered_link_is_no_less_careful_than_a_phone`** | Being a service buys freedom from the *platform*, not from the data plan. A laptop tethered to a phone must not start uploading forty gigabytes because it is technically a daemon. |
| **`a_service_still_stops_when_the_battery_is_nearly_gone`** | Nothing about being a service makes the battery bigger. |
| **`switching_background_syncing_off_does_not_stop_a_daemon_somebody_started`** | That switch means "do not work unless I am looking at the app". Starting a daemon *is* the deliberate act it exists to require. A daemon silently doing nothing because of a phone setting is a support case nobody could diagnose. |
| **`nothing_is_moved_over_a_metered_connection_unless_it_was_asked_for`** | The row that decides whether somebody trusts this on their phone. A tool that silently spent a data allowance would be uninstalled once and remembered for years. |
| **`the_file_list_still_arrives_on_a_metered_connection`** | The other half: knowing *what* changed is kilobytes and always happens. Metadata and content are separate purchases. |
| `allowing_metered_downloads_actually_allows_them` | Somebody with an unlimited plan who says so is believed. |
| **`a_low_battery_stops_background_work_but_never_stops_a_person`** | Refusing to work while somebody is watching is how a tool gets a reputation for being broken. They can see the battery indicator themselves. |
| **`the_button_works_even_when_the_schedule_would_not`** | A button that does nothing teaches people the application is broken. Neither a low battery nor a metered connection overrides a deliberate act; only having no network does. |
| `an_open_application_on_free_wifi_syncs_almost_live` | The case everybody judges the product on. |
| `no_network_means_no_plan_and_no_button` | The one state where there is nothing to honour. |
| `switching_background_syncing_off_leaves_the_foreground_alone` | The setting is about the background, and only the background. |
| **`background_intervals_sit_above_every_platform_floor`** | Every mobile platform imposes a fifteen-minute floor on periodic background work. An interval below it is not a schedule, it is a number the operating system ignores. |
| **`a_day_of_metered_checking_is_not_measurable_on_a_data_plan`** | The arithmetic behind the once-a-day metadata round, so the claim in the module documentation is checked rather than asserted. |

## `keeping` — what a device holds when it cannot hold everything (8)

`src/keeping.rs`. A budget bounds the *quantity*; this decides the *choice*.
Pure, so the reasoning can be argued with in a test instead of observed on a
phone, and deterministic, so two rounds never disagree and spend a data plan
swapping the same two files back and forth.

| Test | What it proves |
| --- | --- |
| **`the_budget_keeps_what_was_asked_for_not_what_arrived_first`** | The defect the module exists for. The listing is deliberately given oldest-first, which is the order a log replays in: an implementation that kept whatever arrived first would keep the file from six years ago. That is what shipped before this, and "keep two gigabytes" meant "keep the first two gigabytes the log mentions". |
| **`a_file_too_large_for_the_room_left_does_not_starve_the_rest`** | The boundary that decides whether the setting is usable. A phone whose account starts with a film must still get the documents behind it, and "stop at the first thing that does not fit" is the obvious implementation that would not. |
| **`the_answer_does_not_depend_on_the_order_the_files_were_listed_in`** | Stability is the anti-thrashing property. Every file in the fixture shares a date, because ties on the sort key are where an unstable implementation shows itself — and two devices that disagree about what matters most is the churn this prevents. |
| **`what_is_here_and_not_wanted_is_offered_up_and_what_is_wanted_is_fetched`** | Both directions come from one decision. Computing them separately is how a device comes to release a file it is about to fetch again. |
| `a_filter_matches_a_directory_and_not_a_name_that_merely_starts_the_same` | `Photos` must not match `Photos-old/x`. A filter that silently matches more than it names is how a phone fills with the wrong gigabytes. |
| `no_budget_and_no_filter_keeps_everything` | The laptop case, and the one that must not change. |
| `smallest_first_keeps_the_most_files_and_oldest_first_keeps_the_archive` | Same account, same budget, three orders, three different answers — which is the point. A device that ignored the setting would give the same answer to all three. |
| `an_empty_choice_asks_for_nothing` | No work invented from an empty listing. |

# `itsanas-node` — a node on disk (134)

`src/`. Keystore, configuration, and the one sync round that honours what a
device was told to keep. It lived inside the command-line binary until the
Android shell needed exactly the same things: two implementations of the
passphrase handling is one too many.

## `coordinator` — publishing an address (15)

Found on a real coordinator, on the Freebox VM, the first time a member
registered with one: `itsanas register` printed `announced 0.0.0.0:9797`. That
is the default listen address and the right default — accept from every
interface — but it is not somewhere a peer can dial, and nothing checked. The
comment above the call says a device nobody can reach has not really joined
anything.

| Test | What it proves |
| --- | --- |
| **`red_team_a_refusal_is_no_answer_rather_than_not_enrolled`** | §8 1c (ii), found by the redteam agent: a `Refused` reply to `Standing` (a rate limit, a caller not served) was read as "not enrolled" and ended every fresh confirmation at once. Only a `Standing` reply is about the device; a refusal is no answer. Sabotaged by mapping `Refused` to `Unenrolled`. |
| **`red_team_an_unpinned_coordinator_is_not_asked_about_other_accounts`** | `pinned` is false without the coordinator's device id configured: unpinned, whoever is on the path answers "live" by echoing the presented claim. That `standing` and the asker consult it is proved in `tests/withdrawals.rs`. |
| **`an_unspecified_listen_address_is_not_what_gets_published`** | The address published is the local end of the connection that just reached the coordinator, not `0.0.0.0`. Of this machine's addresses it is the one demonstrably able to talk to the coordinator. Still wrong behind NAT, where only the coordinator can see the address a peer needs; that is a protocol change and is written down in `coordinator.rs`. |
| **`the_published_port_is_the_listening_one_not_the_one_dialled_from`** | The local end carries an *ephemeral* source port. Taking the port along with the address would publish somewhere nothing listens — a failure that arrives later, elsewhere, and looks like a network fault. |
| `an_address_somebody_chose_is_left_alone` | Substitution happens only where the configuration said "anywhere". A specific address or a hostname is a decision, and overruling it would break the setups that were configured deliberately. |
| **`an_address_that_only_its_own_lan_can_dial_is_tried_last`** | The scenario the whole step is for: the laptop is at a friend's house and the coordinator hands it the account's machines, three of them on `192.168.1.x` at home. Dialling those first spends the round's budget and its connection timeouts on addresses that cannot answer — and from a network using the same private range they reach a *stranger's* machine, refused only because the device id is pinned. Addresses reachable from anywhere sort first, by the receiver's own judgement and never by a claim in the presence. |
| **`the_announced_port_is_not_replaced_by_the_listening_one`** | A port forward exists to map an outside port to a different inside one; `ngas.fr:9801 -> 192.168.1.11:9797` is the normal shape. Substituting the listening port would publish an address the router forwards nothing to, and the failure would read as the peer being offline. |
| **`a_configured_announce_is_published_instead_of_the_local_address`** | Without it a node behind a router publishes its address on the LAN it is on, which is precisely what no machine in another house can use. |
| `a_machine_that_moves_still_publishes_where_it_is` | No `announce` is the right configuration for a laptop, and it must still publish something: announcing is also the heartbeat availability is counted from, so a node that stopped would be counted as gone. |
| **`red_team_a_presence_its_device_did_not_sign_is_dropped`** | §8 0o phase 2b.1. A coordinator relays presences and does not make them: an address changed after the Pi signed it, and a presence signed by another key with the Pi's id on top, are both dropped and counted, and the genuine one -- dated 1970, as a Pi with no real-time clock dates it -- is kept. The check is who signed, never when. Sabotaged by keeping everything. |
| **`red_team_a_coordinator_cannot_pass_off_another_accounts_machine_as_yours`** | §8 0o phase 2b.3. The client's check on `ClaimedPeers` (`verified_claimed`): another account's genuine machine, listed under this one, is dropped and counted, and this account's own is kept. Sabotaged by checking only the presence's signature. |
| `the_private_ranges_a_home_actually_uses_are_all_recognised` | RFC1918, loopback, link-local, carrier-grade NAT (which is what a mobile network and an overlay VPN hand out) and IPv6 unique-local and link-local. Publishing any of them tells members elsewhere to dial a machine inside somebody else's network. |

---

## `contact` — when the coordinator is dialled at all (25)

§8 0o phase 2a. A round used to dial the coordinator every time, 288
connections a day per node whatever happened. Now it publishes at start, when
its address changes, and hourly, and reads the account's devices only on the
connection a publication opens. Every instant is this machine's own; nothing
here reads a peer's clock or trusts the coordinator's order.

| Test | What it proves |
| --- | --- |
| **`a_machine_that_never_moves_dials_the_coordinator_once_an_hour`** | The number the step is justified by: 288 rounds of a Pi that never moves make 24 connections, 24 publications and 24 reads -- every read on a publication's connection, none on its own. |
| **`red_team_a_machine_that_found_everybody_by_broadcast_still_publishes`** | The Rodin finding of 2026-09-21 against the first rule ("dial when the round reached nobody"): a Pi that found the VM by broadcast never published, and a laptop elsewhere could not find it. The rule is not given who was reached; publishing at start and hourly are each sabotaged and each turns this red. |
| **`a_machine_that_changes_network_publishes_at_once`** | A laptop whose address changed publishes on the next round, not in an hour; an unchanged address inside the hour costs no connection; losing the route counts as a change. |
| **`a_machine_enrolled_after_this_one_started_is_dialled_within_the_hour`** | Why reads ride on every publication instead of waiting for a listed device to go missing: a machine enrolled after this one started is on no list, so it can never go missing. Eleven rounds make no connection, the hourly one reads, and the new machine is in the book. |
| **`red_team_a_coordinator_cannot_grow_the_address_book_without_bound`** | The coordinator still decides which presences it lists -- an older one unsigned, any of them stale. 65 536 devices in one answer leave `MAX_DEVICES` in the table; a thousand addresses for one device leave `MAX_ADDRESSES`. |
| **`red_team_addresses_that_never_worked_do_not_displace_one_that_did`** | Twenty stale addresses for the Pi, from a coordinator that lies or is only out of date, arrive after its LAN address answered: that address is still dialled first and the rest is capped. Order is this machine's record of success, never the list's. |
| **`red_team_a_coordinator_that_has_signed_cannot_talk_this_node_down_to_an_unsigned_list`** | §8 0o 2b.1, found by Rodin: a coordinator that hangs up on `SignedPeers`, as an older one does, would have the client fall back to `Peers`, whose addresses it can forge -- the signature check made advisory against the one party it checks. Once a coordinator has signed, `Due::accept_unsigned` stays false for every later hour. Sabotaged by leaving it true. |
| **`red_team_a_restart_does_not_reopen_the_downgrade`** | §8 0o 2b.2: 2b.1 remembered that the coordinator signs only in the process, so every daemon start re-opened the fallback to an unsigned list until the first read. A book saved after a signed read, loaded back, gives `accept_unsigned == false` on the first round. Sabotaged by not loading `signs`. |
| **`red_team_an_address_book_edited_to_hold_a_forged_presence_loses_it_on_load`** | `<home>/address-book` is editable by anything running as this user, and its addresses are dialled before the coordinator is asked anything. A file holding one genuine presence and one whose address was changed after signing, marked as having worked, loads the genuine one only and says `1 of 2` were dropped. Sabotaged by skipping `verify_origin` on load. |
| **`the_address_that_worked_is_still_first_after_a_restart`** | What the book is for: the address that answered, recorded in this machine's own unix seconds, is still dialled first after a save and a load; an unchanged book is not rewritten; and an address an older coordinator handed out unsigned is dialled but never written, so no load has to take it on trust. |
| **`red_team_a_clock_back_in_1970_does_not_rank_a_stale_address_first`** | Found by Rodin on 2026-09-29: success times are this machine's unix seconds, and a Pi with no real-time clock reads 1970 until NTP answers. An address that worked at 40 must outrank one that worked at 1.7·10⁹, or every round spends a connect timeout on yesterday's address. `Contact::worked` records a success as later than any already in the book. Sabotaged by recording the clock as read. |
| **`claimed_addresses_are_relayable_across_a_restart_and_nothing_else_is`** | §8 0o 2b.3: `Contact::relayable` returns the presences that came with this account's claim, and still does after a save and a load; an address read without a claim, or unsigned, is dialled and never relayed. Sabotaged by not writing the claim, and by not taking it from the read. |
| **`the_same_address_signed_again_later_keeps_the_later_signature`** | Found by Rodin on 2026-09-29 before 2b.3's relay: the book kept the first signature of an address and never the later ones, so the presence it would relay aged while the machine kept re-publishing, and a receiver could not tell it from a replay. Signatures at 100, 200, 150: 200 is kept. Sabotaged by keeping the first. |
| **`red_team_an_address_book_edited_to_hold_another_accounts_claim_does_not_relay_it`** | `<home>/address-book` is editable by anything running as this user. Another account's genuine claim on a listed machine, planted in the file, is dropped on load (`ClaimedPresence::verify_for` against this node's account) and said in the warning; the address stays dialled, never relayed. Sabotaged by skipping the check on load. |
| **`red_team_the_upgrade_does_not_reopen_the_downgrade`** | The book went from version 1 to 2 (claims). A version-1 file read as empty would forget that the coordinator signs, and the first round after the upgrade would accept an unsigned list. It is read as version 2 with no claims. Sabotaged by not recognising version 1. |
| **`a_damaged_address_book_is_an_empty_one_not_a_failed_start`** | Three bytes of garbage load as an empty book with a warning, and a missing file (a first start) as an empty book with none. The book is a cache the coordinator refills; it never stops a daemon. |
| **`a_device_no_longer_listed_is_forgotten`** | A withdrawn or long-silent device leaves the book at the next read, so a round stops spending connect timeouts on it. |
| **`a_newer_address_relayed_by_a_machine_of_the_account_joins_the_book`** | §8 0o 2b.3 (c), the point of the step: the Pi moved while the coordinator was down, a machine of the account that reached it says where, and `Contact::relayed` keeps the newer address -- behind the one that worked, and relayable onward. |
| **`red_team_a_peer_cannot_hand_out_a_presence_it_forged`** | A relayed row whose address was changed after its device signed it is refused and never reaches the book. Sabotaged by skipping `verify_for` in `relayed`. |
| **`red_team_a_relay_cannot_pass_off_another_accounts_machine_as_ours`** | Rodin's finding of 2026-09-29, closed on the relay: a listed device's presence, newer on every date, claimed by *another* account, is refused -- only the owner's signature can tell. Sabotaged by skipping `verify_for`. |
| **`red_team_a_relay_cannot_bring_back_a_machine_the_owner_withdrew`** | `verify_for` has no date, so a relay could replay a withdrawn machine's old, unrevoked claim. The coordinator's next read drops the device, and a relay never introduces a device the book does not hold. Sabotaged by letting `relayed` take an unknown device. |
| **`red_team_a_peer_cannot_strand_a_machine_at_an_address_it_left`** | A genuine but older presence (the device's own clock, compared only with itself) at an address the machine has left is refused, so the book does not dial it every round nor relay it onward. Sabotaged by dropping the presence-date check. |
| **`red_team_a_presence_from_a_clock_far_ahead_does_not_blind_the_relay`** | A device whose clock once read 2099 signed a genuine presence with that date; as "the latest" it made every later relayed address for it look older, for good. Dates past `MAX_CLOCK_SKEW` from now count for nothing: such a held presence is not the latest, and such a row is refused. Two sabotages (the filter on held presences; the refusal of rows), red both. |
| **`red_team_a_claim_older_than_the_one_held_is_refused`** | A claim the owner has since re-issued is not taken back from a relay. Sabotaged by dropping the claim-date check. |
| **`red_team_a_stranger_asking_for_the_accounts_presences_is_refused`** | `SharedBoard`, what the listener answers from, gives the account's rows only to a device the book holds with this account's claim, and never a machine its own row. Sabotaged by dropping the membership check. |

## `node` — identity on disk (21)

| Test | What it proves |
| --- | --- |
| **`the_keys_can_be_read_while_the_store_is_held_by_another_process`** | Only one process may hold a node's store, and the daemon holds it on every machine that is working — so anything that opened a `Node` refused to run on exactly the machines somebody asks about, including `doctor`, which is what a person runs *because* something is wrong. Stopping the daemon to ask then changes the answer: a node that is not running is not listening, so "can anybody reach me" comes back no, for a reason that is the asking. `Identity::open` reads the keystore and the config, neither of which is locked. |
| **`an_empty_node_home_reads_as_unmounted_storage_rather_than_a_fresh_start`** | A node home on a disk that is not mounted is an *empty directory*, and "no node found, run `itsanas init`" is then advice to create a **second account** on the root filesystem — while the real one sits on a disk nobody is looking at, and the next backup captures the empty one. A directory that does not exist at all still reads as a fresh start. |
| **`a_changed_passphrase_opens_the_same_node_and_the_old_one_no_longer_does`** | `itsanas passphrase` re-seals the keystore without regenerating anything — same account, same device id — the old passphrase stops working, and the pending file is renamed over the keystore rather than left beside it. |
| **`red_team_an_opened_node_bounds_its_writes_by_what_its_pledge_earns`** | Every write goes through `node.store`, and the store refuses nothing it has not been told about. Opening a node pledging 700 GB must hold writes to the 300 GB that earns at 30/70, and a node pledging nothing to the joining allowance, as `keep` does. Sabotaged on the wiring in `Node::assemble` and on the allowance floor: without either, the CLI, the folder and the phone would write unbounded while every store test stayed green. |
| **`red_team_the_disk_room_sets_aside_what_the_pledge_still_owes`** | `Node::disk_room`: free space less what the pledge still owes beyond what is already hosted; owed past free leaves 0, over-hosting owes nothing, an unreadable free space bounds nothing rather than refusing every write, and a disk that really reads 0 free bounds everything (the `itsanas-redteam` agent found 0 read as unknown). Sabotaged (the owed pledge ignored): red. |
| **`red_team_a_folder_beside_the_home_is_charged_on_its_volume`** | `bound_writes` tells the store where a pull's folder copy lands: none without a folder, `SameVolume` for a folder on the home's disk (so `pull_room` charges both copies), `OtherVolume` with that volume's free space otherwise; unsure counts as the same volume. Sabotaged (folder never reported): red. |
| **`red_team_our_own_chunks_in_the_vault_do_not_count_as_hosted`** | Our other devices push to this vault too. `held`, what the pledge has already been paid, must count only other accounts' bytes: 40 000 bytes of our own chunks count 0, 10 000 of a stranger's count 10 000. Counted as hosted, our own backlog shrank the reserve by its own size. And asking must not create a vault directory for us, which made a fresh node list itself among the accounts it hosts (found by the `itsanas-redteam` agent). Sabotaged twice (`held_for_others` returning the whole vault; the owners guard removed): red. |
| **`red_team_files_this_machine_has_not_downloaded_count_against_its_writes`** | A phone knows most of its account only from the laptop's log in its vault. A 300 000-byte file written on the laptop and never downloaded must reach the phone's write bound as `elsewhere`, or the phone writes as though the account were the sliver it keeps. Sabotaged on the `Absent` filter in `bound_writes`. |
| **`red_team_a_machine_that_lends_nothing_writes_by_what_the_account_lends`** | The rule this step first shipped, caught by Rodin before it merged: the bound read *this machine's* pledge. A laptop pledging nothing -- the default -- in an account whose Pi lends 700 GB must be held to the 300 GB the account earns, on opening and again after the refresh every writer calls; it was being held to the joining allowance for the whole account. Sabotaged on both. |
| `a_wrong_current_passphrase_changes_nothing` | Somebody at an unlocked terminal cannot choose a new passphrase for a machine without the current one; the keystore bytes are untouched. |
| **`the_phrase_is_not_written_anywhere_under_the_node_directory`** | Scans every file under the node's home for the phrase. A recovery phrase stored on the machine it protects is not a backup, it is an extra copy for an attacker to find. |
| **`the_phrase_does_not_leak_through_debug`** | The single most likely way for a phrase to escape is a stray `dbg!` or a derived `Debug`. |
| **`red_team_printing_a_node_does_not_print_the_master_secret`** | `Node` derived `Debug`, and `secrets` holds the plaintext encoding of the master secret and the device seed. `Zeroizing` protects the memory's lifetime, not its formatting: its own `Debug` forwards to `Vec<u8>`, which prints every byte. Nothing formatted a `Node`, so this was a loaded gun rather than a shot fired — one `tracing::debug!(?node)` from the whole account in a journal. Every other secret-bearing type here has a hand-written redacting `Debug` for exactly this reason; this was the one that derived, **directly above the comment naming "a struct derive that includes it" as the way this material escapes**. Asserts on any eight-byte run of the secret, not on a field name. |
| **`a_published_test_phrase_is_refused_as_a_real_account`** | Restoring Alice's published phrase as a real account is refused, with an explanation. |
| **`the_device_identity_also_survives_a_restart`** | If the device key changed on every start, every restart would look like a new device to the version vectors and history would fragment. |
| **`creating_over_an_existing_node_is_refused`** | Overwriting would destroy the master secret and make every chunk stored under it permanently unreadable. |
| **`a_phrase_round_trips_through_restore`** | Same account, *different* device id — two machines sharing a device identity would share a sequence counter and fork the log. |
| **`opening_a_missing_node_says_what_to_do_about_it`** | The error names both `init` and `login`. |
| `a_created_node_reopens_with_the_same_identity` | Reopening does not orphan the data. |
| `the_wrong_passphrase_does_not_open_the_node` | Indistinguishable from a tampered keystore, on purpose. |
| **`red_team_a_pledge_under_what_keep_needs_is_refused`** | `Node::check_split`, the one rule `keep`, `pledge`, `space --apply` and the phone's setters ask: a 1 MiB pledge for a 20 GiB keep is refused, naming the pledge that earns it (which is then accepted) and the command `itsanas space --pledge 47G --keep 20G --apply`; a keep inside the joining allowance, or no keep, needs no pledge; a stricter 20/80 split in the node's config refuses what 30/70 allows. Sabotaged (`check_split` always `Ok`; `allowed_for` reading `Split::DEFAULT`): red. |

## `owners` — what a host stores for each account (27)

§8 1c (2026-10-04). A host used to bound only itself, so a client that pledged
nothing was served until every host was full. Now a device presents its
account's signed claim; the host holds at most three sevenths of the account's
claimed pledge for it, or of what it has proved it hosts once its offer is
contradicted; and what goes beyond what proof earns comes out of one share,
3/10 of the host's pledge, for every account together.

| Test | What it proves |
| --- | --- |
| **`red_team_a_device_that_presents_no_claim_stores_nothing`** | A rebuilt client leaves the claim out and says it is old; it is told to update. Sabotaged by admitting a caller the book does not know. |
| **`red_team_a_claim_for_another_device_is_refused`** | A claim presented by a device other than the one it names lends nothing. Sabotaged by dropping the device check. |
| **`red_team_an_account_stores_at_most_three_sevenths_of_its_pledge`** | The 30/70 bargain held by the host: pledge 700, 299 stored, one more byte taken, two refused with the numbers. Sabotaged by no allowance. |
| **`red_team_a_giant_self_signed_claim_takes_at_most_the_share_lent_on_promises`** | Found by Rodin on the plan: every node holds its account key, so a petabyte pledge costs nothing to sign. It takes at most the share. Sabotaged by skipping the share. |
| **`red_team_one_passed_audit_does_not_open_the_host_to_a_giant_claim`** | Found by the redteam agent on the first version, which exempted an account from the share once one device had passed one audit. Proof is counted in bytes the account's devices hold for this host. Sabotaged by counting a passed audit as a petabyte of proof. |
| **`red_team_throwaway_accounts_share_one_quota_lent_on_promises`** | Immediate credit renews with every new account; two of them share one quota. Sabotaged by skipping the share. |
| `a_newcomer_stores_at_once_and_earns_more_by_hosting` | Nicolas's rule: the space offered gives credit at once, inside the share, and hosting this host's chunks earns room past it at the same ratio. |
| **`red_team_a_contradicted_account_keeps_only_what_it_proved`** | Nicolas's rule for a contradicted offer: a paused device makes the allowance `room_earned(proved)`, so cheating never earns more than playing straight. Sabotaged by ignoring the pause. |
| **`red_team_a_device_paused_for_its_audits_proves_nothing`** | A device that hosted, threw the data away and was paused for its audits keeps the records written before the pause (a failed challenge withdraws only the chunk asked about); they earn the account nothing. Sabotaged by counting a paused device's records as proof. Found by sabotage on 2026-10-05: no test failed without the `paused` filter.
| **`red_team_a_terabyte_claim_from_a_device_that_refused_this_host_loses_its_credit`** | §8 1c (i). The claim is self-signed; the one test a host can put it to is offering the device its own chunks, which the push path does every round it dials it. A device that claims a terabyte and refused for a full pledge (`note_peer_full`) keeps only what it proved. Sabotaged by ignoring the refusal, and by ignoring its date. |
| `a_device_that_refused_this_host_keeps_what_it_proved_and_its_siblings_pledges` | Not a sanction: a full device earns `room_earned(proved)` at the same ratio, and the account's other devices keep their pledges. Keeps the red-team test above from passing on a book that refuses everything. Red when the full device's pledge counts as nothing, or as its whole claim. |
| `a_refusal_older_than_the_retry_no_longer_counts` | A refusal older than `FULL_RETRY` -- the window after which the push path probes again -- no longer cuts the claim. Red when any refusal counts, whatever its age. |
| **`red_team_a_device_the_coordinator_says_was_withdrawn_stores_nothing`** | §8 1c (ii). Every node holds the account key, so a withdrawn device signs itself a fresh live claim. Once the coordinator says it was withdrawn it stores nothing, a later live claim does not bring it back, and a later answer does not undo it: a withdrawal is final, as on the coordinator. Sabotaged three ways: `admits` ignoring it, `take` ignoring it, a `Live` verdict replacing it. |
| **`red_team_no_word_from_the_coordinator_means_no_storing`** | Nicolas, 2026-10-05: a device of another account this host has not confirmed with the coordinator stores nothing, and neither does one whose confirmation is older than `STANDING_FOR` while the coordinator stays silent. Sabotaged by admitting an unconfirmed device, and by never letting a confirmation lapse. |
| `a_silent_coordinator_keeps_a_fresh_confirmation_and_an_empty_answer_ends_it` | No answer leaves a fresh confirmation standing, so one missed round is not an outage; an answer with nothing for the device ends it at once. |
| `this_hosts_own_devices_need_no_word_from_the_coordinator` | A household replicates to itself with the coordinator down or unconfigured, and its own devices are never asked about: the bargain is between accounts. |
| `due_names_the_unconfirmed_and_the_ageing_and_never_the_withdrawn` | What a round asks the coordinator about: the never-confirmed and those past half of `STANDING_FOR`, never a withdrawn device, at most the number asked for. |
| **`red_team_junk_devices_do_not_take_every_question_from_a_real_one`** | Found by the redteam agent: the round asked about the book in id order, eight a round, so junk devices (free keys) took every question for ever. Twenty disowned junk devices presented first, then a real one: the round asks about the real one alone. Sabotaged by removing the `ASK_AGAIN_AFTER` back-off. |
| **`red_team_a_flood_of_newcomers_does_not_let_a_real_confirmation_lapse`** | A confirmation past half of `STANDING_FOR` is asked about before every newcomer, however many presented before it. Sabotaged by id order, and by ordering on presentation alone. |
| **`red_team_presenting_again_does_not_buy_another_question`** | A device answered once is not asked again inline however often it presents, so one junk device cannot spend the minute's budget. Sabotaged by asking whenever a device is unconfirmed. |
| **`red_team_a_coordinator_cannot_vouch_with_a_claim_for_another_device_or_account`** | The coordinator's answer is checked like any claim: one for another device, another account, or not signed by the account confirms nothing (`Unenrolled`). A coordinator can refuse a member, never vouch for one. Sabotaged by dropping each of the three checks in `verdict`. |
| `a_device_is_asked_about_when_it_presents_so_it_stores_on_its_first_round` | With an asker, a device of another account is asked about when it presents, so it stores on its first round rather than the host's next; a confirmed device and this host's own are not asked. Red when presentation does not ask, or asks about this account. |
| **`red_team_a_flood_of_presentations_does_not_become_a_flood_of_questions`** | Device keys are free: asking on every presentation would make a host an amplifier against its coordinator. Fifty presentations ask `ASKS_PER_MINUTE` (30) questions. Sabotaged by removing the budget. |
| **`red_team_a_withdrawn_device_is_forgotten_and_a_sixth_is_refused`** | A withdrawal presented to the host forgets the device; a sixth device adds nothing. Sabotaged twice: keep the withdrawn device, drop the five-device limit. |
| **`red_team_a_device_cannot_store_under_another_accounts_name`** | A device stores only for the account its claim names -- the host's own included, whose devices are exempt. Sabotaged by dropping the check. |
| **`red_team_empty_claims_cannot_lock_newcomers_out`** | Found by the redteam agent: free keys fill the book with claims that store nothing. A full book forgets one whose account holds nothing here. Sabotaged by refusing when full. |
| `a_claim_is_checked_for_its_signer_not_its_date` | Found by the redteam agent: a host on a Pi with no clock reads 1970 and would refuse every claim. `verify_origin`, as for presences. Sabotaged by checking the date (ten tests go red). |

## `keeping` — a round on a device short of room (3)

`src/keeping.rs`. Where the choice, the catalogue and a real socket meet. Both
tests were confirmed by sabotage: removing the release, and removing the notice
to the peer, each turns the matching test red.

| Test | What it proves |
| --- | --- |
| **`a_full_device_makes_room_for_a_better_ranked_file`** | The property that separates a budget from a ratchet. The first version filled up once and from then on nothing new could arrive, because nothing old could leave — measured on the trial device, told to keep 200 KiB and holding 907 KiB with no path back down. |
| **`releasing_content_withdraws_this_device_from_the_peers_ledger`** | A device that lets go of content and does not say so becomes a liar, and the lie inflates the one number somebody consults before believing their data is safe. The audit would find it eventually: sixteen chunks per peer per round, which on a million-chunk account is most of a year. |
| `a_machine_with_room_takes_the_ordinary_path` | A laptop chooses nothing and takes the whole account, exactly as before the selective path existed. |

## `config` — settings (26)

| Test | What it proves |
| --- | --- |
| **`a_split_in_the_config_file_overrides_the_default`** | The point of the field, and the reason the ratio stopped being a constant. Without it the network cannot be tuned to the machines actually in it — a pool of phones needs a different split from a pool of servers, because a phone under `keep` has released its data and costs the network three copies rather than two — and the argument for each number lives in a commit message instead of a file. Also checks the absent case: no line means the network's default, not nothing. |
| **`red_team_an_impossible_split_in_the_config_file_is_refused_where_it_enters`** | `split = 30/0` divides by zero the first time somebody runs `itsanas keep`, and the daemon is usually what holds the store open when they do. Refused when the file is *read*, in the same breath as the listen address, because a value that cannot work must never reach the arithmetic that assumes it can. What it catches is a parser that takes the two numbers itself and leaves the check to `Split::new`'s callers, which is how a validated type ends up with an unvalidated door. The refusal has to show what a split looks like, or the person fixing it is guessing. |
| **`red_team_a_node_cannot_grant_itself_a_more_generous_split`** | Nothing on the network enforces the bargain yet, so `itsanas keep` refusing an unearned limit is the only live check, and it reads this field. Accepting `split = 1000/1` would switch it off with a text editor where it used to take a rebuilt client. Found by an audit of the change that introduced the field. Checks `31/69` and `301/700`, one part in a hundred and one in a thousand over, so a comparison that divides and rounds is caught too; stricter (`25/75`) and equal (`3/7`) still load. |
| **`an_unknown_setting_is_an_error_rather_than_being_ignored`** | A silently discarded typo is how a node ends up pledging nothing while its operator believes it pledged a terabyte. |
| **`defaults_are_safe`** | Pledge defaults to zero and listen defaults to loopback. A node that has not said what it offers has not offered any. |
| **`a_nonsense_size_is_refused_rather_than_read_as_zero`** | Reading "ten gigabytes" as 0 would silently disable hosting. |
| **`red_team_an_announce_nobody_can_dial_is_refused_where_it_enters`** | `announce` is published to every machine of the account, so a wrong value is a wrong address on all of them. `0.0.0.0:9797` is the obvious slip — it is what `listen` says on every node — and tells peers to dial nothing; loopback and `localhost` are worse, because they resolve on every machine, so each member dials *itself*, authenticates against its own device id, and reports the wrong machine as unreachable. Refused when the file is read, alongside the listen address. |
| `an_announced_address_survives_the_round_trip_through_the_file` | Written by `itsanas announce`, read back by the daemon at the next start. A setting that rendered and did not parse would leave a node publishing its LAN address again after a restart, with nothing saying so. |
| `a_malformed_line_names_its_line_number` | Errors are actionable. |
| `an_overflowing_size_is_refused` | `999999999999T` does not wrap. |
| `sizes_parse_the_way_people_write_them` | `500`, `1K`, `2MB`, `10G`, `1TiB`. |
| `sizes_format_readably` / `formatting_never_panics_at_the_extremes` | Output is legible at every magnitude. |
| **`a_quoted_price_parses_back_to_no_less_than_the_price`** | A refusal names the pledge that would make a request legal and the command that sets it. `format_size` floors to a tenth, so at 30/70 the price of 31 GiB (72.33 GiB) read "72.3 GiB" and pledging exactly that was refused again; and the suggested `--pledge 93.0 GiB` never parsed at all. The quote is now a whole unit rounded up, and this checks it reads back as no less at every remainder — and pins `73G`, the figure the documents use. |
| **`red_team_an_instance_name_cannot_leave_the_home_directory`** | `itsanas --instance NAME` joins the name onto the home directory, and `install/clean.sh --instance NAME` deletes what it names. A `/`, `\` or `..` let through would open — or erase — a directory that is no instance's. Refused, not cleaned: `../etc`, `a/b`, `..`, uppercase, an edge dash, a space, 33 characters, the empty name, and `passphrase`, because `~/.itsanas-passphrase` is the default node's passphrase file. |
| `an_instance_lives_where_provision_sh_puts_it` | `provision.sh --instance x` installs into `~/.itsanas-x`; a CLI that mapped the name anywhere else would open an empty home beside the installed node and report a node that is not there. |
| **`red_team_a_node_under_the_other_home_variable_is_found`** | `stranded_node` finds a node at the same place under the other of `HOME` / `USERPROFILE` only when the chosen home holds none, and not without another home. The check behind the CLI's refusal to mint a second identity. Sabotaged (never found): red. |
| **`red_team_on_windows_the_profile_wins_over_home`** | `provision.ps1` builds every node home from `$env:USERPROFILE`; `HOME` is not a Windows variable, but Git Bash sets one and some installs make it permanent. Preferring `HOME` there made the CLI and the script name two different `~/.itsanas-NAME`. On Windows the profile wins, elsewhere `HOME`, and an empty value is no home. Sabotaged (HOME first everywhere): red. |
| `a_config_round_trips` / `comments_and_blank_lines_are_ignored` / `several_peers_accumulate` / `a_missing_file_reads_as_defaults` | The format works. |
| **`a_listen_address_nobody_can_bind_is_refused_when_the_file_is_read`** | A `listen` line was stored without being parsed, so `listen = localhost:9797` was accepted and failed later at `serve`. Under systemd with `Restart=on-failure` that is a unit dying every thirty seconds with the reason in a journal nobody opens. The test carries its own control: the same file with a bindable address must still load. |
| `an_address_that_loads_is_stored_exactly_as_written` | Validation does not rewrite the value. IPv6 has several spellings of one address, and a node that publishes one form while its owner reads another has two answers to one question. |

# `itsanas-android` — the JNI boundary (10)

`src/lib.rs` and `src/devices.rs`. The only crate that relaxes the unsafe
lint, and the only one whose contract is with another language. The device
tests run against a real coordinator in-process (`itsanas-coord`, dev-only,
`hostile` for `bound_devices`). **The Kotlin is not tested anywhere**: no CI
job builds or runs the app; `scripts/build-apk.sh` was run by hand on
2026-10-01 and compiled it, and the screens were not opened. What is tested here is deliberately
thin: the behaviour underneath belongs to the crates that own it, and repeating
it through a JNI call would test the same thing twice. What cannot be tested
anywhere else is the *shape* — field names Kotlin parses by string, and a
failure that has to arrive as a sentence rather than a crash.

The real test of this crate is the one in `docs/PORTING.md` §4: an account
restored from twenty-four words on an Android 15 emulator, five files pulled
from a host over a real socket, and one of them opened.

| Test | What it proves |
| --- | --- |
| **`a_plan_is_reported_with_the_names_kotlin_reads`** | The field names are a contract with another language, and a rename here fails silently over there — the application would show an empty reason and no interval, and nothing would say why. |
| **`asking_a_closed_node_says_so_rather_than_crashing`** | Every entry point can be called before an account is open, because Android restarts a process whenever it likes. It has to answer with a sentence a person can act on, not with a panic crossing into the JVM. |
| **`red_team_the_phone_saves_pledge_and_keep_as_a_pair_and_says_so_in_its_words`** | `set_limits`, the body of `setLimits`, which the settings screen now calls: raising pledge and keep together is accepted (saved one after the other, the new keep was checked against the old pledge and refused); a pair that does not fit is refused naming the screen's field, not `itsanas space`; a negative pledge is refused rather than read as its absolute value (found by `itsanas-redteam`). Sabotaged three ways (keep checked against the old pledge; `SplitRefusal`'s own words; `unsigned_abs`): red. |
| **`every_kotlin_native_call_has_its_rust_entry_point_and_back`** | Reads `Native.kt` and `lib.rs`: every `external fun` has its `Java_fr_ngas_itsanas_Native_*` and back. A name on one side only compiles on both and fails on the phone at the first tap with `UnsatisfiedLinkError`, and no CI job runs the app. Sabotaged by renaming `withdrawDevice`'s export: red. |
| **`red_team_the_phone_withdraws_only_a_listed_full_id_and_never_itself`** | `devices::withdraw` against a coordinator: an abbreviated id, an id that is no device of the account (the coordinator would file it as withdrawn for ever), another account's device and this phone itself are all refused with nothing withdrawn; the listed full id withdraws exactly that device. Sabotaged three ways (resolve a prefix against the listing; drop the self check in `coordinator::withdraw_device`, shared with the CLI; drop the listing check): red. |
| **`red_team_an_all_phone_account_at_the_limit_can_free_a_slot_from_the_phone`** | Five phones enrolled and never heard from again; the sixth, not enrolled, sees none of them. Joining answers `atCap` with all five full ids and words naming the screen, not a command; withdrawing one from the unenrolled phone works (what the refusal named counts as shown); joining again takes the slot. Sabotaged two ways (the refusal returned as an error; what it named not remembered): red. |
| **`red_team_a_device_withdrawn_elsewhere_is_not_offered_from_an_old_refusal`** | Found by `itsanas-redteam`: remembered ids were merged into a complete listing too, so a device withdrawn from another machine kept a Withdraw button. Refused at the limit, then one device withdrawn elsewhere and the phone enrolled by another path: the list is the coordinator's, the withdrawn device absent. Sabotaged by merging regardless: red. |
| **`red_team_a_refusal_naming_only_some_devices_says_so`** | Found by `itsanas-redteam`: an account above the limit (ten devices, enrolled with the bound off) is named only eight by the coordinator, "and 2 more"; the phone said "already has 8". It now answers `partial` and says the rest are named next time, guessing no count. Sabotaged by `partial = false`: red. |
| `the_list_marks_this_phone_among_the_account_s_devices` | The devices screen's list: both devices of the account, this phone marked once, `complete` true when enrolled. |
| **`red_team_the_phone_s_setters_keep_the_split`** | `set_pledge` and `set_keep`, the bodies of `setPledge` and `setKeep`, refuse a pledge under what keep needs and a keep the pledge does not earn, and leave the node file and the node in memory (a process-wide mutex, saved by the next setter) as they were; with no keep, both go through. Reaches the two functions, **not the JNI shims** that lock the node and call them -- those are covered only by reading. Sabotaged five ways (either call dropped; either call moved below its assignments; `check_split` always `Ok`): red. |

# `itsanas-drive` — the account as a folder (9)

Two files, and they are tested for two different reasons.

`src/lib.rs` is the **projection**: given everything the account knows and a
directory somebody is looking at, what should appear. That is where the bugs
live — a prefix is not a directory, a path separator is not the same on both
sides, an absent file must still have a size — and none of it needs a
filesystem driver.

`src/projfs.rs` is the **binding**, written here rather than taken from
crates.io because `projfs 0.1.2` reaches `owning_ref 0.3.3` through `chashmap`
and that is RUSTSEC-2022-0040. Driving real ProjFS in a test needs the Windows
feature enabled, an administrator to enable it and a reboot, none of which
belong in a suite that has to pass in under a minute on a Raspberry Pi. So what
is tested is the three conversions that sit either side of the boundary and
would fail silently: the tests below are the ones whose failure Windows would
report as something other than itself. The binding as a whole is covered by
having been used — the account was browsed in Explorer on 2026-09-08.

Writing back into the folder is not built. The notification callback exists and
is deliberately unbound: honouring it means deciding what a local edit does to
an account several machines hold.

| Test | What it proves |
| --- | --- |
| **`a_directory_is_a_prefix_several_files_share`** | An account stores paths, not a tree: nothing anywhere records that `Documents` exists, only that `Documents/report.pdf` does. A projection that does not invent the directory shows an account with sub-folders as an empty one. |
| **`a_file_that_is_not_here_is_still_a_file_with_a_size`** | The whole point of a virtual drive. A placeholder with no size shows as zero bytes, and somebody concludes their file is damaged rather than absent. |
| `a_name_that_merely_starts_the_same_is_not_inside_it` | `Photos-old` is not inside `Photos`. |
| **`the_separators_the_operating_system_uses_are_not_the_accounts`** | Windows hands back backslashes and the account speaks in slashes. Getting it wrong does not fail: the listing comes back empty, which reads as "the account is empty" rather than as "the path did not match". |
| **`a_guid_survives_the_round_trip_that_keys_the_cursor_map`** | Enumeration cursors are keyed by the sixteen bytes of the id Windows hands back, because keying them on a `uuid` would mean a crate for it. If that conversion were not injective, two open enumerations would share a cursor and Explorer would show one directory's entries inside another — with nothing in any log, because both lookups succeed. |
| **`a_directory_is_flagged_as_one_and_a_file_is_not`** | Windows decides whether to offer a folder or a file from one bit. Wrong, and a directory is unopenable rather than wrong-looking. |
| **`every_string_handed_to_windows_ends_in_a_nul`** | Every call in the binding takes a `PCWSTR` and walks it until it finds a zero. A `Vec<u16>` without one is a read past the end of an allocation, and it would work by accident most of the time. |
| **`red_team_a_file_shorter_than_windows_believes_is_refused_not_padded`** | The one a red-team sweep found, and the one the manual test could not. The placeholder's size is written once from the catalogue and nothing calls `PrjUpdateFileIfNeeded`, while the sync loop in the same process keeps adopting newer versions from peers — so a file that shrinks on another machine leaves Windows asking for the old length. `Source::read` returned `()`, so a short fill was invisible, and the whole buffer went to `PrjWriteFileData` regardless. The tail was **uninitialised heap**: most likely the plaintext of a file hydrated a moment earlier through the same allocator, arriving inside a different file. The buffer is zeroed now, `read` returns a count, and a short answer stops. **The `SAFETY:` comment above it said the tail was padding Windows discards** — the gate checks that a reason is written, not that it is true. |
| **`red_team_a_read_past_the_end_serves_nothing_rather_than_zeros`** | The same fault at its extreme: an offset past the content fills nothing at all. Padding would hand back a block of zeros that reads as a legitimate hole in a sparse file. |

# `itsanas-placement` — unit tests (34)

## `nodeset` — where a chunk belongs (16)

| Test | What it proves |
| --- | --- |
| **`a_small_network_never_spreads`** | The direction that can do damage. Spreading three copies over two peers means one copy per chunk — a privacy preference turned into data loss, on exactly the networks least able to afford it. Walks every count below the threshold and asserts the answer is off. |
| **`the_threshold_is_where_a_holder_stops_getting_most_of_the_account`** | Nine candidates for three copies, because each then receives a third. The arithmetic is the reason for the number, so the number is pinned to it rather than chosen. |
| **`enough_machines_with_no_room_is_not_enough`** | The objection a count of machines cannot see. Nine peers offering a gigabyte each are nine peers; they cannot hold four terabytes three times over, and a threshold counting only machines says "on" while the data has nowhere to go. This is the condition that decides whether the largest contributor can be served at all. |
| **`not_knowing_the_capacity_blocks_rather_than_passes`** | Today's real state: a node learns its peers' pledges from the coordinator and nothing asks. An unknown treated as a pass is how a check comes to bless the one case it was written for. |
| `nothing_stored_is_not_a_reason_to_spread` | Zero copies or zero bytes means nothing to place, and spreading nothing is not a state worth entering. |
| **`removing_a_node_moves_only_that_nodes_share`** | The M5 exit criterion, and stronger than the usual phrasing: **zero** chunks move between two *surviving* nodes. With modulo hashing almost everything moves, which at real scale means re-uploading the whole network. |
| **`adding_a_node_only_pulls_in_its_own_share`** | The same property in the other direction. |
| **`distribution_matches_pledged_capacity`** | A node pledging 4× holds roughly 4× as many chunks, measured over 20 000 chunks across a 1:8 spread. Without this the "mutual" in mutual storage is a fiction and the small nodes carry the network. |
| **`no_floating_point_is_involved`** | Greps the module's own source for `f64`, `.ln(`, `.powf`. `f64::ln` is libm-dependent and two platforms can differ in the last ulp, which would make two machines disagree about where a chunk lives — silently, with no error. |
| **`placement_is_deterministic`** / **`the_answer_does_not_depend_on_the_order_the_set_was_built_in`** | Two peers given the same membership by different routes reach the same answer. |
| **`a_users_own_devices_always_hold_their_own_data`** | A user whose peers have all left must still be able to read their own files. |
| **`owner_affinity_does_not_starve_a_user_with_many_devices`** | Documents the deliberate current behaviour when a user has more devices than the replication factor — right for availability, wrong for durability, and the fix belongs with the repair loop. |
| **`one_enormous_node_cannot_take_over_the_swarm`** | The slot cap bounds how much of the network's data can be concentrated on the single machine most worth attacking. |
| **`different_owners_get_different_placements_for_the_same_chunk_id`** | Placement must not reintroduce the cross-user correlation that blinded chunk ids remove. |
| **`a_replica_set_never_contains_the_same_node_twice`** | Three replicas on one machine is one replica with extra steps, and would make the durability accounting a lie. |
| `identical_capacities_distribute_evenly` | No node is starved; none is favoured. |
| `a_swarm_smaller_than_the_replication_factor_returns_everyone` / `an_empty_swarm_places_nothing_rather_than_panicking` / `asking_for_zero_replicas_returns_none` | Edges. |
| `duplicate_and_zero_capacity_nodes_are_refused` | Malformed membership is rejected at construction. |

## `repair` — noticing a chunk is running out of copies (13)

| Test | What it proves |
| --- | --- |
| **`a_chunk_nobody_holds_is_planned_for_rather_than_overlooked`** | The case that loses data. A chunk absent from the census would be invisible to repair. |
| **`a_swarm_too_small_to_meet_the_floor_raises_an_alert`** | Silence would mean a user believing they have three replicas when the network can only ever give them two. |
| **`a_chunk_with_a_single_copy_left_is_flagged_as_critical`** | The difference between "a node is having an evening off" and "one more failure and this is gone". |
| **`an_offline_node_is_not_a_reason_to_move_data`** | A sleeping node will come back. Re-placing its chunks would mean the network churns every time somebody shuts a laptop — but the shortfall is still reported. |
| **`repair_never_plans_a_deletion`** | An over-replicated chunk is wasted space; a wrongly deleted one is gone. The plan type has no deletion variant, and this test makes adding one a deliberate act. |
| **`repair_never_sends_a_chunk_to_a_node_that_should_not_hold_it`** | Otherwise repair slowly spreads every chunk to every node and capacity accounting stops meaning anything. |
| **`a_holder_that_has_left_the_swarm_does_not_count_towards_the_floor`** | Counting a decommissioned machine means believing in a replica that no longer exists. |
| **`the_census_counts_distinct_holders_not_repeated_claims`** | A peer answering twice must not inflate the replica count into a false sense of safety. |
| `one_missing_replica_produces_exactly_one_push_to_the_right_node` | The ordinary case, exactly. |
| `a_fully_replicated_chunk_needs_nothing` | No make-work. |
| `a_plan_is_deterministic_and_ordered` | Two nodes produce comparable plans, so an operator can diff two logs. |
| `an_empty_census_produces_an_empty_plan` / `nothing_is_planned_when_no_node_is_reachable` | Edges. |

---

# `itsanas-folder` — unit tests (32)

## `decision` — what should happen to one path (15)

A pure function of three content hashes: what is on disk, what the store says,
and what this device last put there. Every branch is a unit test because
several of them are hard to stage on a real filesystem and all of them are
destructive if wrong.

| Test | What it proves |
| --- | --- |
| **`a_file_that_was_never_downloaded_is_exported_not_deleted`** | The most dangerous confusion in the design. A device that has never had a file must not read its absence as a deletion — that would announce the removal of everything its owner has. |
| **`deleting_from_the_store_only_ever_follows_a_recorded_local_file`** | Exhaustive over all 27 combinations: the destructive action is unreachable unless the ledger says this device genuinely had the file. |
| **`a_stale_ledger_alone_never_moves_data`** | Whatever the ledger says, if disk and store agree the answer is bookkeeping — never an upload, download or delete. |
| **`the_same_edit_made_twice_is_not_a_conflict`** | Both sides changed to identical content. A sibling here would litter the folder for nothing. |
| **`a_delete_racing_an_edit_brings_the_file_back`** / **`an_edit_racing_a_delete_keeps_the_edit`** | Matches the sync engine: an unexpected file costs a second, a lost edit is unrecoverable. |
| **`no_input_combination_panics_or_is_undecided`** | All 27 shapes of the problem are decided. |
| `a_new_local_file_is_imported` / `an_edited_local_file_is_imported` / `a_file_the_user_deleted_is_removed_from_the_store` / `a_remote_edit_is_written_out` / `a_remotely_deleted_file_is_removed_from_disk` / `two_different_edits_keep_both` / `both_sides_deleting_agrees` / `nothing_to_do_when_all_three_agree` | The ordinary cases. |

## `scan` — reading the folder safely (12)

| Test | What it proves |
| --- | --- |
| **`a_logical_path_can_never_escape_the_folder`** | Traversal, absolute paths, drive letters and Windows device names all refused. These strings arrive in a peer's log, so a path that escaped would let a peer write anywhere on the disk. |
| **`symlinks_are_skipped_rather_than_followed`** (unix) | A link inside the folder pointing at `~/.ssh` would otherwise quietly upload a private key, and the user would see only a harmless-looking file. |
| **`operating_system_debris_is_ignored`** | Every machine writes its own `.DS_Store`; syncing them means they fight forever. |
| **`the_staging_directory_is_never_synced`** | Otherwise the folder syncs its own scratch space back and forth. |
| **`nested_directories_become_slash_separated_logical_paths`** | Backslashes must not leak into logical paths, or one file has two names on two machines. |
| `round_tripping_a_path_through_the_filesystem_and_back_is_stable` | The mapping is a genuine round trip. |
| Others | Flat scans, empty folders, missing folders, sizes and mtimes, paths outside the root. |

## `watch` — noticing changes (5)

| Test | What it proves |
| --- | --- |
| **`the_debounce_is_long_enough_to_outlast_an_editor_save`** | Acting on the first event would import a half-written file, and because the store hashes what it reads, that truncation would become a real version and replicate. |
| **`watching_a_missing_directory_is_an_error_rather_than_silence`** | Silently watching nothing would mean the daemon believes it is reacting when it is not. |
| `a_real_change_is_noticed` / `a_quiet_folder_times_out_rather_than_blocking_forever` | It works, and it does not hang. |

---

# `itsanas-folder` — integration tests (23)

| Test | What it proves |
| --- | --- |
| **`a_brand_new_device_downloads_everything_and_deletes_nothing`** | The catastrophe. An empty folder on a device that has never synced must produce downloads, not a mass deletion. |
| **`a_file_over_the_budget_is_refused_left_on_disk_and_blocks_nothing_else`** | A file the account has no room for lands in the report's failures with the numbers, twice in a row, while a small file beside it is imported; the refused file is untouched on disk and no deletion is held. Refused must never read as deleted, or a quota would cost somebody the original. |
| **`a_file_the_user_deletes_is_deleted_everywhere`** | The counterpart — a genuine delete must propagate, with a tombstone so an offline device does not resurrect it. |
| **`an_imported_file_is_announced_to_peers_not_just_stored_locally`** | A real bug found by running two daemons: the reconciler wrote to the store but never sealed a log segment, so files looked synced on the machine that had them and existed nowhere else. |
| **`a_deletion_is_announced_to_peers_too`** | The same, for deletes. |
| **`a_pass_that_changes_nothing_does_not_produce_an_empty_segment`** | Flushing unconditionally would mint a segment on every idle scan and grow the log without bound. |
| **`reconciling_twice_does_nothing_the_second_time`** | A non-idempotent reconciler means a daemon uploads the folder forever and never settles. |
| **`a_local_edit_colliding_with_a_remote_one_keeps_both`** | Both survive, and the sibling keeps its extension so it still opens in the right application. |
| **`a_deep_pass_catches_an_edit_the_fast_path_misses`** | Documents the size-and-mtime gap and proves the deep scan closes it. |
| **`deleting_the_last_file_in_a_tree_prunes_the_empty_directories`** | Without it, every machine slowly fills with empty directories nothing removes. |
| **`no_staging_file_survives_a_reconcile`** | Every export would otherwise leak a temp file. |
| `a_second_device_reproduces_the_folder_exactly` | Two machines, one folder content, byte for byte. |
| `a_full_corpus_round_trips_through_a_folder_byte_for_byte` | A real data set, unchanged. |
| Others | New files, edits, remote changes, remote deletes, nested directories, delete/edit races, identical concurrent edits, empty folders. |

---

---

# `itsanas-discover` — serverless local discovery (43)

The only parser in the project fed unsolicited packets by anybody, with no
handshake in front of it. Everything else sits behind TLS and behind a peer that
has already proved which device it is, so this crate is tested the way a network
edge has to be: every corruption, every truncation, and the failure modes of the
hardware it will actually run on.

## `beacon` — the announcement (17)

| Test | What it proves |
| --- | --- |
| **`a_device_cannot_advertise_a_device_it_does_not_own`** | The reason the packet is signed at all. Without it, anyone on the network claims to be the Raspberry Pi and every node dials them instead. |
| **`corrupting_any_single_byte_is_refused_and_never_panics`** | Every single-bit flip of every one of 147 bytes. The decoder may reject; it may not take the daemon down, and it may not accept a mutated field. |
| **`every_truncation_and_extension_is_refused_before_anything_is_read`** | The length is fixed, so every other length is rejected before a field is touched. There is no size on the wire for an attacker to lie about. |
| **`arbitrary_garbage_never_panics`** | Anything at all arrives on a UDP port, including another protocol's traffic on a machine that reuses the number. |
| **`a_signature_from_another_domain_does_not_verify_here`** | Domain separation checked rather than assumed: a signature the device made for the peer protocol must not be replayable as a presence announcement. |
| **`an_ancient_clock_still_produces_a_valid_announcement`** | A Raspberry Pi 4 has no real-time clock and announces itself believing it is 1970. It must still be findable, or a machine that just came back is invisible until NTP runs. |
| **`red_team_the_user_id_never_appears_on_the_wire`** | See **Red-team tests** above. |
| **`red_team_two_beacons_of_one_account_carry_unlinkable_tags`** | See **Red-team tests** above. |
| **`red_team_a_stranger_holding_the_user_id_cannot_recognise_the_tag`** | See **Red-team tests** above. |
| **`red_team_a_tag_lifted_onto_another_device_is_not_recognised`** | See **Red-team tests** above. |
| **`red_team_a_version_1_beacon_is_still_heard_and_never_counted_as_mine`** | See **Red-team tests** above. |
| `the_household_recognises_itself_whatever_its_clock_says` | No clock is in the tag: a Pi 4 has no RTC and boots in 1970, and a tag rotated on time would make its own household treat it as a stranger exactly when it came back from a power cut. Checked at clocks 0, 2023 and `u64::MAX`. |
| `the_layout_is_exactly_as_documented` | The wire format is a compatibility commitment. If it drifts, an older build on another machine stops finding this one and the symptom is "discovery silently does nothing". Version 2 changed byte 8 and the meaning of bytes 9..41 on purpose and kept the 147 bytes and every other offset; a version 1 packet is checked to be the same size. |
| `an_unknown_version_is_refused_not_guessed_at` | No optimistic reinterpretation of a future format, whose fields may mean something else entirely at these offsets. Versions 0, 3 and 255 are refused; 1 and 2 are read. |
| `foreign_traffic_is_discarded_on_the_magic_rather_than_the_signature` | Sharing a port with something else costs one comparison, not a signature check per packet. |
| `a_zero_port_is_refused` | An announcement nothing can serve is either a bug or bait for a connection that cannot succeed. |
| `an_announcement_round_trips` | The basic path. |

## `neighbours` — the bounded table (14)

| Test | What it proves |
| --- | --- |
| **`a_rebooted_pi_with_a_reset_clock_is_still_followed_to_its_new_address`** | Why the *receiver's* clock decides and the sender's is ignored. Superseding by sender clock would leave a rebooted Pi pinned to a stale address until NTP ran — exactly when someone is waiting for it to come back. |
| **`the_table_never_grows_past_its_capacity`** | A device id is a free keypair, so anyone on the network can mint valid announcements without limit. Unbounded means an out-of-memory kill on the Pi, triggered by a stranger. |
| **`a_flood_of_strangers_cannot_evict_a_known_peer`** | The eviction attack. Without protection, a flood pushes the Pi out of every table and the household stops syncing while every node believes discovery is working. |
| **`red_team_a_copied_tag_does_not_sort_a_stranger_among_my_machines`** | See **Red-team tests** above. |
| **`red_team_a_replayed_version_1_beacon_does_not_demote_an_upgraded_machine`** | See **Red-team tests** above. |
| **`own_devices_are_dialled_before_strangers`** | Reaching your own machines is what makes a folder appear; a stranger is a hosting candidate and can wait. |
| `a_table_full_of_protected_devices_refuses_a_stranger_rather_than_forgetting_one` | The bound is never satisfied by discarding something known real. |
| `the_oldest_unprotected_entry_is_the_one_evicted` | Eviction is least-recently-heard, not arbitrary. |
| `expiry_forgets_the_quiet_but_keeps_your_own_switched_off_machines` | A laptop that is off has not stopped being your laptop; forgetting its address costs a slower reconnection every time it wakes. |
| `the_dial_order_is_stable_across_two_nodes_with_the_same_view` | Two machines that heard the same announcements produce the same list, so they do not retry each other in lockstep. |
| `a_device_that_changed_network_is_followed_and_reported` | A laptop moving between networks is followed, and the move is news. |
| `repeating_the_same_announcement_is_not_reported_as_news` | A beacon arrives every thirty seconds forever. Logging each one makes an unreadable journal, which is the same as no journal on the day something breaks. |
| `a_new_device_is_recorded_with_the_address_it_was_heard_from` | The address comes from the datagram, never from the packet. |
| `a_capacity_of_zero_is_treated_as_one_rather_than_never_recording` | A misconfiguration degrades rather than silently disabling discovery. |

## `lan` — the socket (12)

| Test | What it proves |
| --- | --- |
| **`announcing_to_port_zero_is_refused_rather_than_sent_into_the_void`** | **Found by running it, not by a test.** `bind(0)` used one number for both the local port and the broadcast target, so an ephemeral bind sent every announcement to `255.255.255.255:0` — accepted by the operating system, delivered to nobody, reported as five successful sends. |
| **`an_ephemeral_bind_still_announces_to_the_discovery_port`** | The other half of the same bug: listening and announcing are separate numbers. |
| **`an_oversized_datagram_is_rejected_rather_than_truncated_into_a_valid_one`** | The receive buffer is larger than a valid announcement on purpose. With an exact-sized buffer the kernel trims the excess and hands up something that parses, which is how a padded packet smuggles data past a parser. |
| **`a_tampered_announcement_is_refused_at_the_socket`** | Verification happens on the real path, not only on hand-built byte arrays. |
| `an_announcement_crosses_a_real_socket_and_verifies` | End to end over a real UDP socket. |
| `a_quiet_network_times_out_rather_than_blocking_forever` | A daemon polls this in a loop; a silent network must leave it idle, not hung. |
| `foreign_traffic_on_the_port_is_reported_as_foreign_not_as_a_failure` | A busy network must not flood the log and hide the failure that matters. |
| `a_broadcasting_socket_asks_the_kernel_for_broadcast` | Without `SO_BROADCAST` nothing reaches 255.255.255.255 and every send still reports success. |
| **`two_nodes_on_one_machine_both_hear_the_discovery_port`** | Replaces a test that asserted the opposite. Two accounts on one machine bind UDP 21037 together (`SO_REUSEADDR`) and both hear one real broadcast. The refusal it replaces did not stop the second node: it ran with discovery silently off. Sound only because discovery never sends unicast, which reaches one socket of several. **On a machine that cannot send a broadcast at all** — the macOS CI runner answers "No route to host" — only the shared bind is checked, and the test says so on stderr; hearing is checked on Linux and Windows. |
| **`red_team_an_upgraded_listener_still_learns_a_not_yet_upgraded_sender`** | See **Red-team tests** above. |
| `two_accounts_on_one_machine_hear_each_other_and_keep_apart` | Named instances of two accounts on one machine learn each other's device and port over real sockets, and neither reads the other as its own: only the account's key decides that now. |
| `the_announce_interval_is_not_expensive_to_leave_running` | An acceptance criterion, not a preference: the first version that keeps a laptop awake gets uninstalled. Under half a megabyte a day, and one lost packet never forgets a peer. |

---

---

---

# `itsanas-node` away from home (`tests/away_from_home.rs`) — the lookup a member elsewhere makes (5)

A real coordinator on a real socket and three real nodes of one account. The
only thing simulated is which machine each node runs on, which is the thing the
test is about.

| Test | What it proves |
| --- | --- |
| **`red_team_a_coordinator_cannot_pass_off_an_address_its_machine_never_signed`** | The unit test proves `verified` drops a forgery; this proves `contact` calls it, and that the fallback to unsigned `Peers` is not what answers. A real coordinator whose directory holds an address for the VM that the VM never signed (`Directory::plant_presence`, behind the dev-only `hostile` feature): the laptop's read drops it, keeps the Pi's genuine presence, and counts one lie for the daemon to report. Sabotaged by reading `SignedPeers` without checking. Since 2b.3 it also checks that the Pi's row comes back with the account's claim (`Contacted::claimed`); sabotaged by dropping the claims the read kept. Planted at the server's own time: planted at the test's fixed date, the presence had expired, was left out for that reason, and the first version of this test passed without checking a signature -- the count caught it. |
| **`red_team_a_coordinator_that_pretends_to_be_old_cannot_talk_a_node_down_to_an_unsigned_list`** | The downgrade, on a real coordinator told to hang up on `ClaimedPeers` and `SignedPeers` as an older one does (`Directory::play_old`, `hostile` feature); the unsigned read carries no claims, so nothing from it could be relayed. A node with no history still reads it -- the VM is upgraded after the machines -- and the result says `signed: false`, so the daemon logs it; a node that has seen it sign gets a failed read instead. Sabotaged twice: the client falling back regardless, and an unsigned list reported as signed. The daemon's own call to `Contact::signed` is not under test: it lives in `one_round`, which only `acceptance-local.sh` runs. |
| **`a_contact_publishes_and_reads_on_one_connection_and_the_probe_agrees`** | `Contact` compares the address the UDP probe finds with the one it found last time; if the probe and the TCP connection named different addresses on an ordinary network, a machine that never moved would look moved every round and publish every round. On a real coordinator: the probe of `0.0.0.0:9797` is `127.0.0.1:9797`, the publication sends exactly that, the read on the same connection lists the Pi, and an `announce` is what the probe returns. |
| **`red_team_the_account_s_pledge_is_the_other_machines_as_the_coordinator_lists_them`** | The laptop learns what the account lends from the coordinator, through `Request::Devices`. The Pi claims 700 GB, the laptop 50 GB: the laptop must remember 700 GB for the others and reach 750 GB for the account. Counting its own entry as well would let it write past what the account earns; leaving the Pi out would hold it to the joining allowance. Sabotaged on the filter that leaves this machine out. |
| **`a_member_elsewhere_is_given_the_address_that_can_answer_first`** | The unit tests prove the ordering function orders; this proves the lookup *applies* it, and that what a member is handed is the announced address with its announced port. Deleting the call in `coordinator::peers` leaves every unit test green — the shape of a defence that is tested and not wired — and this fails, naming the order it got. |

---

# `itsanas-node` telling a member what is wrong (`tests/says_what_is_wrong.rs`) — end to end (3)

A real coordinator and a real node on real sockets. The unit tests prove the
decision refuses what it must and that a probe can tell one machine from
another; these prove the two halves are joined, and that what comes back is a
sentence somebody can act on.

| Test | What it proves |
| --- | --- |
| **`changing_the_announced_address_is_worth_asking_about_again`** | Found by using it, during the fleet migration of 2026-09-18: a node repointed at a new coordinator and given a new announced address could not confirm the new one, because the budget was keyed by device alone and the second question read as a repeat of the first. The budget exists to stop a daemon asking every round, not to stop somebody who just changed the thing being asked about. |
| **`red_team_asking_twice_in_a_row_does_not_cost_the_coordinator_twice`** | The budget, through the whole stack rather than in the limiter alone: the second ask inside the hour is answered from what is known, without another outbound connection. This is what makes the design affordable at three thousand machines. |
| **`a_member_whose_address_reaches_the_wrong_machine_is_told_which_way_it_is_wrong`** | The answer distinguishes "nothing resolves" from "nothing answers" from "something answered and it was not you", because those are three different evenings of work. |
| **`a_member_who_publishes_an_address_only_their_lan_can_dial_is_told_why`** | A private announced address comes back as **not checked** rather than as a verdict. The three states are not decoration: `Reachable`, `Unreachable` and `Unknown` were two states for half a day, and `doctor` on a perfectly reachable machine printed "NOTHING can reach this machine" followed by the reason — which was that it had been asked twice within the hour. Somebody reading the first line rewires a router that works. |

# `itsanas-coord` — admission (17 of the coordinator's unit tests)

`src/invitation.rs` and the invitation half of `src/directory.rs`. The front
door: until this existed, every other defence in the project — audits, the
reliability pause, the probation ladder, the keyed audit order — was aimed at a
hostile *host*, and a hostile host is somebody who joined.

| Test | What it proves |
| --- | --- |
| **`red_team_a_registration_that_fails_does_not_burn_the_invitation`** | Redeeming and creating the account were two transactions: spend the use, commit, then write the account. Anything failing after the first — and `NameTaken` is trivial to provoke on purpose and easy to hit by mistyping — destroyed the invitation and created nothing. Free denial of service against the inviter, and an invitee locked out by their own typing error. |
| **`red_team_the_coordinator_cannot_write_itself_an_invitation`** | The coordinator holds every invitation and every account. If it could mint one it would be the admission authority rather than a notice board, and ECONOMICS.md §7 is a promise that it is not. It may refuse — denial of service, already in the threat model — and must not be able to admit. |
| **`red_team_an_invitation_cannot_be_edited_after_signing`** | Every field is in the signed payload. If the expiry, the use count, the inviter or the code were outside it, an invitation for one machine on one afternoon would become an open door. |
| **`red_team_one_invitation_admits_one_stranger_however_many_try_it`** | A code posted in a group chat, or forwarded by the person it was sent to. Without spending uses, one endorsement admits everybody who ever saw it and "membership costs a member's endorsement" is false for every account after the first. |
| **`red_team_re_lodging_a_spent_invitation_does_not_refill_it`** | The way round the previous test. A client whose connection dropped re-sends what it signed; if lodging reset the counter an inviter could refill their own code for ever. |
| **`red_team_a_stranger_cannot_vouch_for_a_stranger`** | Otherwise invitation buys nothing: mint one keypair, sign invitations with it, admit as many accounts as you like. The endorsement has to come from somebody already inside. |
| **`red_team_the_founding_window_is_asked_for_and_shuts_by_itself`** | An attack the *fix* introduced. An invitation to admit the first member has no author, so something must open the door once — but if an empty directory always admitted its first caller, then on a public address the founder is whoever finds the port first, and the operator learns this by being refused from their own coordinator with a stranger inside holding the only account that can invite. The window is a flag the operator passes, and it still admits exactly one account. |
| **`every_refusal_reads_the_same_so_codes_cannot_be_enumerated`** | A coordinator that said "no such code" for one and "already spent" for another would let anybody probe which codes exist, and the codes are the thing keeping strangers out. |
| **`who_let_them_in_has_an_answer_afterwards`** | Attribution is what an endorsement is *for*. A member who admits forty accounts that all fail their audits has to be findable, or inviting is free in the only sense that matters. |
| **`a_member_re_registering_needs_no_new_invitation`** | Re-registering is how a member refreshes their agreement key and how a client retries a dropped connection. Demanding a fresh invitation for either would lock people out of their own accounts, on a coordinator whose whole job is letting them back in. |
| **`the_first_member_of_an_invite_only_coordinator_can_join`** | The chicken and the egg, handled rather than named. The first version required an invitation unconditionally and produced a coordinator that was running, reachable, correct in every detail and impossible to join. |
| `an_invited_stranger_joins_and_an_uninvited_one_does_not` | The ordinary path, both ways. |
| `an_expired_invitation_admits_nobody` | A code from last year is not still a way in. |
| `an_invitation_signed_by_a_member_verifies` / `the_secret_opens_its_own_invitation_and_no_other` | The primitives. |
| **`the_code_id_reveals_nothing_about_the_secret`** | The coordinator stores the hash, not the secret, so a stolen directory is a list of endorsements nobody can redeem. Two secrets differing in one bit must not produce related ids. |
| `an_invitation_good_for_nothing_is_refused_rather_than_stored` | Zero uses, or an expiry before the issue date. Neither can admit anybody, so storing them fills the directory with rows that exist only to be rejected. |

---

# `itsanas-folder` — what a pass tells the person reading it (`tests/reports.rs`) (6)

The store's view of a rename is a deletion and an addition, and that is what
replicates; nothing here changes it. What changes is the report `itsanas scan`,
`itsanas sync` and the daemon print: they name the files, and a move reads as
one move instead of "a file was lost and an unknown one appeared" -- the line
that sends somebody to restore a file they only moved. Functional tests: no
data or key is touched, each was sabotaged anyway (2026-10-01, seven sabotages,
each turned its test red). The last two tests come from the `itsanas-redteam`
pass on the diff: both were ways for the report to hide a deletion.

| Test | Why it exists |
|---|---|
| `a_file_renamed_in_the_folder_is_reported_as_one_rename` | A file moved into a subdirectory comes out as one `mv` line and "1 renamed" in the summary, not "1 in, 1 deleted locally"; the store still deleted the old path and holds the new one, byte for byte. Sabotage: no pairing, or the summary counting the pair twice. |
| `a_rename_made_on_another_device_is_reported_as_one_rename` | The same for a move another device made: the old file leaves this disk, the new one arrives, the report says it was renamed elsewhere. Sabotage: no pairing. |
| `only_an_unambiguous_move_of_the_same_bytes_is_called_a_rename` | Two identical copies deleted and one created, a file renamed *and* edited, and an empty file swapped for another empty one: none is called a rename, because the bytes do not say which file became which. Sabotage: pair the first candidate, or pair empty files. |
| `a_log_names_the_first_files_and_counts_the_rest` | The daemon's log names at most `LINES_IN_A_LOG` files and counts the rest in a last line, so a first pass over a full library does not flood it and nothing vanishes without a number; `scan` lists every one. Sabotage: drop the "and N more" line. |
| `deleting_one_of_two_copies_is_a_deletion_not_a_rename` | `a` and `c` hold the same bytes; `a` deleted and `c` copied to `b` is one gone and one come with those bytes, but `a` was a distinct file: the report names its deletion, not `mv a -> b`. A pair counts only when the new path is the only live one with its bytes. Sabotage: ignore the other live copy. |
| `a_bounded_log_still_names_every_deletion_and_conflict` | Thirty imports, a peer's deletion and a conflict in one pass: the bounded log names the deletion and the conflict and bounds only the imports. Before, the bound cut from the tail and both fell into "N more". Sabotage: one list cut as a whole. |

---

# `itsanas-folder` — storage that vanished (`tests/storage_vanished.rs`) (6)

The failure this file is about is not exotic and it destroys data on every
machine of an account at once. An unmounted disk, or a network share that
dropped, leaves its mount point behind as an **empty directory**: the scan finds
nothing, every file the ledger says this machine holds looks deleted, and those
deletions replicate. The disk comes back an hour later with the files still on
it, and the account has already agreed they were gone.

Nothing in the filesystem distinguishes that from a folder somebody emptied on
purpose, so there are two defences aimed at two shapes — a **marker** carrying
the device id, which goes away with the storage it sits on, and a **guard on the
count**, for when the directory really is there and most of it is not.

| Test | What it proves |
| --- | --- |
| **`red_team_an_unmounted_folder_writes_no_deletion_at_all`** | The accident itself: an empty mount point beside a ledger of three files. The pass refuses, names the cause, and the ledger still holds all three. Without the marker this writes three deletions into the log, where they replicate as deletions to every machine of the account. |
| **`red_team_a_folder_that_emptied_itself_has_its_deletions_held`** | The other shape: the directory is there, the marker with it, and six of seven files are not — a restore that wrote into the wrong place, a `rm -rf` in the wrong terminal. The deletions are held, nothing leaves the account, and `itsanas folder --confirm` is what applies them when somebody has looked. |
| **`red_team_a_folder_that_belongs_to_another_node_is_refused`** | Two nodes pointed at one directory is the other way a folder empties itself: each deletes what the other wrote. The marker names a device, so the second one can tell, and the refusal says whose folder it is. |
| `deleting_a_few_files_is_an_ordinary_thing_to_do` | Three files of seven go through untouched. A guard that held every deletion would teach its owner to pass `--confirm` out of habit, which is how a guard becomes a formality. |
| `a_folder_from_before_markers_is_adopted_rather_than_refused` | Upgrading must not stop anybody: a folder whose files are present gets a marker and carries on. |
| `the_marker_is_never_synced` | Syncing the marker would send one machine's device id to every other machine, where it would name the wrong device and make every folder look foreign. |

# `itsanas-coord` — claims kept in two orders (4)

Every "where are this account's machines" walked the whole claims table and
filtered, so one member's lookup cost O(devices in the entire network) and the
coordinator's work grew with the square of the fleet: 2.58 ms per lookup at 3000
devices, against about 10 lookups a second for a fleet that size. A second table
keyed by **account then device** makes it one range scan, ~8 µs, flat. The
numbers are in ROADMAP "Known ceilings".

Denormalised data is only defensible when every way it can disagree with itself
is pinned down, which is what these three do.

| Test | What it proves |
| --- | --- |
| **`red_team_one_accounts_range_cannot_reach_into_the_next_accounts_devices`** | One account's devices are now a *range* rather than a filtered scan, so the filter **is** the key layout. Get the boundary wrong and a member's lookup returns the neighbouring account's machines — a privacy failure and an address book that tells people to dial strangers. Tested on the property that makes the range safe, with adjacent account ids built on purpose: every key of account *n* sorts below every key of account *n+1*, whatever devices either holds. |
| **`the_index_and_the_claims_never_disagree_whatever_is_done_to_them`** | Two tables answering one question must never drift: a first enrolment, a superseding claim, a withdrawal, and a second account's machine that must not appear. Compares the indexed lookup against the whole-table walk it replaced. Catches a write path that updates one table and not the other, which reads to a member as their machines vanishing from the address book. |
| **`a_device_enrolled_by_an_older_binary_is_found_again_at_the_next_start`** | The operation that breaks a repair conditioned on "the index is empty", and it is one that has been performed on the Pi: **downgrading the coordinator binary**. An older build enrols devices by writing the claims table and knowing nothing of the index, so coming back up the index is *stale* rather than empty and the repair would skip. Those machines would be permanently invisible — `claim_for` knows them, they announce, and `peers_of` never returns them. The condition is row counts disagreeing, which covers empty and stale alike. Found by the Rodin audit of 2026-09-18. |
| **`a_directory_written_before_the_index_existed_is_repaired_on_open`** | The upgrade. A coordinator running since before this table has claims and no index; reading that as "this account has no devices" would be silent and total — every member told their machines are gone, and the only clue being that it started at an upgrade. The file is repaired on open instead, the same rule as the holder ledger's second ordering. |

# `itsanas-coord` — asking whether a member can be reached (12)

The one question a machine cannot answer about itself: a node knows it reached
the coordinator, because it just did, and nothing tells it whether anything can
come back. A member whose port forward is wrong looks, to every other member,
exactly like a member who is switched off — so until this existed the failure
was invisible on both sides.

It is also the only request that makes a coordinator *act* on the internet
rather than answer about it, which is why half of these tests are about what it
refuses to do.

| Test | What it proves |
| --- | --- |
| **`red_team_a_probe_cannot_be_aimed_at_the_coordinators_own_network`** | A coordinator that dials what a caller names is a port scanner with somebody else's address on it. It dials only what the caller **announced**, which carries that device's own signature, and never a private, loopback, link-local or CGNAT address — which would be a scan of the coordinator's own LAN, the one network a member has no business reaching. Checks six ranges a home actually uses, and the three public shapes the feature exists for. |
| **`red_team_a_device_with_no_account_cannot_make_the_coordinator_dial_anything`** | Device keys are free keypairs, so completing a handshake identifies a caller and vouches for nothing. The refusal has to name *enrolment*: an unenrolled device also has no presence, so "there is nothing to probe" refuses it by accident today and would stop doing so the day anything else writes a presence. |
| **`a_withdrawn_device_stops_being_probed_for`** | Withdrawal is the account saying a machine no longer speaks for it. A stolen laptop keeps its key, and must stop buying the coordinator's outbound connections with it. |
| **`a_device_that_never_announced_is_told_so_rather_than_probed`** | There is nothing to probe, and the alternative — letting the caller supply an address — is the scanner above. |
| **`red_team_asking_to_be_probed_again_and_again_buys_one_probe_an_hour`** | Without a budget, a member's daemon turns every round into an outbound connection the coordinator pays for, which is the load this design exists to keep off it. Also checks that the window *reopens*: a machine that really did move must be able to find out it is reachable again. |
| **`red_team_a_probe_that_reaches_a_different_machine_is_not_a_success`** | The reason the probe is a device-authenticated handshake and not a connection. A forward pointing at the wrong host — the other Pi, a printer, a neighbour on the same public address — is an open port, and reporting it as success tells a member their setup works while every peer that dials them is refused by the device pinning. |
| **`a_probe_slot_is_given_back_however_the_probe_ends`** | A slot not returned is a limit that shrinks to zero, and every probe of an unreachable member is a failure path. **It found a real one**: the guard was built with `then_some`, which evaluates its argument eagerly, so a refused slot was constructed and immediately dropped — the counter underflowed to `usize::MAX` and the next probe panicked on the increment. |
| `a_probe_that_reaches_the_device_says_so` | The happy path, against a real TLS listener: reachable means *this device answered here*. |
| `a_probe_of_an_address_where_nothing_listens_says_nothing_answered` | The answer names what a person should go and look at — a forward, a firewall — rather than reporting a number. |
| `a_probe_of_a_name_that_does_not_resolve_says_that_rather_than_timing_out` | DNS and a closed port are different problems, fixed in different places, and must read differently. |
| **`red_team_a_name_that_resolves_into_a_private_network_is_not_dialled`** | The bypass, and it was live for an afternoon: the guard on the announced string treats a *name* as public — correctly, since what it resolves to is the resolver's business — and the dial then resolves it. `nas.example.org` pointing at `192.168.1.10` walked through a guard written to stop exactly that. The check that counts is on the resolved address, before any socket. The sabotage run that proved the hole reached the real daemon on the machine the test ran on. |
| `measure_what_one_lookup_costs_across_a_fleet` | `#[ignore]`d, and a measurement rather than an assertion: what one address lookup costs with 0, 500, 1500 and 3000 devices in the directory. The numbers are in ROADMAP "Known ceilings"; they exist so the decision to index claims by account is made against a number rather than an intuition. |

# `itsanas-coord` — the coordinator server (24)

Fifteen integration tests in `tests/coordinator.rs` and nine unit tests beside
the code. A real coordinator on a real socket: real directory, real TLS with
device authentication, real signatures, real framing.

One number in the heading, and the split in prose, so that
`scripts/check-counts.py` can add the headings up: the sum of every heading
naming a crate has to equal that crate's real test count, and a heading carrying
two numbers made `itsanas-coord` look eight short.

| Test | What it proves |
| --- | --- |
| **`red_team_a_trickled_handshake_is_cut_off_by_the_coordinator`** | Every member dials this one address, so a slot held by a caller trickling its handshake is a slot no member gets. The deadline is unit-tested in `itsanas-tls`; this proves the coordinator applies it. Fails when the coordinator ignores its deadline. |
| **`a_member_s_device_list_includes_a_machine_that_has_gone_quiet`** | `device list` was built on `Peers`, which drops a device silent for a week — the lost laptop the list is opened to find. Every live enrolment is listed, heard-from first; one that never announced reads as never heard from rather than fresh; a withdrawn one is absent. |
| **`red_team_a_stranger_cannot_list_another_member_s_devices`** | Another member and an unenrolled keypair are both refused the list of pledges and silences. |
| **`red_team_coordinator_messages_keep_their_wire_numbers`** | Every request and response is written under the number deployed coordinators and clients already read it by; an exhaustive match stops a new variant compiling until it is numbered. |
| **`escrow_is_stored_by_an_enrolled_device_and_recovered_by_name_alone`** | MVP acceptance test D at the protocol layer. A machine with no device, no account and no key fetches the sealed container using only the username, and the passphrase is what opens it. |
| **`red_team_reconnecting_does_not_reset_the_escrow_attempt_budget`** | The escrow blob is the one thing reachable without proving anything, so the rate limit is the whole defence. A per-connection counter would be no counter: an attacker reconnects, pays one handshake, and works through a word list. |
| **`red_team_an_unenrolled_device_cannot_overwrite_someone_elses_escrow`** | Replacing a member's container with one whose passphrase you chose would either take their account or — quieter — destroy their ability to recover, discovered on the day they needed it. |
| **`red_team_a_device_cannot_publish_an_address_for_a_device_it_does_not_own`** | Announcing somebody else's device at an address you control black-holes their machines. TLS pinning stops data being exposed; nothing else stops the denial of service. |
| **`red_team_a_name_cannot_be_taken_over_by_a_different_key`** | A username pointing at the wrong key sends everyone looking that member up to an impostor. |
| **`red_team_an_oversized_username_is_refused_before_the_directory_sees_it`** | Nothing downstream has to be robust against a caller-chosen length. |
| `escrow_is_off_until_a_blob_is_stored_and_can_be_withdrawn_again` | Passphrase recovery is a trade, so it is opt-in *and* reversible. Without the second half the only safe choice would be never to use it. |
| `a_member_registers_enrols_a_device_and_is_then_findable_by_name` | The ordinary path end to end: after this, somebody who knows only a username can reach the machines. |
| `a_connection_that_asks_too_much_is_told_why_rather_than_cut_off` | A silent close surfaces as "connection aborted by your host software", which reads like a firewall and sends whoever is debugging it an hour in the wrong direction. |
| `the_peer_list_is_bounded_however_many_devices_a_user_enrols` | A member with a thousand devices is not a way to make the coordinator send a thousand records to anybody who asks. |
| `a_version_mismatch_is_refused_rather_than_guessed_at` / `asking_about_an_unknown_name_says_so_rather_than_inventing_one` | No optimistic guessing, no invented answers. |

Unit tests in `service.rs` and `protocol.rs` cover the limiter's arithmetic and
the open-request list:

| Test | What it proves |
| --- | --- |
| **`red_team_grinding_one_account_is_cut_off_after_a_few_attempts`** | The limiter does what the whole centralisation argument rests on. |
| **`red_team_flooding_invented_names_cannot_reset_a_real_account_counter`** | The limiter is itself a table a stranger writes into. A full table that evicted the oldest entry would let an attacker clear their own counter by inventing names. |
| **`only_hello_and_escrow_retrieval_are_reachable_without_proving_anything`** | The hostile-internet argument rests on this list being two items long, so it is asserted rather than described. |
| `a_log_line_never_contains_anything_a_caller_wrote` | Otherwise a stranger picks their username and writes into the operator's journal. |
| `the_budget_comes_back_after_the_window` | Somebody who mistypes five times is not locked out of their own account for good. |
| `expired_windows_are_forgotten_so_the_table_does_not_fill_with_history` / `a_name_that_was_never_asked_about_is_allowed_once_the_table_has_room` | The bound holds without leaking history. |
| `a_request_round_trips_through_postcard` | The encoding. |

---

# `itsanas-wire` — framing (17)

Every byte parsed here comes from a stranger's computer.

| Test | What it proves |
| --- | --- |
| **`an_oversized_length_is_rejected_before_anything_is_allocated`** | Five bytes on the wire asking the peer to reserve four gigabytes. On a Raspberry Pi a handful of these is fatal. |
| **`every_truncation_of_a_valid_frame_is_an_error_and_never_a_panic`** | Every prefix of a valid frame, rejected rather than half-parsed. |
| **`corrupting_any_byte_never_panics`** | Every single-bit corruption of every byte. The decoder may reject; it may not abort the process. |
| **`arbitrary_garbage_never_panics`** | Random bytes fed to both the one-shot decoder and the streaming reader. |
| **`the_reader_does_not_grow_without_bound_on_a_stalled_frame`** | A peer that sends a header then trickles bytes forever cannot make the buffer exceed one maximum frame. |
| **`a_frame_split_across_reads_is_reassembled`** | The normal case on a real stream, byte by byte. |
| **`an_unknown_wire_version_is_refused_not_guessed_at`** | No silent reinterpretation of a future format. |
| `a_frame_exactly_at_the_limit_is_accepted_and_one_byte_over_is_not` | The boundary is where it is documented to be. |
| `several_frames_in_one_read_are_all_returned` | Batched arrivals are all delivered, leaving nothing buffered. |
| `the_header_is_exactly_as_documented` | The layout matches the doc comment. |
| `a_frame_round_trips` / `an_empty_payload_is_a_valid_frame` | The basic paths. |

The five remaining tests cover `Connection`, the generic `Read + Write` wrapper:

| Test | What it proves |
| --- | --- |
| **`a_close_part_way_through_a_message_is_an_error`** | A peer that hangs up mid-frame must not have its partial response treated as complete. This is the difference between a truncated answer and a short one. |
| `a_clean_close_between_messages_is_not_an_error` | The legitimate case is not turned into a failure. |
| `an_oversized_frame_is_refused_rather_than_buffered_towards` | The limit is enforced on the streaming path too, not only the one-shot decoder. |
| `a_message_round_trips` / `several_messages_come_back_in_order` | The basic paths, and that nothing is left buffered between messages. |

---

# `itsanas-tls` — device authentication and listener limits (24)

Six unit tests in `auth.rs`, two in `session.rs`, six in `limits.rs`, five in
`reach.rs`, five integration tests in `tests/handshake.rs`.

| Test | What it proves |
| --- | --- |
| **`a_proof_from_one_session_is_worthless_in_another`** | The property the whole transport rests on. A man in the middle who terminates TLS has two sessions with two different exporters, so a captured proof cannot be relayed into the other one. If this test is ever weakened, authentication becomes replayable and nothing else in the crate catches it. |
| **`the_payload_never_reaches_the_socket_in_plaintext`** | Records every byte actually written to the socket and scans it for a canary. Proves the encryption is on the wire, not merely configured. |
| **`dialling_a_device_and_reaching_a_different_one_is_refused`** / `dialling_a_known_peer_refuses_a_different_answer` | An address that resolves to the wrong machine is refused rather than trusted — the coordinator hands out addresses and is not trusted to say who lives at one. Tested at both the proof layer and the socket layer. |
| `claiming_to_be_another_device_fails` / `a_tampered_signature_is_refused` | The two direct forgeries. |
| `an_honest_proof_identifies_the_device` / `the_proof_round_trips_through_the_wire` | The mechanism works, and works through framing. |
| `a_server_learns_who_called_without_being_told_in_advance` | A node can serve a device it has never met, which is what lets anyone offer storage. |
| `every_process_presents_a_different_certificate` | Certificates are anonymous and disposable, so an observer cannot correlate two connections by them. |
| `two_devices_authenticate_each_other_and_exchange_a_message` | End to end over a real socket. |
| **`red_team_a_handshake_trickled_a_byte_at_a_time_is_cut_off_at_the_deadline`** | A read timeout bounds one read, not a handshake: a caller sending a byte a little inside it holds a server slot for ever. `accept_within` gives every read and write only what is left of a total deadline. Fails when the deadline is ignored. |
| **`the_deadline_is_lifted_once_the_caller_has_authenticated`** | The deadline is for proving who you are. The first version restored the idle timeout through a clone of the socket, and on Windows a duplicated socket handle does not share its timeouts, so an authenticated peer was cut off a few hundred milliseconds in. This test found it. |
| **`red_team_one_address_cannot_take_every_slot`** | One machine opening connections until the global cap is reached leaves the listener up and serving nobody else. At most `per_address` from one IP address. |
| **`red_team_one_device_key_cannot_hold_more_than_its_share`** | The same, for a proven device key dialling from several addresses. |
| **`red_team_one_ipv6_subnet_cannot_take_every_slot_by_changing_address`** | A household on IPv6 owns a /64 and can give every connection its own source address, so counting single addresses stops nobody. IPv6 is counted by its /64. |
| `an_ipv4_caller_is_one_caller_however_it_arrives` | A dual-stack listener sees IPv4 callers as `::ffff:a.b.c.d`; they are counted as the IPv4 address, not as a second caller. |
| `the_overall_cap_holds_across_addresses` | The global cap is a cap. |
| **`a_closed_connection_gives_its_slot_back_even_after_a_panic`** | A slot that is never returned is a limit that shrinks, one crash at a time, until the listener serves nobody. |
| **`a_second_listener_cannot_take_a_port_this_one_holds`** | `SO_REUSEADDR` on Windows does not mean what it means on Unix: it lets a *second* process bind a port a first one already holds and take its traffic, and `SO_EXCLUSIVEADDRUSE` — which `TcpListener::bind` sets — is what stops it. A socket built by hand to clear `IPV6_V6ONLY` does not inherit that, and `socket2` 0.6 exposes no safe way to set it. **This test failed the moment it was written**, on the first version of `bind_dual_stack`, which is why Windows now keeps the plain IPv4 listener rather than the dual-stack one. |
| **`a_node_asked_for_every_interface_is_reachable_over_ipv6_too`** | `listen = 0.0.0.0:9797` is the default and it is IPv4 only, so every IPv6 caller was refused by a node that believed it was accepting from everywhere. IPv6 is the one route between two houses that needs no port forward and costs nothing per machine, so an IPv4-only listener cannot use the cheapest path there is. A wildcard bind takes a dual-stack socket; on a machine with no IPv6 the test asserts the fallback instead, because refusing to start there would be a regression in exchange for a reachability that machine cannot have. |
| **`an_address_that_does_not_answer_does_not_hide_the_one_that_does`** | A dual-stack name resolves to an AAAA record *and* an A record, and plenty of networks drop IPv6. Connecting to only the first resolved address turned "one of two routes is shut" into "the peer is down". Fails when the dialler stops at the first address. |
| `an_address_that_names_one_interface_is_not_widened_to_all_of_them` | The dual-stack substitution happens only where the configuration said "anywhere". Widening `127.0.0.1` would put a node meant to be private on every interface of the machine. |
| `nothing_answering_anywhere_is_still_an_error` | Trying several addresses must still fail when none answers, rather than returning the last error as success or hanging on an empty list. |

---

# `itsanas-coord` — five live devices per account (6)

Decided by Nicolas on 2026-09-30. Five unit tests in `directory.rs` and one
integration test in `tests/coordinator.rs`. `MAX_DEVICES_PER_ACCOUNT` in
`claim.rs`; `Directory::bound_devices(false)` plays a coordinator older than
the bound, behind `cfg(test)` and the `hostile` feature.

| Test | What it proves |
| --- | --- |
| **`red_team_a_sixth_device_is_refused_and_nothing_is_written`** | A sixth live claim is refused as `TooManyDevices`, naming the five and `itsanas device forget <id>`, and neither table changes: a written claim would be listed to every machine of the account. Sabotaged on the count (`if false`) and on the command in the message. |
| **`red_team_a_withdrawn_slot_lets_one_more_in_and_the_withdrawn_device_stays_out`** | One withdrawal frees one slot, not two; the withdrawn device's live claim re-signed after it is refused, stays withdrawn and is not counted. Sabotaged by counting withdrawn claims as live. |
| **`red_team_re_signing_a_live_device_on_a_full_account_takes_no_slot`** | A new claim for a device already live (a pledge change, `register` again) is kept on a full account. Counting it would lock a full account out of every pledge change. Sabotaged by counting every live claim. |
| `an_account_already_above_the_bound_keeps_its_devices_and_cannot_add_one` | Seven devices enrolled by an older coordinator stay live and can re-sign; an eighth is refused with the real count. The bound is not applied backwards. |
| `a_full_account_does_not_stop_another_account_enrolling` | The count is per account, read through the owner index range. |
| **`red_team_a_client_older_than_the_bound_is_refused_in_words_on_the_wire`** | Mixed versions: a sixth claim sent as any client sends it gets `Response::Refused` at once, with the device ids and `itsanas device forget` in the text an old client prints, and nothing written. Not a hang-up an old client would retry, not a `Done`. |

# `itsanas-coord` — forgetting an account (2)

Added 2026-10-05, after a test account named `mandarine` held its name for
good and nothing could free it. `Directory::forget_account`, run by
`itsanas-coordinator --forget-account NAME` with the coordinator stopped.

| Test | What it proves |
|---|---|
| **`a_forgotten_account_frees_its_name_and_its_live_devices`** | The name can be registered again under another key, and the old account's live claim, escrow and id lookup are gone, so the coordinator hands out nothing of it. Forgetting a name nobody holds answers "none" rather than failing. |
| **`red_team_forgetting_an_account_keeps_its_withdrawals_final`** | A withdrawal is final for its device id (HANDOVER §6). If forgetting erased it, the same keys registering again would bring a stolen, withdrawn machine back with a fresh live claim. Sabotaged by dropping withdrawals with the rest: red. |

# `itsanas-node` five devices (`tests/five_devices.rs`) — the client's half of the bound (4)

| Test | What it proves |
| --- | --- |
| **`red_team_a_sixth_machine_refuses_to_enrol_itself_even_where_the_coordinator_would_not`** | Against a coordinator that would admit it, a sixth machine of the account refuses before it signs, names the five devices it read from `ClaimedPeers`, and the coordinator holds no claim for it. Sabotaged by ignoring the client's check, and by not reading `ClaimedPeers` on a machine not yet enrolled. Also: `coordinator::cap_named` reads all five full ids back out of the refusal, which is how the Android app puts a Withdraw button beside each (sabotaged by matching 12-character words: red). |
| **`red_team_a_machine_of_a_full_account_can_register_again`** | `itsanas register` on a live device of a full account succeeds: `Devices` answers only a live device, so an answer means a re-signing and the client does not count (not "am I in the list": the list is truncated, silent devices last, which `itsanas-redteam` showed could refuse a long-silent device of an account above the bound). A regression guard; the "already live" rule of `room_for` is sabotaged red in the next test but one. |
| **`red_team_a_new_machine_of_a_full_account_whose_machines_are_all_lost_can_free_a_slot`** | Found by `itsanas-redteam`: five machines lost or reinstalled hold the five slots, and the new machine is the only one left and is not enrolled. It is refused, the coordinator's refusal carries full device ids (a short one cannot be resolved by a machine refused `Devices`), a withdrawal by full id is accepted from it, and the freed slot takes it. Sabotaged by short ids in the refusal. The CLI half -- `itsanas device forget <full id>` no longer lists first -- was checked by hand against a real coordinator both ways (refused "only an enrolled device" without the change), not by a test. |
| `room_is_counted_in_live_devices_and_a_device_already_live_needs_none` | `coordinator::room_for` alone: four leave room, five do not, and a device among the five always has room. |

# `itsanas-coord` — departures (3)

A device leaving on purpose tells the coordinator, which records it apart from a
silence for a regulation that does not exist yet. Nothing reads the record today.

| Test | What it proves |
| --- | --- |
| **`red_team_a_departure_notice_from_another_device_is_refused`** | A history anybody can write about somebody else is worthless to the regulation it is kept for. Two ways in, both refused with nothing recorded: a genuine notice delivered over another device's connection (a replay), and a notice naming a device and signed by another key (a forgery). Then the device itself is heard, so the refusals are not an accident of a broken path. |
| **`red_team_a_departed_device_keeps_its_slot`** | `itsanas leave` is not a withdrawal: on a full account, the departed device keeps its live claim and a sixth device is still refused with `TooManyDevices`. Freeing the slot is `itsanas device forget <id>` from another device, which `leave` now prints. Sabotaged (departure deleting the claim): red. |
| `a_departure_is_recorded_apart_from_a_silence` | The only reason to record departures is to keep them apart from silences: a device that stopped announcing without a word has none on record. Recording one changes nothing else yet — the device's last presence stands. |

# `itsanas-coord` — is this device still enrolled? (3)

§8 1c (ii). `Request::Standing` carries a claim the owner signed and is
answered with the coordinator's own claim for that device, so a host learns of
a withdrawal it could not otherwise know of. In `service.rs`.

| Test | What it proves |
| --- | --- |
| **`red_team_standing_answers_the_withdrawal_not_the_claim_presented`** | A withdrawn device re-signs a live claim and presents it; the coordinator answers with the withdrawal it holds. Sabotaged by echoing the presented claim. |
| **`red_team_standing_tells_nothing_about_a_device_the_caller_was_not_shown`** | A claim that does not verify is refused, and one signed under another account than the device's own is answered `None`: the question cannot be asked about a device the caller was never shown. Sabotaged by dropping either check. |
| `standing_answers_a_live_device_with_its_claim_and_an_unknown_one_with_nothing` | The ordinary answers: a live device's claim, and nothing for a device never enrolled. |

# `itsanas-node` withdrawals (`tests/withdrawals.rs`) — the host's half, against a real coordinator (4)

| Test | What it proves |
| --- | --- |
| **`red_team_a_withdrawn_device_that_re_signs_stores_nothing_on_a_host`** | End to end: a device confirmed live stores; withdrawn from a sibling, it signs a fresh live claim, and a host that meets it asks the coordinator (`coordinator::standing`) and refuses it with `WITHDRAWN`. Sabotaged by `standing` never asking, and by the coordinator echoing the presented claim. |
| **`red_team_a_host_whose_coordinator_does_not_answer_stores_for_no_other_account`** | Nicolas's rule over a real socket: the coordinator is gone, `standing` fails, and the member's device is refused with `UNCONFIRMED`. Sabotaged by `standing` never asking. |
| `a_host_that_asks_when_a_device_presents_lets_it_store_on_its_first_round` | The inline asker against a real coordinator: a member presenting for the first time stores in the same connection. Red when the asker never asks. |
| **`red_team_a_host_whose_coordinator_is_not_pinned_stores_for_no_other_account`** | Found by the redteam agent: unpinned, the withdrawn device on the path answers "live" by echoing its own claim. Neither the round (`report.unpinned`, nothing asked) nor the inline ask asks an unpinned coordinator, and the member is refused `UNCONFIRMED`. Sabotaged by dropping `pinned` from each. |

# `itsanas-coord` — claims, directory, accounting (64)

Catalogued by property rather than test by test: the crate is a library with no
server yet, and what matters is which rule each group of tests pins down.

**Claims and revocation.** Device claims cannot be forged, retimed or stripped of
their revocation; a claim dated far in the future is refused, because
supersession is by timestamp and such a claim could never be replaced; replaying
an old enrolment cannot un-revoke a stolen laptop; presence is signed by the
device and a claim by the owner, so a laptop changing networks never needs the
key that can revoke everything; a username cannot be taken over by another key
and re-registering cannot reset the joining date; a device cannot be claimed
without an account or by two accounts; **a node cannot inflate its own
availability by saying so** — a single heartbeat buys only the floor; a
coordinator that was itself offline for a year does not annihilate everyone's
standing; escrow is off until asked for; the accounting floors entitlement
against the member, clamps availability at both ends, and permits reclaiming
only in the harshest state.

Three of them are catalogued one by one, because they correct what the
paragraph above used to promise — that a stolen laptop cannot un-revoke itself.
It could, and "replaying an old enrolment" was the only case tested.

| Test | What it proves |
| --- | --- |
| **`red_team_a_machine_holding_the_master_key_cannot_bring_a_withdrawn_device_back`** | A claim dated after a withdrawal, signed with the master secret every keystore holds, is refused out loud and the device stays out of the live set. |
| **`red_team_a_withdrawal_signed_on_a_slow_clock_still_withdraws`** | A withdrawal wins over a live claim whatever the two signing clocks say. |
| **`a_later_enrolment_does_not_supersede_a_withdrawal`** | The same rule at `supersedes`, so the directory's refusal is not the only thing keeping a withdrawn device out. |

One more, alone because it is the check a *reader* makes rather than the coordinator:

| Test | What it proves |
| --- | --- |
| `a_relayed_presence_is_checked_for_its_signer_and_not_its_date` | `SignedPresence::verify_origin`, for a presence read second-hand: an address changed after signing fails, and a genuine presence passes for a reader whose clock says 1970, where `verify` -- the coordinator's check on arrival, against its own clock -- refuses it as from the future. |
| `a_claimed_presence_of_this_account_is_kept_whatever_the_readers_clock` | `ClaimedPresence::verify_for`, the check made on each row of `ClaimedPeers` (§8 0o phase 2b.3): a genuine presence with its owner's live claim passes for a reader whose clock says 1970, where the dated `SignedClaim::verify` refuses the same claim. |
| **`red_team_a_relay_cannot_pass_off_another_accounts_machine_as_yours`** | A signed presence says where a machine is, not whose. Another account's machine with both signatures genuine is refused because its claim names another owner. Kept, it fills the address book with machines this one cannot sync with and gossip would hand it on. Sabotaged by skipping the owner comparison. |
| **`red_team_a_claim_cannot_vouch_for_a_different_device`** | This account's genuine claim on one machine, paired with a stranger's genuine presence, is refused: the claim must name the presence's device. Sabotaged by skipping that comparison. |
| **`red_team_a_withdrawn_device_is_not_relayed_as_live`** | A withdrawn laptop keeps its keys and can still sign presences; its owner's withdrawal must keep it out. Sabotaged by ignoring `revoked`. |
| **`red_team_a_claim_with_a_forged_owner_signature_is_refused`** | Another account's claim with its owner field rewritten to this account fails the owner's signature. Sabotaged by not checking the claim's signature. |

**The two halves of the space bargain agree.** `itsanas space` and both
provisioners refuse a `--keep` larger than the pledge earns, and the refusal
quotes what would be needed instead — so `Split::room_earned` decides and
`Split::pledge_needed_for` writes the sentence. Since 2026-09-14 the rule is a
`Split` rather than a single `CONTRIBUTION_RATIO`, which means both ends of that
sentence round and the default is a number somebody can change; these are
catalogued by name because they hold a rule two other programs depend on:

| Test | What it pins down |
| --- | --- |
| **`the_limit_and_the_price_quoted_for_exceeding_it_never_contradict`** | The worst kind of instruction: somebody is told "keeping 31 GiB needs 93 GiB pledged", pledges exactly 93, and meets the same sentence again. Two functions write that message between them and integer division truncates, so the property has to hold at every remainder. It was nearly free while the ratio was 3, because the quote was a multiplication by it and could not round the wrong way; at 30/70 the quote divides too, and quoting the floor rather than the ceiling breaks it at almost every input. Checked in both directions and across seven splits, because a rule that holds only at the shipped number is not a rule. |
| **`the_quote_saturates_rather_than_wrapping_on_an_absurd_request`** | Where that stops being true. Past what the split can price, the quote saturates and understates what would be needed. The refusal is still correct there and the free-space check refuses such a number anyway, so this is a boundary written down rather than a bug left open — but arithmetic that wrapped instead would turn an absurd request into a small one and let it through. Both ends are checked now: a split may earn *more* than it is given, so the earning side can leave a `u64` as well. |
| **`the_default_split_is_thirty_seventy`** | The one number the rest of the file applies, pinned — and it has already moved once, from the 25/75 that `CONTRIBUTION_RATIO = 3` encoded. Stated twice, as the constant and as the bytes it produces, so that a change to the arithmetic and not to the constant fails here too. What it catches is a tidy-up back to 25/75, which takes a sixth of everybody's entitlement away without touching a line of arithmetic. |
| **`red_team_a_split_with_a_zero_part_is_refused_rather_than_dividing_by_zero`** | `network = 0` divides by zero inside `pledge_needed_for`. That is a panic in whatever holds the store's exclusive lock, which on the Pi is the daemon, and the daemon restarts into the same configuration file. `own = 0` is quieter and worse: every pledge earns nothing, every `itsanas keep` is refused, and the machine reads as broken rather than as misconfigured. Both doors are checked — `Split::new` and the written `own/network` form — because a validated type with an unvalidated parser is not validated. |
| **`a_split_survives_the_form_it_is_written_in`** | `Display` writes the configuration file and `parse` reads it back. If the two disagree, a node saves its own settings and then refuses to start on them, and the setting it breaks on is the one the whole bargain rests on. Includes the spaces a person editing the file by hand leaves behind. |
| **`red_team_entitlement_follows_the_coordinator_s_split_not_a_device_s`** | Two splits exist and they are not the same thing: the one in a node's configuration file decides what that machine refuses to its own owner, and the one `assess` is given decides what the network grants. What this catches is a `split` field added to `DeviceContribution` — the struct a device fills in about itself — and read by `assess`. If a member's number ever reaches that arithmetic, widening an entitlement costs one line of a text file and the bargain is decoration. |
| **`red_team_a_second_username_cannot_renew_the_joining_allowance`** | `register_admitted` answered two questions from two tables: "has this key been here before?" from BY_ID, and "does this account exist?" from ACCOUNTS keyed by *name*. They agree until one key asks for a second name — then the key counts as returning, so no invitation is demanded, and the name is unknown, so the branch that preserves the account's registration date is skipped and a fresh account is minted with today's date. One signed message every thirty days turned a bounded 10 GiB joining allowance into a permanent free tier. **The two sibling tests covered (same key, same name) and (different key, same name); nobody wrote (same key, different name)**, and this page recorded the property as established. Same shape as the freshness guard that lived in one branch of three. |
| **`red_team_one_admitted_key_cannot_mint_accounts_on_an_invite_only_coordinator`** | The same defect on its other axis. The invitation gate is skipped for a key that already has an account — right for somebody re-registering the name they hold, wrong for anything else. An admitted member could open unlimited accounts with no invitation, and usernames here are bound to a key for ever with no release path, so one member could squat every short name on the coordinator. |

---

# `itsanas-cli` — setup run twice (1, `#[ignore]`d)

`tests/setup.rs`. Runs the real binary twice with `--answers` (service off) in
a temporary home. Ignored because each run pays a full Argon2id derivation;
the `slow-tests` CI job runs it in release.

| Test | What it proves |
| --- | --- |
| `setup_run_twice_from_answers_changes_nothing` | (ignored, release) The binary run twice with --answers (service = false) in a temp home: the second run shows no new words and leaves the device id, keystore and config byte for byte; Sabotaged (Account check skipped): red. |

---

# `itsanas-release` — signed release manifests (14)

`crates/itsanas-release` (`tests/release.rs`): the manifest (`itsanas-release 1`,
one `file` line per target with size, BLAKE3 and SHA-256), its Ed25519
signature, the sealed signing key and the `itsanas-release` binary used by
`scripts/sign-release.*` and `.github/workflows/release.yml`. No node calls
`verify_release` yet (self-update is 0t part 4), and `RELEASE_KEY` is `None`
until Nicolas pins his key, so today every verification is refused.

| Test | What it proves |
| --- | --- |
| `a_release_signed_by_the_trusted_key_is_accepted_end_to_end` | The honest path holds: manifest from a dir, sealed key signs it, verify_release + check_file accept the real binary (a refusal here would block every tester's update). |
| **`red_team_a_manifest_signed_by_another_key_is_refused`** | A release signed by anyone but the trusted key is refused (else anyone could push binaries to every node); Sabotaged (signature check accepts any key): red. |
| **`red_team_one_changed_byte_in_a_signed_manifest_is_refused`** | One byte altered in a signed manifest (e.g. a hash swapped for a trojan's) is refused; Sabotaged (signature check accepts any key): red. |
| **`red_team_an_older_or_equal_version_is_refused_as_a_downgrade`** | A genuinely signed but older or equal release is refused, so an attacker cannot replay an old vulnerable binary; Sabotaged (<= turned into <; version fields reordered): red. |
| **`red_team_a_file_whose_size_or_hashes_do_not_match_is_refused`** | A downloaded binary whose size, BLAKE3 or SHA-256 differs from the signed manifest is refused, not installed; Sabotaged (BLAKE3 check off; SHA-256 check off): red. |
| **`red_team_a_truncated_download_is_refused`** | A download cut short is refused before install, so a flaky network never leaves a half binary in place; Sabotaged (size check accepts smaller files): red. |
| **`red_team_the_key_file_with_a_wrong_passphrase_is_refused`** | A stolen key file without the passphrase (or a tampered one) signs nothing and fails in one plain line; Sabotaged (unlock failure falls back to a zero key): red. |
| **`red_team_with_no_release_key_pinned_every_manifest_is_refused`** | A build with no release key pinned refuses every manifest with "no release key pinned yet" instead of a misleading forgery error; Sabotaged (None yields an empty trust): red. |
| `the_release_key_is_pinned_until_nicolas_changes_it_on_purpose` | RELEASE_KEY's value is pinned, so changing the key every node trusts is a visible decision in a diff; Sabotaged (constant changed): red. |
| `a_next_key_named_by_a_signed_manifest_is_trusted_by_learn` | `Trust::learn` adds a key named by a manifest the old key signed, for that `Trust` value. Rotation itself is not built: nothing calls `learn` outside tests, nothing persists a learned key, and the old key is never dropped; Sabotaged (next key not stored): red. |
| **`red_team_a_draft_with_one_binary_byte_flipped_is_not_signable`** | `itsanas-release check` (run by `sign-release.sh`/`.ps1` before the key is touched) refuses a draft whose binary differs from its manifest by one byte, or is missing, so a draft edited by anyone with repository write access is never signed; Sabotaged (file check skipped): red. |
| **`red_team_a_draft_whose_manifest_version_is_not_the_tag_is_not_signable`** | A draft of tag v0.3.0 carrying a manifest for 0.2.0 is refused before signing, so a replayed or hand-made manifest is not signed; Sabotaged (version comparison off): red. |
| `a_manifest_names_only_files_derived_from_their_target` | A manifest line whose file name is not itsanas-<target>[.exe] is refused, so a signed manifest cannot point an installer at another file; Sabotaged (name check off): red. |
| `versions_compare_as_numbers_not_as_text` | 0.10.0 is newer than 0.9.0 and non-digit parts are refused, so the downgrade rule cannot be fooled by text ordering; Sabotaged (field order swapped; digits-only check off): red. |

---

# Planned tests

Listed here so the gap between what is claimed and what is verified stays
visible. These land with the milestones in [ROADMAP.md](ROADMAP.md).

## M2 — remaining

Deferred to the milestone that makes them meaningful — they need a second node,
which does not exist until M4:

- **Storage accounting**: pledged bytes, bytes on disk and bytes in the index
  agree within the documented overhead, so a node cannot silently under-provide.
- **Data-presence audit**: after replication, Bob's store holds the expected
  chunk count for Alice, each byte-identical to what Alice would re-derive, and
  Bob can decrypt none of them.

## M3 — remaining

The convergence suite above covers the simulation, the offline-device scenario,
concurrent edits and the delete/edit race. Still outstanding:

- **Rename detection** does not re-upload chunk data. Deduplication already makes
  a rename cheap in bytes — the chunks are identical, so nothing is re-stored —
  but the operation log currently records it as a delete plus a create rather
  than as a rename, which costs a log entry. ✅ The user's intent is no longer
  lost in what they read: since 2026-10-01 a folder pass recognises a move of
  the same bytes, here or on another device, and reports it as one rename
  (`tests/reports.rs`). ⬜ The log itself still carries two operations.
- **File watching**, once M7 gives it a daemon to live in: dropped `notify`
  events under load are covered by the periodic rescan, and that rescan needs a
  test that removes events deliberately.

Deliberately *not* planned: a clock-skew test. Ordering never consults a clock —
the version vectors carry no timestamps and `recorded_unix` is advisory and read
by nothing that decides anything. A test asserting that a wrong clock changes
nothing would be asserting the absence of code that does not exist, which is the
kind of test this project treats as worse than none.

## M4 — remaining

The decoder is already exercised against every truncation and every single-bit
corruption of a valid frame, and against arbitrary garbage. Still outstanding:

- **A real fuzzing campaign** (`cargo-fuzz`). The hand-written adversarial suite
  covers the inputs someone thought of, which is exactly the set a fuzzer is
  needed to go beyond.
- **Refetch elsewhere**: a peer returning a chunk that fails to open is detected
  today (the AEAD tag catches it), but nothing yet retries the fetch against a
  different host — there is no placement layer to supply one.
- **Reputation**: a peer that fails a storage challenge should be marked
  unreliable. The challenge works; nothing records the result yet.
- **QUIC**: the transport is TLS 1.3 over TCP and fully tested; QUIC is now only
  wanted for NAT hole punching. Everything above the transport is
  transport-agnostic, so these tests should port unchanged — which is the point
  of the split, and worth checking rather than assuming.

## M5 — remaining

Minimal disruption, weighted distribution and owner affinity are all measured
above. What is not yet tested, because it is not yet built:

- **A chunk dropping below the floor is repaired without intervention.** The
  plan is computed and tested; nothing executes it. The end-to-end test needs
  the daemon.
- **A peer that fails a storage challenge is recorded as unreliable**, and
  repair stops counting it towards the floor.

## M6 — coordinator

- New-device login via username and passphrase recovers the full account.
- **Coordinator-compromise test**: dump the coordinator's entire stored state
  and assert it contains no plaintext, no canary, and no usable key material.
- A malicious coordinator serving a wrong node set cannot cause data loss.

## M7 — daemon

- The low-node-count alert actually fires when peers drop below the replication
  floor, and clears when they return.
- The sync-stalled alert fires when no round completes within the threshold.
- End-to-end: a file dropped in the folder appears on a second device with no
  user action.
