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

    external fun create(home: String, username: String, passphrase: String): String

    external fun login(home: String, username: String, phrase: String, passphrase: String): String

    external fun open(home: String, passphrase: String): String

    external fun close(): String

    external fun status(): String

    external fun list(): String

    external fun get(path: String, destination: String): String

    external fun put(path: String, source: String): String

    external fun remove(path: String): String

    external fun sync(metadataOnly: Boolean): String

    external fun setKeep(bytes: Long, order: String, only: String): String

    external fun setPledge(bytes: Long): String

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
