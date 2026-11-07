// Dust's Android activity, in place of the one dx writes (Dioxus.toml,
// `application.android_main_activity`). dx reads this file as it is, with
// no template filled in, so the application id is written out.
//
// From Android 15 an app targeting SDK 35 or later is drawn edge to edge:
// the window reaches under the status bar, the navigation bar and a display
// cutout, and the keyboard no longer shrinks it. wry puts the webview in the
// activity's content as it is, so the page would sit under the bars and the
// keyboard would cover the answer box. Here the content keeps clear of them
// all, padded by their insets, which it then consumes, so the page is laid
// out between the bars and above the keyboard and no inset reaches it twice.
// The bars show the window's background behind them, the paper of the
// app's theme (assets/android/values*/styles.xml).
//
// No deprecated window call is made for it (Window.setStatusBarColor and
// Window.setNavigationBarColor are deprecated in Android 15): the bars'
// colours and icons come from the theme.
//
// Sync's sign-in is asked for here too (src/sync/auth/android.rs): Google
// Play services' authorization, for Drive's app data folder alone. The app
// asks over JNI and looks for the answer; nothing here calls into Rust.
// Play services needs the dependency the Gradle hook adds
// (scripts/android-gradle-hook.sh), and knows the app by its package and
// signing key, tied to the project in Google's console (docs/sync.md).

package dev.dioxus.main

import android.app.Activity
import android.os.Build
import android.os.Bundle
import android.view.View
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.Keep
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.google.android.gms.auth.api.identity.AuthorizationRequest
import com.google.android.gms.auth.api.identity.AuthorizationResult
import com.google.android.gms.auth.api.identity.Identity
import com.google.android.gms.common.api.Scope
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

typealias BuildConfig = dev.dust.morse.BuildConfig

/// Drive's app data folder: the one scope sync asks for.
private const val DRIVE_APPDATA = "https://www.googleapis.com/auth/drive.appdata"

class MainActivity : WryActivity() {
    /// Each request's answer, by its number, until the app takes it.
    private val driveAnswers = ConcurrentHashMap<Long, String>()
    private val driveRequests = AtomicLong()
    /// The request waiting on the user's answer in Play services' sheet,
    /// 0 for none: one at a time, as its answer comes back to it alone.
    @Volatile private var driveAsking = 0L
    private lateinit var driveConsent: ActivityResultLauncher<IntentSenderRequest>

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Registered before the activity starts, as a result launcher must
        // be: the user's answer comes back to it.
        driveConsent = registerForActivityResult(
            ActivityResultContracts.StartIntentSenderForResult()
        ) { result ->
            val id = driveAsking
            driveAsking = 0L
            val data = result.data
            if (result.resultCode != Activity.RESULT_OK || data == null) {
                driveAnswers[id] = "cancelled"
                return@registerForActivityResult
            }
            try {
                val authorized = Identity.getAuthorizationClient(this)
                    .getAuthorizationResultFromIntent(data)
                answerDrive(id, authorized)
            } catch (e: Exception) {
                driveAnswers[id] = "error " + (e.message ?: e.javaClass.simpleName)
            }
        }
        // Before Android 15 the window is not edge to edge: the system keeps
        // the content clear of the bars itself, as it always has.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM) {
            keepClearOfSystemBars()
        }
    }

    /// Ask Play services for a token to Drive's app data folder, `asking`
    /// the user in its own sheet where it needs to: the request's number,
    /// for the app to look for the answer by (dustDriveAnswer). Called from
    /// the app's own thread, over JNI.
    @Keep
    fun dustDriveAuthorize(asking: Boolean): Long {
        val id = driveRequests.incrementAndGet()
        runOnUiThread {
            val request = AuthorizationRequest.builder()
                .setRequestedScopes(listOf(Scope(DRIVE_APPDATA)))
                .build()
            Identity.getAuthorizationClient(this)
                .authorize(request)
                .addOnSuccessListener { result ->
                    val consent = result.pendingIntent
                    if (!result.hasResolution() || consent == null) {
                        answerDrive(id, result)
                    } else if (!asking) {
                        driveAnswers[id] = "needs-user"
                    } else if (driveAsking != 0L) {
                        driveAnswers[id] = "busy"
                    } else {
                        driveAsking = id
                        try {
                            driveConsent.launch(IntentSenderRequest.Builder(consent.intentSender).build())
                        } catch (e: Exception) {
                            driveAsking = 0L
                            driveAnswers[id] = "error " + (e.message ?: e.javaClass.simpleName)
                        }
                    }
                }
                .addOnFailureListener { e ->
                    driveAnswers[id] = "error " + (e.message ?: e.javaClass.simpleName)
                }
        }
        return id
    }

    /// The answer to the request `id`, once there is one, taken: `token
    /// <token>`, `needs-user`, `cancelled`, `busy` (the user is asked
    /// already, for another request), or `error <why>`.
    @Keep
    fun dustDriveAnswer(id: Long): String? = driveAnswers.remove(id)

    private fun answerDrive(id: Long, result: AuthorizationResult) {
        val token = result.accessToken
        val granted = result.grantedScopes.any { it == DRIVE_APPDATA }
        driveAnswers[id] = when {
            token == null -> "error no token"
            !granted -> "error Dust was not given its folder in your Google Drive"
            else -> "token $token"
        }
    }

    /// The content padded by the bars, a display cutout and the keyboard,
    /// whichever reaches furthest at the foot, on every change of them.
    private fun keepClearOfSystemBars() {
        val content = findViewById<View>(android.R.id.content)
        ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
            val bars = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
            )
            val keyboard = insets.getInsets(WindowInsetsCompat.Type.ime())
            view.setPadding(bars.left, bars.top, bars.right, maxOf(bars.bottom, keyboard.bottom))
            WindowInsetsCompat.CONSUMED
        }
    }
}
