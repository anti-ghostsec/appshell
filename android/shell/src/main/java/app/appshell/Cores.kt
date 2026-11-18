package app.appshell

import android.content.Context
import java.lang.ref.WeakReference

/**
 * The app's core (one per app: Leaflet's windows share their settings), and
 * which activity is which window.
 */
object Cores {
    private var core = 0L
    private var app = ""
    private val windows = HashMap<Int, WeakReference<ShellActivity>>()

    fun get(context: Context, app: String, files: List<String>): Long {
        if (core == 0L || this.app != app) {
            this.app = app
            val list = org.json.JSONArray(files).toString()
            core = Native.create(app, SafPlatform(context.applicationContext, app), list)
        }
        return core
    }

    fun current(): Long = core

    /** Folio starts over each time its window opens. */
    fun drop() {
        if (core != 0L) Native.destroy(core)
        core = 0L
        windows.clear()
    }

    fun bind(win: Int, activity: ShellActivity) {
        windows[win] = WeakReference(activity)
    }

    fun unbind(win: Int) {
        windows.remove(win)
    }

    fun window(win: Int): ShellActivity? = windows[win]?.get()

    fun any(): ShellActivity? = windows.values.firstNotNullOfOrNull { it.get() }
}
