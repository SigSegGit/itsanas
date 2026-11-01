package fr.ngas.itsanas

import org.json.JSONArray
import org.json.JSONObject

/**
 * Everything this application is allowed to ask the core.
 *
 * One object, one library load, and every call answers with a JSON string or
 * throws [NativeException]. The shape is deliberate: each crossing into Rust
 * allocates and can throw, so the calls are whole operations rather than field
 * getters. Listing a thousand files is one crossing.
 *
 * Every one of these blocks. Nothing here may be called on the main thread —
 * the operations touch a disk or a socket, and `Files` runs them through
 * `withContext(Dispatchers.IO)`.
 */
object Native {
    init {
        System.loadLibrary("itsanas_android")
    }

    external fun exists(home: String): Boolean

    /** {"min":6,"max":128}: the rule the screens state and check as you type. */
    external fun passphraseRule(): String

    /**
     * Whether a username can be taken, asked of the coordinator before anything is
     * written. Never throws for a network failure.
     * {"valid":true|false,"why":"..."|null,"taken":"yes"|"no"|"unknown"}
     * `valid:false` carries taken:"unknown" and is not sent to the network.
     */
    external fun checkName(username: String): String

    /**
     * The words and the name of an existing account, before anything is written.
     * {"words":"ok"|"bad","wordIndex":<1-based>|null,"count":<n>,
     *  "name":"yours"|"other"|"none"|"unknown"|"invalid","why":"..."|null}
     * `words:"bad"` means the name was not asked. A numbered grid is accepted.
     */
    external fun checkRestore(username: String, phrase: String): String

    /** A fresh identity's words, written nowhere: {"phrase":"w1 w2 ..."}. */
    external fun newPhrase(): String

    /** This node's words again, from its keystore: {"phrase":"..."}. Wrong passphrase throws. */
    external fun words(home: String, passphrase: String): String

    /** Re-seal the keystore. The new one obeys the rule; wrong current throws. {"changed":true} */
    external fun changePassphrase(home: String, current: String, new: String): String

    /**
     * Restore an account from its words, as a person saved them; a numbered grid
     * is accepted. A new account comes this way too: [newPhrase], show and check
     * the words, then this, so a rotation or a kill before it loses nothing. Sets
     * a budget for this disk, and offers nobody anything.
     */
    external fun login(home: String, username: String, phrase: String, passphrase: String): String

    external fun open(home: String, passphrase: String): String

    /**
     * "I forgot my passphrase": the words replace the node on this phone.
     * Refused (nothing touched) when the words are bad or belong to another account
     * as the coordinator lists it. Answers like login.
     */
    external fun restoreOver(home: String, phrase: String, passphrase: String): String

    /**
     * One best-effort round, then what logging out would lose. Closes nothing.
     * {"reached":n,"onlyHere":n,"liveChunks":n,"copiesElsewhere":n,
     *  "heldForOthers":bytes,"username":"..."}
     */
    external fun logoutCheck(metered: Boolean): String

    /**
     * Tell the machines, withdraw this device, close and erase the node. Never fails
     * for the network; an unreachable coordinator leaves a marker so the next login
     * withdraws the old id. {"told":n,"departed":bool,"withdrawn":bool}
     */
    external fun logout(home: String): String

    /**
     * Delete the whole account at the coordinator, then log out. [confirm] must be
     * the account's username: refused otherwise, nothing sent.
     * {"deleted":true} or throws with the coordinator's refusal.
     */
    external fun deleteAccount(home: String, confirm: String): String

    /**
     * Debug builds and instrumented tests only: every node this process creates,
     * restores or opens is pinned to this coordinator instead of the official one.
     * Kotlin never calls it outside androidTest. [device] may be empty (no pin). A
     * release build throws.
     */
    external fun useCoordinatorForTests(address: String, device: String): String

    external fun close(): String

    external fun status(): String

    /**
     * What the Space step and Settings show. All bytes.
     * {"freeBytes","totalBytes","localBytes","heldForOthers","pledgeBytes",
     *  "keepBytes"|null,"othersPledgedBytes","largestOtherPledgeBytes","accountBytes",
     *  "earnedBytes","allowedBytes","maxPledgeBytes","maxKeepBytes",
     *  "suggested":{"pledgeBytes","keepBytes","sameAsOtherMachines":bool},
     *  "split":{"own":30,"network":70}}
     * `maxPledgeBytes` and `maxKeepBytes` are what leaves the phone its headroom, each
     * taken alone: a slider ends there, and [setLimits] judges the pair.
     */
    external fun space(): String

    /** JSON: whether the signed release offers a newer APK (see [Updater]). */
    external fun checkUpdate(manifest: String, signature: String): String

