package fr.ngas.itsanas

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

/** One device of the account, as the coordinator lists it. */
data class Device(
    val id: String,
    val short: String,
    val address: String?,
    val thisPhone: Boolean,
    /** Null when the coordinator did not say, or the device never announced. */
    val silentForSeconds: Long?,
)

/**
 * The account's devices. [complete] is false when this phone could not get the
 * coordinator's full list (it is not enrolled): then the list holds what was
 * heard from this week and what the last refusal named.
 */
data class DeviceList(val devices: List<Device>, val complete: Boolean, val limit: Int)

object DeviceAnswers {
    fun list(json: String): DeviceList {
        val o = JSONObject(json)
        return DeviceList(devices(o.getJSONArray("devices")), o.getBoolean("complete"), o.getInt("limit"))
    }

    fun devices(array: JSONArray): List<Device> = (0 until array.length()).map { index ->
        val d = array.getJSONObject(index)
        Device(
            id = d.getString("id"),
            short = d.getString("short"),
            address = if (d.isNull("address")) null else d.getString("address"),
            thisPhone = d.getBoolean("thisPhone"),
            silentForSeconds = if (d.has("silentForSeconds") && !d.isNull("silentForSeconds")) {
                d.getLong("silentForSeconds")
            } else {
                null
            },
        )
    }
}

private fun heard(seconds: Long?): String = when {
    seconds == null -> "not heard from"
    seconds < 3600 -> "heard from within the hour"
    seconds < 86400 -> "heard from ${seconds / 3600} h ago"
    else -> "heard from ${seconds / 86400} day(s) ago"
}

/**
 * The account's devices, with a withdraw button beside each one but this phone.
 *
 * [refused] is what joining said when the account was at its device limit, and
 * [named] the devices that refusal listed: the screen opens on them, because
 * the phone cannot list the account's devices before it is enrolled, and the
 * lost phones the person came to withdraw are exactly the ones that are silent.
 */
@Composable
fun DevicesDialog(
    refused: String?,
    named: List<Device>,
    onClose: () -> Unit,
    complain: (String) -> Unit,
) {
    val scope = rememberCoroutineScope()
    var shown by remember { mutableStateOf<DeviceList?>(null) }
    var loading by remember { mutableStateOf(true) }
    var asking by remember { mutableStateOf<Device?>(null) }
    var said by remember { mutableStateOf<String?>(null) }
    // Copies: once a device is withdrawn, the refusal's banner and its list
    // are out of date, and a Withdraw button on a device already gone ends in
    // a refusal.
    var banner by remember { mutableStateOf(refused) }
    var fallback by remember { mutableStateOf(named) }

    suspend fun reload() {
        loading = true
        shown = try {
            Account.devices()
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (error: Throwable) {
            // Not enrolled and not answering: what the refusal named is still
            // something to act on.
            if (fallback.isEmpty()) complain(error.message ?: "could not list the devices")
            if (fallback.isEmpty()) null else DeviceList(fallback, false, 0)
        }
        loading = false
    }

    LaunchedEffect(Unit) { reload() }

    AlertDialog(
        onDismissRequest = onClose,
        confirmButton = { TextButton(onClose) { Text("Close") } },
        title = { Text("This account's devices") },
        text = {
            Column(
                Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                banner?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
                said?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
                if (loading) CircularProgressIndicator()
                val list = shown
                if (list != null) {
                    // A count only when it is the coordinator's whole list.
                    if (list.complete && list.limit > 0) {
                        Text(
                            "${list.devices.size} of at most ${list.limit} devices.",
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    if (!list.complete) {
                        Text(
                            "This phone is not enrolled, so only the devices heard from " +
                                "this week and those named when joining was refused are shown.",
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    list.devices.forEach { device ->
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                Text(
                                    device.short + if (device.thisPhone) "  (this phone)" else "",
                                    fontFamily = FontFamily.Monospace,
                                )
                                Text(
                                    (device.address ?: "no address") + " · " +
                                        heard(device.silentForSeconds),
                                    style = MaterialTheme.typography.bodySmall,
                                )
                            }
                            if (!device.thisPhone) {
                                TextButton({ asking = device }) { Text("Withdraw") }
                            }
                        }
                    }
                }
            }
        },
    )

    asking?.let { device ->
        AlertDialog(
            onDismissRequest = { asking = null },
            title = { Text("Withdraw ${device.short}?") },
            text = {
                Text(
                    "That device stops being part of this account: nothing will sync " +
                        "with it again, and its place among the ${shown?.limit?.takeIf { it > 0 } ?: 5} " +
                        "devices is freed. This cannot be undone for that device; to use " +
                        "it again, clear the app's data on it and log in afresh. " +
                        "It does not erase what that device already holds: whoever has it " +
                        "and its passphrase can still read this account.",
                )
            },
            confirmButton = {
                TextButton({
                    asking = null
                    scope.launch {
                        try {
                            said = Account.withdrawDevice(device.id)
                            banner = null
                            fallback = fallback.filter { it.id != device.id }
                            reload()
                        } catch (error: Throwable) {
                            complain(error.message ?: "could not withdraw it")
                        }
                    }
                }) { Text("Withdraw") }
            },
            dismissButton = { TextButton({ asking = null }) { Text("Cancel") } },
        )
    }
}
