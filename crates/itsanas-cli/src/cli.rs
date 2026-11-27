//! The command line itself: the arguments `clap` parses into a [`Command`].

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::deleting;

/// How many machines should hold each chunk, this one included.
///

#[derive(Parser)]
#[command(
    name = "itsanas",
    version,
    about = "Peer-to-peer mutual storage: your data on their disks, unreadable to them",
    long_about = None,
)]
pub(crate) struct Cli {
    /// Where this node keeps its state.
    #[arg(long, global = true, env = "ITSANAS_HOME")]
    pub(crate) home: Option<PathBuf>,

    /// A named instance on this machine: its state is in ~/.itsanas-NAME, the
    /// home `install/provision.sh --instance NAME` set up.
    #[arg(long, global = true, env = "ITSANAS_INSTANCE")]
    pub(crate) instance: Option<String>,

    #[command(subcommand)]
    pub(crate) command: Command,
}

/// What to do with the devices on this account.
#[derive(Debug, Subcommand)]
pub(crate) enum DeviceCommand {
    /// List them, as the coordinator has them.
    List,
    /// Withdraw one, so nothing dials it again.
    ///
    /// Takes the full device id or the twelve-character short form -- the short
    /// form because that is what the error naming a dead device prints, and
    /// asking somebody to go and find the long one is asking them to do work
    /// the program can do. Also answers to `withdraw`, the word every message
    /// about it uses; `forget` stays the name because clients older than the
    /// alias are told to type it by the coordinator.
    #[command(visible_alias = "withdraw")]
    Forget {
        /// The device to withdraw.
        device: String,
    },
}