    /** "ok" when the file at [path] is the APK the signed release names. */
    external fun checkApk(manifest: String, signature: String, path: String): String

    external fun list(): String

    external fun get(path: String, destination: String): String

    external fun put(path: String, source: String): String

    external fun remove(path: String): String

    external fun sync(metadataOnly: Boolean): String

    external fun setKeep(bytes: Long, order: String, only: String): String

    external fun setPledge(bytes: Long): String

    /**
     * Pledge and keep together, checked as a pair and against the disk. A negative
     * [keep] means no limit; 0 is a budget of nothing, the listing and no contents.
     */
    external fun setLimits(pledge: Long, keep: Long): String

    /** The account's devices, this phone marked. */
    external fun devices(): String

    /**
     * Withdraw one of the account's devices by its full id. Refused for this
     * phone, and for an id the account's listing does not show.
     */
    external fun withdrawDevice(id: String): String

    /**
     * Enrol this account and device with the configured coordinator. [invite]
     * may be empty; an invite-only coordinator needs a code the first time and
     * never again. At the device limit this answers rather than throws:
     * `atCap` true, the devices to choose from, and what to say. So does an
     * invitation refused: `{"enrolled":false,"atCap":false,"needsInvite":true,
     * "codeRefused":bool}`, `codeRefused` when [invite] was given and would not do.
     * A network failure still throws.
     */
    external fun register(invite: String): String

    external fun addPeer(address: String): String

    external fun removePeer(address: String): String

    external fun plan(
        metered: Boolean,
        charging: Boolean,
        batteryLow: Boolean,
        foreground: Boolean,
        contentOnMetered: Boolean,
    ): String
}

/**
 * What the core refused, in the same words the command line would print.
 *
 * Thrown from Rust rather than returned as a code, because a shell that has to
 * remember to check a return value eventually forgets, and the one it forgets
 * is "the passphrase was wrong".
 */
class NativeException(message: String) : Exception(message)

/** One file in the account, whether or not this phone holds it. */
data class Known(
    val path: String,
    val size: Long,
    val modifiedUnix: Long,
    val here: Boolean,
) {
    val name: String get() = path.substringAfterLast('/')
    val folder: String get() = path.substringBeforeLast('/', "")
}

/** What the node is and what it holds. */
data class Status(
    val username: String,
    val deviceId: String,
    val here: Long,
    val notHere: Int,
    val bytesOnDisk: Long,
    val vaultBytes: Long,
    val keepBytes: Long?,
    val pledgeBytes: Long,
    val peers: List<String>,
    val copiesElsewhere: Int,
    val onlyHere: Int,
    val liveChunks: Int,
)

/** Parsing, kept next to the calls it belongs to rather than spread about. */
object Answers {
    fun files(json: String): List<Known> {
        val array = JSONObject(json).getJSONArray("files")
        return (0 until array.length()).map { index ->
            val file = array.getJSONObject(index)
            Known(
                path = file.getString("path"),
                size = file.getLong("size"),
                modifiedUnix = file.getLong("modified"),
                here = file.getBoolean("here"),
            )
        }
    }

    fun status(json: String): Status {
        val o = JSONObject(json)
        return Status(
            username = o.getString("username"),
            deviceId = o.getString("deviceId"),
            here = o.getLong("files"),
            notHere = o.getInt("notHere"),
            bytesOnDisk = o.getLong("bytesOnDisk"),
            vaultBytes = o.getLong("vaultBytes"),
            keepBytes = if (o.isNull("keepBytes")) null else o.getLong("keepBytes"),
            pledgeBytes = o.getLong("pledgeBytes"),
            peers = strings(o.getJSONArray("peers")),
            copiesElsewhere = o.getInt("copiesElsewhere"),
            onlyHere = o.getInt("onlyHere"),
            liveChunks = o.getInt("liveChunks"),
        )
    }

    fun phrase(json: String): String = JSONObject(json).getString("phrase")

    fun strings(array: JSONArray): List<String> =
        (0 until array.length()).map { array.getString(it) }
}

/** Sizes a person reads, not bytes a machine counts. */
fun humanSize(bytes: Long): String {
    if (bytes < 1024) return "$bytes B"
    val units = listOf("KiB", "MiB", "GiB", "TiB")
    var value = bytes.toDouble() / 1024
    var unit = 0
    while (value >= 1024 && unit < units.size - 1) {
        value /= 1024
        unit++
    }
    return if (value >= 100) {
        "${value.toInt()} ${units[unit]}"
    } else {
        String.format("%.1f %s", value, units[unit])
    }
}
