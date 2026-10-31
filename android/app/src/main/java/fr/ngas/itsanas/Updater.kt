package fr.ngas.itsanas

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.core.content.FileProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL

/**
 * The application's own updates, outside Play (2026-10-31).
 *
 * The phone is installed by hand from a GitHub release, so nothing else ever
 * tells it a newer one exists. This reads the latest release's signed
 * manifest and leaves every trust decision to the native core
 * ([Native.checkUpdate], [Native.checkApk]): the signature is checked with the
 * release key compiled into this build, the downloaded APK against the size
 * and hashes that signature covers -- the checks a desktop node makes before
 * it replaces its own binary. Android then installs it only if it is signed
 * with the same key as the installed application, and asks the person first.
 */
object Updater {
    private const val LATEST = "https://github.com/SigSegGit/itsanas/releases/latest/download"

    /** A newer release the signed manifest offers. */
    data class Offer(val version: String, val size: Long, val manifest: String, val signature: String)

    /** The offer, or null when this is the latest release. Throws on a bad signature. */
    suspend fun check(): Offer? = withContext(Dispatchers.IO) {
        val manifest = fetch("$LATEST/manifest.txt").decodeToString()
        val signature = fetch("$LATEST/manifest.txt.sig").decodeToString()
        val answer = JSONObject(Native.checkUpdate(manifest, signature))
        if (!answer.getBoolean("update")) {
            null
        } else {
            Offer(answer.getString("version"), answer.getLong("size"), manifest, signature)
        }
    }

    /**
     * The offer to show when the application opens, at most once a day: null
     * when it already asked today, when this is the latest release, or when
     * the check failed (offline, GitHub not answering). A bad signature is
     * a failure too here; Settings -> Updates shows it in words.
     */
    suspend fun dueOffer(context: Context): Offer? {
        val prefs = context.getSharedPreferences("updates", Context.MODE_PRIVATE)
        val now = System.currentTimeMillis()
        if (now - prefs.getLong("asked", 0L) < 24 * 60 * 60 * 1000L) return null
        prefs.edit().putLong("asked", now).apply()
        return try { check() } catch (e: Exception) { null }
    }

    /**
     * Download the offered APK, check it, and hand it to Android's installer.
     * Returns false when the person must first allow ITSaNAS to install apps:
     * the settings page for that has then been opened.
     */
    suspend fun install(context: Context, offer: Offer): Boolean {
        if (!context.packageManager.canRequestPackageInstalls()) {
            context.startActivity(
                Intent(
                    Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                    Uri.parse("package:${context.packageName}"),
                ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
            return false
        }
        val apk = withContext(Dispatchers.IO) {
            val dir = File(context.cacheDir, "update").apply { mkdirs() }
            dir.listFiles()?.forEach { it.delete() }
            val file = File(dir, "ITSaNAS-${offer.version}.apk")
            file.writeBytes(fetch("$LATEST/itsanas-android-apk"))
            // Throws unless this is byte for byte the APK the signature names.
            Native.checkApk(offer.manifest, offer.signature, file.path)
            file
        }
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", apk)
        context.startActivity(
            Intent(Intent.ACTION_VIEW)
                .setDataAndType(uri, "application/vnd.android.package-archive")
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK),
        )
        return true
    }

    private fun fetch(url: String): ByteArray {
        val connection = URL(url).openConnection() as HttpURLConnection
        connection.instanceFollowRedirects = true
        connection.connectTimeout = 15_000
        connection.readTimeout = 60_000
        try {
            val code = connection.responseCode
            if (code !in 200..299) {
                throw java.io.IOException("$url answered $code")
            }
            return connection.inputStream.use { it.readBytes() }
        } finally {
            connection.disconnect()
        }
    }
}