/// What `itsanas backup` does.
#[derive(Debug, Subcommand)]
pub(crate) enum BackupAction {
    /// Write this account's cold copy into DIR: empty, new, or holding an
    /// earlier copy of this account, refreshed in place.
    Export {
        dir: PathBuf,
        /// Delete from the earlier copy the chunks no file uses any more.
        #[arg(long)]
        prune: bool,
    },
    /// Read a cold copy of this account from DIR and apply it.
    Restore { dir: PathBuf },
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Create a new account on this machine and print its recovery phrase.
    Init {
        /// Account name, as it would be registered with a coordinator.
        #[arg(long)]
        username: String,
    },
    /// Restore an existing account on this machine.
    ///
    /// From the 24-word phrase by default, or from a coordinator with
    /// `--from <host:port>` if the account lodged a recovery container.
    Login {
        #[arg(long)]
        username: String,
        /// Read the 24-word phrase from this file instead of prompting.
        #[arg(long)]
        phrase_file: Option<PathBuf>,
        /// Recover from this coordinator using the passphrase alone.
        #[arg(long, conflicts_with = "phrase_file")]
        from: Option<String>,
        /// The device that coordinator must prove itself to be.
        #[arg(long, requires = "from")]
        device: Option<String>,
    },
    /// Change the passphrase that protects this machine's keys.
    ///
    /// Only this machine's keystore. Whatever starts the daemon still supplies
    /// the old one, and a recovery container lodged with a coordinator stays
    /// sealed under the passphrase it was lodged with; the command says how to
    /// update both. Reads the new passphrase from `ITSANAS_NEW_PASSPHRASE` when
    /// there is no terminal to prompt on.
    Passphrase {
        /// Also re-seal the recovery container at the coordinator under the new
        /// passphrase.
        ///
        /// Without it the container keeps the old passphrase, and a recovery
        /// months later fails with the one this machine now uses. Needs the
        /// daemon stopped, because re-sealing opens the node.
        #[arg(long)]
        recovery: bool,
    },
    /// Set this machine up, step by step, or finish a setup that stopped.
    ///
    /// Each step is checked first and skipped when done, so running it again
    /// after a failure redoes only what is missing and never makes a second
    /// identity. The 24 words and the passphrase are asked in a window of the
    /// system's own (or this terminal), never in a web page.
    Setup {
        /// Ask in this terminal instead of the web page that opens in the
        /// browser on a desktop. A session without a desktop (SSH, no
        /// DISPLAY) is asked here anyway.
        #[arg(long)]
        text: bool,
        /// Every answer but the secrets from this TOML file, asking nothing;
        /// the passphrase from `ITSANAS_PASSPHRASE`.
        #[arg(long, value_name = "FILE.toml")]
        answers: Option<PathBuf>,
        /// With --answers and an account to join: the 24 words, from this file.
        #[arg(long, requires = "answers")]
        phrase_file: Option<PathBuf>,
    },
    /// Stop this machine's background service and forget its passphrase.
    ///
    /// The keys, your files and what this machine holds for others all stay.
    /// While signed out it neither syncs nor answers the others' checks.
    Signout,
    /// Undo `signout`: ask the passphrase, check it, start the service again.
    Signin,
    /// Open this machine's settings in the browser: status, pause, sync now,
    /// interval, space offered, folder, coordinator, sign out.
    Settings {
        /// Open the page at this section: `key` is "Copy this account to a
        /// USB key", what the tray icon's item opens.
        #[arg(long, value_parser = ["key"])]
        section: Option<String>,
    },
    /// Show this node's identity, contents and hosting.
    Status {
        /// One line for a tray icon or a script: `healthy`, `paused`,
        /// `exposed`, `isolated`, `failing`, `unwritable`, `lowdisk`, `short`,
        /// `stale`, `stopped`, `departed` or `unknown`, then the snapshot's
        /// age in seconds when there is one. Never asks for a passphrase.
        #[arg(long)]
        brief: bool,
    },
    /// Stop syncing this machine's files until `itsanas resume`, or for a while
    /// with `--for 1h`.
    ///
    /// Nothing is lost: what you change here waits until you resume, and what
    /// the others change waits for you. This machine keeps hosting for them
    /// and keeps saying where it is, so pausing your own syncing never costs
    /// anybody else a copy. An idle daemon takes it within two seconds; in a
    /// round, it finishes with the machine it is talking to (five minutes at
    /// most) and dials no other. One started later starts paused.
    Pause {
        /// End the pause by itself after this long: `1h`, `8h`, `2d`, from
        /// one minute to 30 days. Without it, the pause lasts until resumed.
        #[arg(long = "for", value_name = "DURATION")]
        duration: Option<String>,
    },
    /// Start syncing again after `itsanas pause`, at once.
    Resume,
    /// Ask the running daemon for a sync round now rather than at its next
    /// interval.
    #[command(name = "sync-now")]
    SyncNow,
    /// How often the daemon syncs: `30s` to `1d`, e.g. `10m` or `1h`, or `auto`
    /// for its own setting. Without a value, says what it is.
    Interval { every: Option<String> },
    /// Application mode, for a machine that backs up a program's data:
    /// `on` or `off`. On, a change goes out two seconds after it is written,
    /// and the other machines in application mode are dialled first and
    /// every minute. Without a value, says which.
    App { state: Option<String> },
    /// Show this account's public identity.
    Whoami,
    /// Check this node's passphrase and nothing else; exit 0 when it opens the
    /// keystore. What `install/clean` asks before deleting an account, so one
    /// person cannot remove another's node on a shared machine by mistake.
    /// Runs beside the daemon: it reads the keystore, not the store.
    CheckPassphrase,
    /// List the nodes on this machine: account, home, folder, daemon.
    Instances,
    /// Rename the unnamed node `~/.itsanas` to `~/.itsanas-ACCOUNT`.
    ///
    /// Stop its daemon first. The service that started it still points at the
    /// old home; the command prints what to change.
    Migrate {
        /// Instance name to use instead of the account name.
        #[arg(long)]
        name: Option<String>,
    },
    /// List the files this node knows about.
    Ls,
    /// Store a file.
    Put {
        /// Logical path inside the account, e.g. `notes/todo.txt`.
        path: String,
        /// Local file to read. `-` reads standard input.
        source: PathBuf,
    },
    /// Retrieve a file.
    Get {
        path: String,
        /// Where to write it. Omit to write to standard output.
        destination: Option<PathBuf>,
    },
    /// Delete a file: into this machine's trash, deleted everywhere when its
    /// days run out (30 unless `itsanas trash days` says otherwise).
    Rm {
        path: String,
        /// Delete it from every machine now, leaving a tombstone so it does not
        /// come back.
        #[arg(long)]
        now: bool,
    },
    /// Every version of a file the account still knows, newest first, and
    /// whether this machine can bring it back.
    ///
    /// A version stays on a machine that held it until `itsanas gc` collects
    /// it, so a file deleted for good can come back too.
    Versions {
        /// The file, as `itsanas ls` named it.
        path: String,
        /// Make version N of the list the file again, on every machine.
        #[arg(long, value_name = "N")]
        restore: Option<usize>,
    },
    /// Give a file a new name, or a folder a new place, on every machine.
    ///
    /// Nothing is uploaded again, and a file not on this machine moves too.
    /// A name the account already uses is never replaced.
    Mv {
        /// The file or folder, as `itsanas ls` names it.
        from: String,
        /// Its new name.
        to: String,
    },
    /// Free this machine of a file of the synced folder: it stays in the
    /// account, on its other machines, and only its copy here goes.
    ///
    /// Only a file the account already holds as it is here. `--keep` brings it
    /// back on the next pass.
    Free {
        /// The file, as `itsanas ls` names it. Omit to list what is freed here.
        path: Option<String>,
        /// Keep it here again.
        #[arg(long)]
        keep: bool,
    },
    /// What waits in the trash, and the rules a delete follows.
    Trash {
        #[command(subcommand)]
        action: Option<deleting::TrashAction>,
    },
    /// Set, or show, where this node keeps what it holds for others.
    ///
    /// The default is `<home>/vault`. Another disk or a NAS works if it locks
    /// files (a local disk, iSCSI, NFS v4 with locking; not SMB). Set only while
    /// the vault is empty: to move a full one, stop the daemon, move the
    /// directory, then set the path. A configured vault that is missing or not
    /// this node's is refused at every start, never recreated empty.
    Vault {
        /// Where the vault lives. Omit to print the current one.
        path: Option<PathBuf>,
        /// Accept a path on the same disk as this node's home. Refused
        /// without it, because an unmounted NAS leaves an empty mount point
        /// on the local disk that looks like a fine new vault.
        #[arg(long)]
        same_disk: bool,
        /// Move a vault that holds data to PATH: copied, checked, then
        /// switched to. The old directory is left where it was, to delete by
        /// hand once the node runs from the new one. Run it with the daemon
        /// stopped; a running daemon holds the vault and the move is refused.
        #[arg(long = "move")]
        move_data: bool,
    },
    /// A cold copy of this account, for a disk that goes in a drawer.
    ///
    /// `export` writes every segment, every chunk this machine holds and the
    /// devices' claims, all sealed as a host holds them; `restore` reads one
    /// back into this node with no peer at all. The 24 words are not in it.
    Backup {
        #[command(subcommand)]
        action: BackupAction,
    },
    /// Write a copy of ITSaNAS onto a drive, that runs where it is plugged in.
    ///
    /// The copy is this program, for this system only, with a launcher that
    /// keeps its home on the drive. It syncs your files and hosts nobody's;
    /// its keys travel with it, sealed by your passphrase alone.
    Portable {
        /// The drive or directory to write `ITSaNAS/` into.
        dir: PathBuf,
    },
    /// Copy this account to a USB key that runs it on any computer.
    ///
    /// A portable copy (every system's program) whose home holds this
    /// account under a passphrase of its own, with a device of its own. Asks
    /// this machine's passphrase and a new one for the key, in windows of the
    /// system's own. Whoever has the key and its passphrase has the account.
    CopyToKey {
        /// The key's drive or a folder on it; `ITSaNAS/` is written there.
        dir: PathBuf,
        /// Ask the passphrases here, then copy in the background, writing
        /// each step to `<home>/copy-to-key.status`.
        #[arg(long)]
        detach: bool,
        /// The background copy's own: its passphrases on standard input.
        #[arg(long, hide = true)]
        passphrases_on_stdin: bool,
    },
    /// Set, or show, the directory kept in step with this account.
    ///
    /// Once set, files put in it are uploaded, files deleted from it are
    /// deleted everywhere, and changes from other devices appear in it.
    Folder {
        /// Where the synced folder lives. Omit to print the current one.
        path: Option<PathBuf>,
        /// Apply deletions a pass held back because there were too many.
        ///
        /// A pass that would remove most of the folder holds the deletions
        /// instead of writing them: an unmounted disk and a folder somebody
        /// emptied look identical from here, and deletions replicate to every
        /// machine of the account. This says a person looked and they really
        /// are meant to go.
        #[arg(long)]
        confirm: bool,
    },
    /// Reconcile the synced folder with the store, once.
    ///
    /// The daemon does this continuously; this is for running it by hand.
    Scan {
        /// Re-hash every file instead of trusting size and modification time.
        ///
        /// Catches a file rewritten within the same second at exactly the same
        /// length, which the fast path cannot see.
        #[arg(long)]
        deep: bool,
    },
    /// Say how much of your own data to keep on this device.
    ///
    /// A phone has a few gigabytes free and an account can have hundreds. What
    /// does not fit is not downloaded -- it stays listed and can be fetched
    /// when you open it, rather than being brought down and deleted.
    ///
    /// This is room for *your* data. `pledge` is room you offer *others*, and
    /// the two are not the same decision: having a terabyte free is not
    /// agreeing to lend a terabyte.
    Keep {
        /// e.g. `2G`, or `all` to hold everything.
        size: Option<String>,
        /// Which files matter most when the limit cannot hold them all:
        /// `newest`, `oldest` or `smallest`.
        ///
        /// Without this the limit bounds the quantity and says nothing about
        /// the choice, so what a device ends up with is whatever the log
        /// replayed first -- which is the order things were written, possibly
        /// by another machine, years ago.
        #[arg(long)]
        order: Option<String>,
        /// Hold only these paths, and let go of the rest. Repeatable.
        ///
        /// `--only all` clears the restriction. A directory prefix matches
        /// everything under it; `Photos` does not match `Photos-old`.
        #[arg(long)]
        only: Vec<String>,
    },
    /// Show what this machine can offer, what that earns, and what limits it.
    ///
    /// Two limits decide how much of your own data a device may hold: the free
    /// space on the disk the node lives on, and what you have offered other
    /// people — this network gives storage in proportion to storage provided.
    /// Both, together, before anything is committed to.
    Space {
        /// Space to offer other people, e.g. `100G`.
        #[arg(long)]
        pledge: Option<String>,
        /// Space for your own data, e.g. `10G`, or `all`.
        #[arg(long)]
        keep: Option<String>,
        /// Set them, rather than only saying whether they would fit.
        #[arg(long)]
        apply: bool,
    },
    /// Say how much space this node offers to other people.
    Pledge {
        /// e.g. `500M`, `10G`, `1T`.
        size: String,
    },
    /// The devices enrolled in this account.
    Device {
        #[command(subcommand)]
        what: DeviceCommand,
    },
    /// Show or set the address this node serves on.
    ///
    /// This is the address `register` and `announce` publish, so changing it
    /// with `serve --listen` alone is not enough: the coordinator would keep
    /// handing other members the old port and they would dial whatever now
    /// answers there. Set it here, and the published address follows.
    Listen {
        /// e.g. `0.0.0.0:9797`. Omit to print the current one.
        address: Option<String>,
    },
    /// The address to publish, when it is not where this node listens.
    ///
    /// What a member on another network dials to reach this machine: the name
    /// or public address of a forwarded port, or a global IPv6 address, with
    /// the port as seen from outside. Without it a node publishes its address
    /// on whatever LAN it is on, which is right at home and useless anywhere
    /// else.
    ///
    /// A machine that moves -- a laptop, a phone -- wants none of this. It has
    /// no address another network can dial, it takes part by dialling out, and
    /// one reachable side per pair is enough.
    Announce {
        /// e.g. `nas.example.org:9801`. Omit to print the current setting.
        address: Option<String>,
        /// Go back to publishing the address this node reaches from.
        #[arg(long, conflicts_with = "address")]
        forget: bool,
    },
    /// Tell the peers and the coordinator this machine is leaving, then exit.
    ///
    /// For a machine switched off on purpose -- sold, retired, away for a
    /// month. Every peer that keeps a ledger stops counting it as a copy at
    /// once and repairs from the devices still online, instead of believing
    /// in it for the seven days a silence takes to be noticed. The coordinator
    /// records the departure apart from a silence.
    ///
    /// Run it with the daemon stopped: one process at a time holds a node's
    /// state. First it drains: each owner is asked to stop counting this
    /// machine and to re-place what rests on it alone, and asked again every
    /// minute until nothing does or `--drain-for` minutes pass. It never
    /// refuses to finish; it says how many owners were left one replica short,
    /// never who. Afterwards `daemon` exits at once, and `serve`, `sync` and `register`
    /// refuse, until `itsanas rejoin`. The device keeps its claim, and so its
    /// slot among the account's devices: `itsanas device forget <id>`, run from
    /// another device, frees it.
    Leave {
        /// Minutes to wait for the owners to re-place what rests on this
        /// machine alone; 0 skips the drain. At most 120.
        #[arg(long, default_value_t = 30)]
        drain_for: u64,
    },
    /// Undo `leave` on this machine: the daemon may start again.
    Rejoin,
    /// Delete this account everywhere, for good.
    ///
    /// Drains this machine as `leave` does, then asks the coordinator, with
    /// the account's key, to delete the account. The hosts free what they keep
    /// for it at their next rounds (they check the account's own signature),
    /// the account's other machines stop, and the username stays retired for
    /// a year. This machine's own files are not touched: `install/clean`
    /// with `--purge-account --purge-folder` removes them.
    DeleteAccount {
        /// The username, typed again: nothing is deleted without it.
        #[arg(long)]
        confirm: String,
        /// Minutes the drain waits, as for `leave`; 0 skips it. At most 120.
        #[arg(long, default_value_t = 30)]
        drain_for: u64,
    },
    /// Serve peers.
    Serve {
        /// Address to listen on. Defaults to the configured `listen`.
        #[arg(long)]
        listen: Option<String>,
    },
    /// Serve peers and sync on a timer, in one process, until interrupted.
    ///
    /// This is how a node is meant to run. `serve` and `sync` cannot run
    /// simultaneously against the same node — the index is held under an
    /// exclusive lock — so two cron entries would fight. The daemon does both,
    /// and unlocks the keys once instead of on every scheduled sync.
    Daemon {
        #[arg(long)]
        listen: Option<String>,
        /// Seconds between sync rounds. Omit to let the sync policy decide.
        ///
        /// The policy in `itsanas-policy` is what the phone and the Mac shell
        /// use too, so leaving this alone means every machine reaches the same
        /// schedule from the same decision table instead of three copies of a
        /// number that drift apart.
        #[arg(long)]
        interval: Option<u64>,
        /// This connection is charged by the gigabyte.
        ///
        /// A laptop tethered to a phone, or a machine on a capped plan. The
        /// daemon then exchanges the signed log — kilobytes — and downloads no
        /// file contents at all, once a day rather than every five minutes.
        ///
        /// Asked for rather than detected: Windows and macOS both expose the
        /// answer, but guessing it from the interface type is how a sync tool
        /// ends up costing somebody fifty euros, and a phone's own hotspot is
        /// Wi-Fi.
        #[arg(long)]
        metered: bool,
        /// Do not announce this node on the local network, and do not listen
        /// for others.
        ///
        /// Local discovery is what lets machines in one house find each other
        /// with nothing configured. Turning it off means every peer has to be
        /// added by hand, and is for networks where broadcast traffic is
        /// unwelcome or where the node should not advertise that it exists.
        #[arg(long)]
        no_discovery: bool,
    },
    /// Run one sync round against a peer.
    Sync {
        /// Peer address, e.g. `pi.local:9797`. Omit to use configured peers.
        address: Option<String>,
        /// Exchange the log but download nothing.
        ///
        /// For an expensive connection — mobile data, or a laptop tethered to a
        /// phone. Files appear in `itsanas ls` marked "not here", and a later
        /// round without this flag fetches them.
        #[arg(long)]
        metadata_only: bool,
    },
    /// Set, or show, the coordinator this node uses.
    ///
    /// A coordinator is optional. Machines on the same network find each other
    /// with no server at all; what this adds is reaching a machine on a
    /// *different* network, and recovering an account from a passphrase.
    Coordinator {
        /// `host:port`. Omit to show the current setting.
        address: Option<String>,
        /// The device the coordinator must prove itself to be.
        ///
        /// Get it with `itsanas-coordinator --identity`. Without it an address
        /// that resolves elsewhere is trusted; with it, refused.
        #[arg(long)]
        device: Option<String>,
        /// Stop using a coordinator.
        #[arg(long, conflicts_with_all = ["address", "device"])]
        forget: bool,
    },
    /// Invite somebody to join the coordinator this node uses.
    ///
    /// Prints a code, once. It is not stored anywhere: send it to the person
    /// joining by whatever means you would have used anyway, and if it is lost,
    /// issue another.
    Invite {
        /// How many accounts it may admit. One unless you say otherwise.
        #[arg(long, default_value_t = 1)]
        uses: u32,
        /// How many days it stays valid.
        #[arg(long, default_value_t = 7)]
        days: u64,
    },
    /// Register this account and device with the configured coordinator.
    Register {
        /// Also lodge a recovery container sealed under this machine's passphrase.
        ///
        /// It lets a new machine be restored with a username and a passphrase
        /// instead of 24 words. The trade is real: anybody who steals the
        /// coordinator's database can attack that passphrase offline, so it is
        /// off unless asked for, and `--withdraw-recovery` takes it back.
        #[arg(long)]
        recovery: bool,
        /// Withdraw a previously lodged recovery container.
        #[arg(long, conflicts_with = "recovery")]
        withdraw_recovery: bool,
        /// The invitation code somebody sent you.
        ///
        /// Needed only by a coordinator that admits new members by invitation,
        /// and only the first time: re-registering is how a member refreshes
        /// their keys and never needs a fresh code.
        #[arg(long, value_name = "CODE")]
        invite: Option<String>,
    },
    /// Add a peer to the configuration.
    Peer {
        #[command(subcommand)]
        action: PeerAction,
    },
    /// Install the newest signed release in place of this program, then
    /// restart the background service on it.
    ///
    /// Only a program installed from a release updates itself, and only to a
    /// newer version whose manifest carries the release key's signature and
    /// whose download matches it byte for byte. `updates = auto` in the
    /// configuration (or the "Updates" choice in setup) has the daemon do this
    /// once a day; `notify`, the default, only says so in `status`.
    Update {
        /// Only say whether a newer release exists; change nothing.
        #[arg(long)]
        check: bool,
    },
    /// Show this account's 24 recovery words again, for a lost sheet.
    ///
    /// Asks this machine's passphrase and reads only the keystore, so it works
    /// while the daemon runs. Whoever sees the words owns the account: write
    /// them on paper, away from this machine.
    Words,
    /// Check that everything this node claims to hold is actually here.
    Doctor {
        /// Also reassemble and re-hash every file. O(data), not O(metadata).
        #[arg(long)]
        deep: bool,
    },
    /// Measure this machine: how fast it chunks, seals, stores and reads.
    ///
    /// The question is not whether a laptop is fast enough — it is whether a
    /// Raspberry Pi is, and the only person who can answer that is the person
    /// holding one. Nothing here touches your account: a throwaway identity and
    /// a scratch directory are made for the run and deleted after it.
    Bench {
        /// How much data to push through each stage, e.g. `64M`, `1G`.
        ///
        /// Generated on the fly, so a large size costs no extra memory.
        #[arg(long, default_value = "256M")]
        size: String,
        /// Fewer samples: a rough answer in a fraction of the time.
        #[arg(long)]
        quick: bool,
    },
    /// Move this node's chunks from one file each into pack files
    /// (experimental: ROADMAP M10).
    ///
    /// Run with the daemon stopped. Safe to stop at any point: until the
    /// last step the store stays as it was, and running it again finishes.
    ConvertPacks,
    /// Reclaim space from files that were deleted or overwritten.
    Gc {
        /// How long a chunk must have been unreferenced, in seconds.
        ///
        /// The grace period exists because "unreferenced" is a local judgement
        /// made with incomplete information: a peer may still be fetching a
        /// chunk whose file this device just deleted.
        #[arg(long, default_value_t = 86_400)]
        grace: u64,
    },
}

#[derive(Subcommand)]
pub(crate) enum PeerAction {
    /// Remember a peer address.
    Add { address: String },
    /// List remembered peers.
    List,
    /// Forget a peer address.
    Remove { address: String },
    /// Find another member by name and remember where their machines are.
    ///
    /// On one network the discovery beacons do this already. This is for the
    /// other case: a member somewhere else, whose address you would otherwise
    /// have to be told and type in by hand.
    Find {
        /// Their username, as registered with the coordinator.
        username: String,
    },
}
