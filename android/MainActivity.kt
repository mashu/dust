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

package dev.dioxus.main

import android.os.Build
import android.os.Bundle
import android.view.View
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

typealias BuildConfig = dev.dust.morse.BuildConfig

class MainActivity : WryActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Before Android 15 the window is not edge to edge: the system keeps
        // the content clear of the bars itself, as it always has.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM) {
            keepClearOfSystemBars()
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
