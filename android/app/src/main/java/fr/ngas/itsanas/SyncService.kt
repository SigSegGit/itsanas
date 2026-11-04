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
import kotlinx.coroutines.CancellationException
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

    @Volatile
    private var destroyed = false

    override fun onBind(intent: Intent?): IBinder? = null

    /** Android 15's limit for this kind of service: stop, or the app is killed. */
    override fun onTimeout(startId: Int, fgsType: Int) {
        stopSelf()
    }

    /** The app was swiped away: the worker carries on from here. */
    override fun onTaskRemoved(rootIntent: Intent?) {
        stopSelf()
    }

    override fun onCreate() {
        super.onCreate()
        running = true
        channel()
        startForeground(NOTIFICATION, notification("Starting", "Opening the account"))
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Restarted by the system after the account was removed: nothing to
        // sync, and a sticky service with nothing to do is a notification that
        // never goes away.
        if (!Native.exists(Vault.home(this).absolutePath)) {
            stopSelf()
            return START_NOT_STICKY
        }
        if (loop == null) {
            loop = scope.launch { run() }
        }
        // Not restarted by the system: Android 15 gives this kind of service six
        // hours a day, and restarting it past them crashes the app. `SyncWorker`
        // carries on while the app is closed.
        return START_NOT_STICKY
    }

    private suspend fun run() {
        while (scope.isActive) {
            val plan = try {
                Vault.plan(this, foreground = false)
            } catch (error: Throwable) {
                // A cancelled coroutine is the service ending, not a failure to report.
                if (error is CancellationException) throw error
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

            // The daemon's own loop, when the policy says this phone hosts: it
            // does this account's rounds too, so none is run beside it.
            val hosting = try {
                Vault.hostTick(this)
            } catch (error: Throwable) {
                if (error is CancellationException) throw error
                Pair(false, error.message ?: "hosting would not start")
            }
            if (hosting.first) {
                if (plan.movesContent) {
                    try {
                        PhoneFolder.sync(this)
                    } catch (error: Throwable) {
                        if (error is CancellationException) throw error
                    }
                }
                update("Hosting", hosting.second)
                delay(HOST_LOOK_MS)
                continue
            }

            update("Syncing", plan.because)
            // Only when content may move, so a metered link pays for no photos.
            // A folder that fails stops nothing: the round still runs, and says so.
            val folderFailed = if (plan.movesContent) {
                try {
                    PhoneFolder.sync(this)
                    null
                } catch (error: Throwable) {
                    if (error is CancellationException) throw error
                    error.message ?: "the folder could not be read"
                }
            } else {
                null
            }
            val line = try {
                val report = Account.syncNow(metadataOnly = !plan.movesContent)
                folderFailed?.let { "Folder: $it" } ?: summarise(report, plan.because)
            } catch (error: Throwable) {
                if (error is CancellationException) throw error
                error.message ?: "the round failed"
            }
            update(if (plan.movesContent) "Synced" else "Checked", line)

            // A look a minute at most, so a phone plugged in starts hosting
            // without waiting for its next two-hour round. The round itself
            // keeps the policy's pace.
            pace(plan.everySeconds?.times(1000) ?: RETRY_MS)
        }
    }

    /** Wait out a round's pace, and end early once the policy says this phone hosts. */
    private suspend fun pace(ms: Long) {
        var left = ms
        while (left > 0 && scope.isActive) {
            delay(minOf(left, HOST_LOOK_MS))
            left -= HOST_LOOK_MS
            val hosting = Account.open && try {
                Vault.hostTick(this).first
            } catch (error: Throwable) {
                if (error is CancellationException) throw error
                false
            }
            if (hosting) return
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
        // A notification posted after onDestroy would outlive the service.
        if (destroyed) return
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
            .setSmallIcon(R.drawable.ic_stat_itsanas)
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
        running = false
        // Off the main thread: the loop's current session has to end first.
        Thread { runCatching { Native.hostStop() } }.start()
        destroyed = true
        scope.cancel()
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "sync"
        /** Whether the service is up, so the worker leaves the work to it. */
        @Volatile
        var running = false

        /** At most once a day, for a process that died with a fingerprint between it and the account. */
        fun sayLocked(context: Context) {
            if (!Vault.lockedNoticeDue(context)) return
            val manager = context.getSystemService(NotificationManager::class.java) ?: return
            manager.notify(
                NOTIFICATION + 1,
                NotificationCompat.Builder(context, CHANNEL)
                    .setContentTitle("Locked")
                    .setContentText("Open the application once to unlock this account")
                    .setSmallIcon(R.drawable.ic_stat_itsanas)
                    .setAutoCancel(true)
                    .build(),
            )
        }

        private const val NOTIFICATION = 1
        private const val RETRY_MS = 5 * 60 * 1000L
        private const val HOST_LOOK_MS = 60 * 1000L

        fun start(context: Context) {
            Vault.noteClosedByPerson(context, false)
            SyncWorker.schedule(context)
            context.startForegroundService(Intent(context, SyncService::class.java))
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, SyncService::class.java))
        }
    }
}
