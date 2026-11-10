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

    external fun open(home: String, passphrase: String): String

    /**
     * Debug builds and instrumented tests only: every node this process creates,
     * restores or opens is pinned to this coordinator instead of the official one.
     * Kotlin never calls it outside androidTest. [device] may be empty (no pin). A
     * release build throws.
     */
    external fun useCoordinatorForTests(address: String, device: String): String

    external fun close(): String

    /**
     * Bring the hosting loop in line with the shared policy. It is the daemon's
     * own: this says what the phone can see and draws nothing. Returns JSON
     * with `running` and `because`.
     */
    external fun hostTick(
        metered: Boolean,
        charging: Boolean,
        percent: Int,
        contentOnMetered: Boolean,
        meteredUsed: Long,
    ): String

    /** Stop the hosting loop, and wait for it. */
    external fun hostStop(): String

    /** The battery level at or below which the owner asked for the background to stop. */
    external fun batteryFloor(): Int

    /**
     * Serve the setup engine's page on 127.0.0.1 and answer its address (the
     * key is in the fragment). [mode] is "setup" or "settings". Secrets are
     * asked through [Host]; the first call finds that class, so it must come
     * from a Java thread.
     */
    external fun pageStart(home: String, mode: String, biometric: Boolean): String

    /** The person left the page: the engine stops. */
    external fun pageStop(): String

    /** Recover the passphrase from the 24 words, on the app's own screens. */
    external fun forgot(home: String): String

    /** {"ok":bool,"wordIndex":n|null,"count":n}: the words as they are typed. */
    external fun checkWords(phrase: String): String

    external fun status(): String

    /** JSON: whether the signed release offers a newer APK (see [Updater]). */
    external fun checkUpdate(manifest: String, signature: String): String

    /** "ok" when the file at [path] is the APK the signed release names. */
    external fun checkApk(manifest: String, signature: String, path: String): String

    external fun list(): String

    external fun listAt(folder: String): String

    external fun get(path: String, destination: String): String

    external fun put(path: String, source: String): String

    external fun remove(path: String): String

    external fun trashPut(path: String, days: String): String

    external fun trashList(): String

    external fun trashRecord(home: String): String

    external fun trashRestore(home: String, path: String): String

    external fun trashKeep(home: String, path: String): String

    external fun trashDelete(path: String): String

    /** Which files of the chosen folder to copy: `{"wanted":[...]}`. */
    external fun folderWanted(listing: String): String
    /**
     * Put copied files of the folder into the account, in one segment, and record
     * them: `[{"file":{...},"source":"..."}]`.
     */
    external fun folderImport(batch: String): String

    external fun sync(metadataOnly: Boolean): String

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
}

/** A folder inside the one on screen, with everything below it counted. */
data class KnownFolder(val name: String, val path: String, val files: Long, val size: Long, val notHere: Long)

/** One folder of the account. */
data class Level(val at: String, val folders: List<KnownFolder>, val files: List<Known>)

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
    val copiesWanted: Int,
    val safe: Boolean,
    val failing: Int,
    val vaultRefuses: Boolean,
    val lowDisk: Long?,
    val silentFor: Long?,
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

    fun level(json: String): Level {
        val o = JSONObject(json)
        val folders = o.getJSONArray("folders")
        return Level(
            at = o.getString("at"),
            folders = (0 until folders.length()).map { index ->
                val folder = folders.getJSONObject(index)
                KnownFolder(
                    name = folder.getString("name"),
                    path = folder.getString("path"),
                    files = folder.getLong("files"),
                    size = folder.getLong("size"),
                    notHere = folder.getLong("notHere"),
                )
            },
            files = files(json),
        )
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
            copiesWanted = o.getInt("copiesWanted"),
            safe = o.getBoolean("safe"),
            failing = o.getInt("failing"),
            vaultRefuses = o.getBoolean("vaultRefuses"),
            lowDisk = if (o.isNull("lowDisk")) null else o.getLong("lowDisk"),
            silentFor = if (o.isNull("silentFor")) null else o.getLong("silentFor"),
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
