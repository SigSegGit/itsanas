package fr.ngas.itsanas

import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings

/**
 * The system's battery limits on background work. Measured on a Galaxy S25,
 * Android 17, 2026-11-12: an app not opened for some hours falls into the RARE
 * standby bucket, where its periodic worker runs a few times a day instead of
 * every fifteen minutes. An app the person lets off those limits is exempt.
 */
object Battery {
    fun unlimited(context: Context): Boolean =
        context.getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(context.packageName)

    /**
     * On: the system's own dialog asks the person. Off: only the system's list
     * can take it back, so that list opens. The app is sideloaded, never on a
     * store whose rules forbid the request.
     */
    @SuppressLint("BatteryLife")
    fun ask(context: Context, on: Boolean) {
        val intent = if (on) {
            Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:" + context.packageName))
        } else {
            Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
        }
        runCatching { context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
    }
}
