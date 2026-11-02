package fr.ngas.itsanas

import android.annotation.SuppressLint
import android.content.Intent
import android.net.Uri
import android.app.AlertDialog
import android.webkit.JsResult
import android.webkit.WebChromeClient
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView

/**
 * The same page as the PC, served by the same engine on 127.0.0.1 (`pageStart`).
 *
 * The web view is locked to that one origin: JavaScript and DOM storage on (the
 * page keeps its key in `sessionStorage`), file and content access off, no mixed
 * content, no second window. Any other address a link or a script reaches goes
 * to the phone's own browser, so nothing outside the engine is ever drawn here
 * with the page's key in reach.
 */
@Composable
fun Page(url: String, onLeave: () -> Unit, modifier: Modifier = Modifier) {
    // A new page is a new port and a new key: a web view of its own, or the old
    // one stays on an address nobody serves any more.
    key(url) { PageView(url, onLeave, modifier) }
}

@SuppressLint("SetJavaScriptEnabled")
@Composable
private fun PageView(url: String, onLeave: () -> Unit, modifier: Modifier) {
    val origin = remember(url) { Uri.parse(url).let { "${it.scheme}://${it.host}:${it.port}" } }
    var view by remember { mutableStateOf<WebView?>(null) }

    // Back goes back through the page's own steps, then leaves.
    BackHandler {
        val shown = view
        if (shown != null && shown.canGoBack()) shown.goBack() else onLeave()
    }

    AndroidView(
        modifier = modifier,
        factory = { context ->
            WebView(context).apply {
                settings.apply {
                    javaScriptEnabled = true
                    domStorageEnabled = true
                    allowFileAccess = false
                    allowContentAccess = false
                    mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
                    setSupportMultipleWindows(false)
                    javaScriptCanOpenWindowsAutomatically = false
                    @Suppress("DEPRECATION")
                    saveFormData = false
                }
                // A web view with no chrome client answers every confirm() with
                // "no" and says nothing: Sign out, Withdraw, Delete and the
                // others the page asks about did nothing at all on a phone.
                webChromeClient = object : WebChromeClient() {
                    override fun onJsConfirm(
                        view: WebView,
                        url: String,
                        message: String,
                        result: JsResult,
                    ): Boolean {
                        AlertDialog.Builder(context)
                            .setMessage(message)
                            .setPositiveButton(android.R.string.ok) { _, _ -> result.confirm() }
                            .setNegativeButton(android.R.string.cancel) { _, _ -> result.cancel() }
                            .setOnCancelListener { result.cancel() }
                            .show()
                        return true
                    }

                    override fun onJsAlert(
                        view: WebView,
                        url: String,
                        message: String,
                        result: JsResult,
                    ): Boolean {
                        AlertDialog.Builder(context)
                            .setMessage(message)
                            .setPositiveButton(android.R.string.ok) { _, _ -> result.confirm() }
                            .setOnCancelListener { result.confirm() }
                            .show()
                        return true
                    }
                }
                webViewClient = object : WebViewClient() {
                    override fun shouldOverrideUrlLoading(
                        view: WebView,
                        request: WebResourceRequest,
                    ): Boolean {
                        val target = request.url
                        if ("${target.scheme}://${target.host}:${target.port}" == origin) return false
                        // Anything else is somebody else's page: the browser's.
                        runCatching {
                            context.startActivity(
                                Intent(Intent.ACTION_VIEW, target).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                            )
                        }
                        return true
                    }

                    // A dead renderer leaves a blank view that no tap revives.
                    override fun onRenderProcessGone(
                        view: WebView,
                        detail: RenderProcessGoneDetail,
                    ): Boolean {
                        onLeave()
                        return true
                    }
                }
                loadUrl(url)
                view = this
            }
        },
    )

    DisposableEffect(Unit) {
        onDispose {
            view?.apply {
                stopLoading()
                destroy()
            }
            view = null
        }
    }
}
