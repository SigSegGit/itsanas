package fr.ngas.itsanas

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * Syncing while nobody is looking.
 *
 * # Why a foreground service and not `WorkManager`
 *
 * `WorkManager` is the right tool for "do this once, eventually". This is a
 * loop that dials several machines, and its interval comes from the shared
 * policy rather than from Android — five minutes when the application is open
 * and the connection is free, once a day on a metered one. A foreground service
 * with a notification is also the honest arrangement: a program that moves
 * somebody's data over their connection should be visible while it does it.
 *
 * # The interval is asked for, not chosen here
 *
 * Every loop asks `itsanas-policy` what to do, through [Vault.plan]. That is
 * the same decision table the desktop daemon has been running for weeks, so
 * this shell inherits behaviour that has been exercised rather than being the
 * first caller of it. It is also why "wait for Wi-Fi" is a sentence the
 * notification can show rather than a state the user has to infer.
 */
class SyncService : Service() {

    private val scope = CoroutineScope(SupervisorJob())
    private var loop: Job? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        channel()
        startForeground(NOTIFICATION, notification("Starting", "Opening the account"))
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (loop == null) {
            loop = scope.launch { run() }
        }
        // Restarted if the system kills it, because a sync tool that silently
        // stops is the failure this project is least able to notice.
        return START_STICKY
    }

    private suspend fun run() {
        while (scope.isActive) {
            val plan = try {
                Vault.plan(this, foreground = false)
            } catch (error: Throwable) {
                update("Not syncing", error.message ?: "the core would not answer")
                delay(RETRY_MS)
                continue
            }

            if (!plan.connects) {
                update("Waiting", plan.because)
                delay(plan.everySeconds?.times(1000) ?: RETRY_MS)
                continue
            }

            if (!Account.open) {
                update("Locked", "Open the application once to unlock this account")
                delay(RETRY_MS)
                continue
            }

            update("Syncing", plan.because)
            val line = try {
                val report = Account.syncNow(metadataOnly = !plan.movesContent)
                summarise(report, plan.because)
            } catch (error: Throwable) {
                error.message ?: "the round failed"
            }
            update(if (plan.movesContent) "Synced" else "Checked", line)

            delay(plan.everySeconds?.times(1000) ?: RETRY_MS)
        }
    }

    private fun summarise(report: String, because: String): String {
        return try {
            val json = org.json.JSONObject(report)
            val reached = json.getInt("reached")
            val adopted = json.getInt("adopted")
            val released = json.getInt("released")
            when {
                reached == 0 -> "No machine answered"
                adopted == 0 && released == 0 -> "Up to date — $because"
                released > 0 -> "$adopted in, $released let go"
                else -> "$adopted file(s) arrived"
            }
        } catch (_: Throwable) {
            because
        }
    }

    private fun update(title: String, text: String) {
        val manager = getSystemService(NotificationManager::class.java)
        manager?.notify(NOTIFICATION, notification(title, text))
    }

    private fun notification(title: String, text: String): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )

        return NotificationCompat.Builder(this, CHANNEL)
            .setContentTitle(title)
            .setContentText(text)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentIntent(open)
            .setOngoing(true)
            .setSilent(true)
            .build()
    }

    private fun channel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = getSystemService(NotificationManager::class.java) ?: return
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Syncing", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Shown while this phone is exchanging data with your machines"
            }
        )
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "sync"
        private const val NOTIFICATION = 1
        private const val RETRY_MS = 5 * 60 * 1000L

        fun start(context: Context) {
            context.startForegroundService(Intent(context, SyncService::class.java))
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, SyncService::class.java))
        }
    }
}
