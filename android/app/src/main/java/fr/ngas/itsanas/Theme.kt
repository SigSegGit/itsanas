package fr.ngas.itsanas

import android.app.UiModeManager
import android.content.Context
import android.os.Build
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext

/** The word the page keeps (`system`, `light`, `dark`): the page decides, the native screens follow. */
enum class ThemeChoice { SYSTEM, LIGHT, DARK }

/** Read before any unlock and no secret, so plain preferences. */
object Appearance {
    private const val PREFS = "itsanas.ui"
    private const val THEME = "theme"

    var choice by mutableStateOf(ThemeChoice.SYSTEM)
        private set

    fun load(context: Context) {
        val saved = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(THEME, null)
        choice = ThemeChoice.entries.firstOrNull { it.name == saved } ?: ThemeChoice.SYSTEM
        applyToWindow(context)
    }

    /** The page chose a theme: the native screens read the same word. */
    fun follow(context: Context, word: String) = set(
        context,
        when (word) {
            "light" -> ThemeChoice.LIGHT
            "dark" -> ThemeChoice.DARK
            else -> ThemeChoice.SYSTEM
        },
    )

    /** Light, dark, or the phone's own: what the page's `data-theme` shows. */
    fun word(): String = choice.name.lowercase()

    fun set(context: Context, value: ThemeChoice) {
        choice = value
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putString(THEME, value.name).apply()
        applyToWindow(context)
    }

    /** So the first window, drawn before Compose, already has the right colour. */
    private fun applyToWindow(context: Context) {
        if (Build.VERSION.SDK_INT < 31) return
        context.getSystemService(UiModeManager::class.java)?.setApplicationNightMode(
            when (choice) {
                ThemeChoice.SYSTEM -> UiModeManager.MODE_NIGHT_AUTO
                ThemeChoice.LIGHT -> UiModeManager.MODE_NIGHT_NO
                ThemeChoice.DARK -> UiModeManager.MODE_NIGHT_YES
            },
        )
    }
}

private val Light = lightColorScheme(
    primary = Color(0xFF0B5CAD), onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFF0B5CAD), onPrimaryContainer = Color(0xFFFFFFFF),
    secondary = Color(0xFF0B5CAD), onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFD5D9DF), onSecondaryContainer = Color(0xFF16191D),
    tertiary = Color(0xFF0B5CAD), onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFFFF4D6), onTertiaryContainer = Color(0xFF5A4100),
    background = Color(0xFFF6F7F9), onBackground = Color(0xFF16191D),
    surface = Color(0xFFFFFFFF), onSurface = Color(0xFF16191D),
    surfaceVariant = Color(0xFFD5D9DF), onSurfaceVariant = Color(0xFF4A525C),
    surfaceContainerLowest = Color(0xFFFFFFFF), surfaceContainerLow = Color(0xFFFFFFFF),
    surfaceContainer = Color(0xFFFFFFFF), surfaceContainerHigh = Color(0xFFFFFFFF),
    surfaceContainerHighest = Color(0xFFFFFFFF),
    outline = Color(0xFF4A525C), outlineVariant = Color(0xFFD5D9DF),
    error = Color(0xFFB3261E), onError = Color(0xFFFFFFFF),
    errorContainer = Color(0xFFFFF4D6), onErrorContainer = Color(0xFF5A4100),
    inverseSurface = Color(0xFF16191D), inverseOnSurface = Color(0xFFF6F7F9),
    inversePrimary = Color(0xFF6CB2FF),
)

private val Dark = darkColorScheme(
    primary = Color(0xFF6CB2FF), onPrimary = Color(0xFF0B1320),
    primaryContainer = Color(0xFF6CB2FF), onPrimaryContainer = Color(0xFF0B1320),
    secondary = Color(0xFF6CB2FF), onSecondary = Color(0xFF0B1320),
    secondaryContainer = Color(0xFF353B44), onSecondaryContainer = Color(0xFFECEFF3),
    tertiary = Color(0xFF6CB2FF), onTertiary = Color(0xFF0B1320),
    tertiaryContainer = Color(0xFF3A2F10), onTertiaryContainer = Color(0xFFFFE2A0),
    background = Color(0xFF121417), onBackground = Color(0xFFECEFF3),
    surface = Color(0xFF1B1E23), onSurface = Color(0xFFECEFF3),
    surfaceVariant = Color(0xFF353B44), onSurfaceVariant = Color(0xFFB4BCC6),
    surfaceContainerLowest = Color(0xFF1B1E23), surfaceContainerLow = Color(0xFF1B1E23),
    surfaceContainer = Color(0xFF1B1E23), surfaceContainerHigh = Color(0xFF1B1E23),
    surfaceContainerHighest = Color(0xFF1B1E23),
    outline = Color(0xFFB4BCC6), outlineVariant = Color(0xFF353B44),
    error = Color(0xFFFF8A80), onError = Color(0xFF121417),
    errorContainer = Color(0xFF3A2F10), onErrorContainer = Color(0xFFFFE2A0),
    inverseSurface = Color(0xFFECEFF3), inverseOnSurface = Color(0xFF121417),
    inversePrimary = Color(0xFF0B5CAD),
)

@Composable
fun ITSaNASTheme(content: @Composable () -> Unit) {
    val dark = when (Appearance.choice) {
        ThemeChoice.SYSTEM -> isSystemInDarkTheme()
        ThemeChoice.LIGHT -> false
        ThemeChoice.DARK -> true
    }
    val activity = LocalContext.current as? ComponentActivity
    DisposableEffect(dark) {
        val clear = android.graphics.Color.TRANSPARENT
        val bars = if (dark) SystemBarStyle.dark(clear) else SystemBarStyle.light(clear, clear)
        activity?.enableEdgeToEdge(bars, bars)
        onDispose {}
    }
    MaterialTheme(colorScheme = if (dark) Dark else Light) {
        // Without this, screens outside a Scaffold draw black text on a dark window.
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background, content = content)
    }
}
