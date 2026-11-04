package fr.ngas.itsanas

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.BatteryManager
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File

/**
 * The node on this phone: where it lives, how it is unlocked, and the
 * conditions the sync policy is asked about.
 *
 * # Where the passphrase lives
 *
 * Typed once, then kept in `EncryptedSharedPreferences`, whose key lives in the
 * Android Keystore and never leaves it. The alternative — prompting on every
 * launch — sounds stricter and is not: an application that asks for a
 * thirty-character passphrase every time it is opened gets a six-character one.
 *
 * What this does **not** do is store the recovery phrase. It stays sealed in the
 * node's keystore, shown only for the passphrase typed again, and is the member's
 * to write on paper.
 */
object Vault {
    private const val PREFERENCES = "itsanas.secrets"
    private const val PASSPHRASE = "passphrase"
    private const val CONTENT_ON_METERED = "content_on_metered"

    /**
     * The node's directory.
     *
     * `filesDir` rather than external storage: it is private to this
     * application, it is not indexed by the media scanner, and it survives
     * everything except an uninstall. Nothing readable is stored there anyway —
     * every chunk is sealed — but a store another application can open is a
     * store another application can corrupt.
     */
    fun home(context: Context): File = File(context.filesDir, "node")

    private fun preferences(context: Context) =
        EncryptedSharedPreferences.create(
            context,
            PREFERENCES,
            MasterKey.Builder(context).setKeyScheme(MasterKey.KeyScheme.AES256_GCM).build(),
            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
        )

    fun rememberedPassphrase(context: Context): String? =
        preferences(context).getString(PASSPHRASE, null)

    fun remember(context: Context, passphrase: String) {
        preferences(context).edit().putString(PASSPHRASE, passphrase).apply()
    }

    fun forget(context: Context) {
        preferences(context).edit().remove(PASSPHRASE).apply()
    }

    fun contentOnMetered(context: Context): Boolean =
        preferences(context).getBoolean(CONTENT_ON_METERED, false)

    fun setContentOnMetered(context: Context, allowed: Boolean) {
        preferences(context).edit().putBoolean(CONTENT_ON_METERED, allowed).apply()
    }

    /**
     * Whether this connection is charged by the gigabyte.
     *
     * The question, asked of the system rather than inferred from the interface
     * type. A phone's own hotspot is Wi-Fi and is metered; plenty of mobile
     * plans are not. Guessing costs somebody fifty euros.
     */
    fun metered(context: Context): Boolean {
        val manager = context.getSystemService(ConnectivityManager::class.java) ?: return true
        val capabilities = manager.getNetworkCapabilities(manager.activeNetwork) ?: return true
        return !capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
    }

    fun charging(context: Context): Boolean {
        val manager = context.getSystemService(BatteryManager::class.java) ?: return false
        return manager.isCharging
    }

    fun batteryLow(context: Context): Boolean {
        val manager = context.getSystemService(BatteryManager::class.java) ?: return false
        val level = manager.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)
        return level in 1..Native.batteryFloor() && !charging(context)
    }

    /** Whether the hosting loop runs now, as the shared policy decides; its reason. */
    suspend fun hostTick(context: Context): Pair<Boolean, String> = withContext(Dispatchers.IO) {
        val battery = context.getSystemService(BatteryManager::class.java)
        val percent = battery?.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) ?: 100
        val json = JSONObject(
            Native.hostTick(
                metered(context),
                charging(context),
                if (percent in 1..100) percent else 100,
                contentOnMetered(context),
                meteredUsed(context),
            )
        )
        hosting(context).edit().putLong("period", json.getLong("periodSeconds")).apply()
        Pair(json.getBoolean("running"), json.getString("because"))
    }

    private fun hosting(context: Context) =
        context.getSharedPreferences("hosting", Context.MODE_PRIVATE)

    /**
     * What this app moved over a metered link this period, each way counted, read
     * from the system's own counters between two looks. All of the app's traffic,
     * so it errs on the side of the person's data plan.
     */
    private fun meteredUsed(context: Context): Long {
        val prefs = hosting(context)
        val uid = android.os.Process.myUid()
        val total = android.net.TrafficStats.getUidRxBytes(uid) + android.net.TrafficStats.getUidTxBytes(uid)
        val now = System.currentTimeMillis() / 1000
        var used = prefs.getLong("used", 0L)
        var since = prefs.getLong("since", now)
        if (now - since >= prefs.getLong("period", 30L * 86400L)) {
            used = 0L
            since = now
        }
        val last = prefs.getLong("total", total)
        // The counters restart with the phone.
        if (metered(context) && total >= last) used += total - last
        prefs.edit().putLong("used", used).putLong("since", since).putLong("total", total).apply()
        return used
    }

    /** The person closed the account: nothing may open it behind their back. */
    fun closedByPerson(context: Context): Boolean = hosting(context).getBoolean("closed", false)

    fun noteClosedByPerson(context: Context, closed: Boolean) {
        hosting(context).edit().putBoolean("closed", closed).apply()
    }

    /** At most once a day: whether to say a killed app waits for its unlock. */
    fun lockedNoticeDue(context: Context): Boolean {
        val now = System.currentTimeMillis()
        val due = now - hosting(context).getLong("lockedSaid", 0L) >= 24L * 3600L * 1000L
        if (due) hosting(context).edit().putLong("lockedSaid", now).apply()
        return due
    }

    /** What the shared policy says to do now, and why, in one sentence. */
    suspend fun plan(context: Context, foreground: Boolean): Plan = withContext(Dispatchers.IO) {
        val json = JSONObject(
            Native.plan(
                metered(context),
                charging(context),
                batteryLow(context),
                foreground,
                contentOnMetered(context),
            )
        )
        Plan(
            scope = json.getString("scope"),
            everySeconds = if (json.isNull("everySeconds")) null else json.getLong("everySeconds"),
            because = json.getString("because"),
        )
    }
}

/** What to do now, how often, and a sentence to show for it. */
data class Plan(val scope: String, val everySeconds: Long?, val because: String) {
    val movesContent: Boolean get() = scope == "everything"
    val connects: Boolean get() = scope != "nothing"
}
