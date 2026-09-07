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
 * What this does **not** do is store the recovery phrase. That is shown once,
 * at creation, and is the member's to write on paper. A phone that held both
 * would be a single object whose theft loses everything.
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
        return level in 1..15 && !charging(context)
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
