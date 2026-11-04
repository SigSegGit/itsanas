package fr.ngas.itsanas

import android.content.Context
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.delay

/**
 * The background once the app is closed. The system gives it about ten minutes,
 * every fifteen or so: the policy decides what runs in them, the same one as
 * while the app is open. Nothing here reopens what the person closed.
 */
class SyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {

    override suspend fun doWork(): Result {
        val context = applicationContext
        if (SyncService.running) return Result.success()
        if (!Native.exists(Vault.home(context).absolutePath)) return Result.success()
        if (!Account.open) {
            if (Vault.closedByPerson(context)) return Result.success()
            val passphrase = Vault.rememberedPassphrase(context)
            if (passphrase == null) {
                SyncService.sayLocked(context)
                return Result.success()
            }
            try {
                Account.reopen(context, passphrase)
            } catch (error: Exception) {
                return Result.retry()
            }
        }
        val plan = Vault.plan(context, foreground = false)
        if (!plan.connects) return Result.success()
        if (plan.movesContent) runCatching { PhoneFolder.sync(context) }
        if (Vault.hostTick(context).first) {
            delay(WINDOW_MS)
            if (!SyncService.running) runCatching { Native.hostStop() }
        } else {
            runCatching { Account.syncNow(metadataOnly = !plan.movesContent) }
        }
        return Result.success()
    }

    companion object {
        private const val NAME = "itsanas-background"
        private const val WINDOW_MS = 9 * 60 * 1000L

        fun schedule(context: Context) {
            WorkManager.getInstance(context).enqueueUniquePeriodicWork(
                NAME,
                ExistingPeriodicWorkPolicy.KEEP,
                PeriodicWorkRequestBuilder<SyncWorker>(15, TimeUnit.MINUTES).build(),
            )
        }
    }
}
