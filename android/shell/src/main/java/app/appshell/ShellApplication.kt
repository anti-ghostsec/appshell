package app.appshell

import android.app.Application
import java.io.File

/**
 * Each time the app starts afresh, whatever the WebView or anything else
 * left in the app's caches last time is deleted, before a WebView exists.
 * (When the app closes normally it clears them itself; this catches a
 * crash, or Android ending the app in the background.)
 */
class ShellApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        wipe(cacheDir)
        wipe(File(dataDir, "app_webview"))
        // Leftovers of an export that didn't finish
        File(filesDir, "export").deleteRecursively()
    }

    private fun wipe(dir: File) {
        dir.listFiles()?.forEach { it.deleteRecursively() }
        if (dir.name == "app_webview") dir.delete()
    }
}
